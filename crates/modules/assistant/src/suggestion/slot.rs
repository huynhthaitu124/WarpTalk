//! Atomic suggestion slot claim (cooldown + per-room + per-workspace budget) and dedup keys,
//! all in Redis so limits hold across worker replicas (spec 09 section 10).

use redis::aio::ConnectionManager;
use warptalk_kernel::{RoomId, WorkspaceId};

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("redis: {0}")]
    Redis(#[from] redis::RedisError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotDenied {
    Cooldown,
    RoomBudget,
    WorkspaceBudget,
}

/// Limits for one claim, taken from the configuration current for the room.
#[derive(Debug, Clone, Copy)]
pub struct SlotParams {
    pub room: RoomId,
    pub workspace: WorkspaceId,
    /// 0 = no cooldown.
    pub cooldown_seconds: u32,
    pub max_per_room: u32,
    pub room_ttl_seconds: u32,
    /// 0 = unlimited.
    pub max_per_workspace: u32,
    pub workspace_window_seconds: u32,
}

// One script: check cooldown, room budget, workspace budget; claim only when all pass.
// Checking BEFORE incrementing means a denied attempt never burns budget.
const SCRIPT: &str = r#"
local dry = ARGV[1] == '1'
local cd = tonumber(ARGV[2])
local rmax = tonumber(ARGV[3])
local rttl = tonumber(ARGV[4])
local wmax = tonumber(ARGV[5])
local wttl = tonumber(ARGV[6])
if cd > 0 and redis.call('EXISTS', KEYS[1]) == 1 then return 'cooldown' end
local n = tonumber(redis.call('GET', KEYS[2]) or '0')
if n >= rmax then return 'room_budget' end
if wmax > 0 then
  local w = tonumber(redis.call('GET', KEYS[3]) or '0')
  if w >= wmax then return 'workspace_budget' end
end
if dry then return 'ok' end
if cd > 0 then redis.call('SET', KEYS[1], '1', 'EX', cd) end
if redis.call('INCR', KEYS[2]) == 1 then redis.call('EXPIRE', KEYS[2], rttl) end
if wmax > 0 then
  if redis.call('INCR', KEYS[3]) == 1 then redis.call('EXPIRE', KEYS[3], wttl) end
end
return 'ok'
"#;

#[derive(Clone)]
pub struct SuggestionState {
    conn: ConnectionManager,
    prefix: String,
}

impl SuggestionState {
    pub fn new(conn: ConnectionManager) -> Self {
        Self::with_prefix(conn, "suggest")
    }

    pub fn with_prefix(conn: ConnectionManager, prefix: impl Into<String>) -> Self {
        Self {
            conn,
            prefix: prefix.into(),
        }
    }

    fn cooldown_key(&self, room: RoomId) -> String {
        format!("{}:cd:{room}", self.prefix)
    }
    fn room_budget_key(&self, room: RoomId) -> String {
        format!("{}:n:{room}", self.prefix)
    }
    fn workspace_budget_key(&self, ws: WorkspaceId) -> String {
        format!("{}:wn:{ws}", self.prefix)
    }

    async fn run(&self, p: &SlotParams, dry: bool) -> Result<Result<(), SlotDenied>, StateError> {
        let mut conn = self.conn.clone();
        let code: String = redis::Script::new(SCRIPT)
            .key(self.cooldown_key(p.room))
            .key(self.room_budget_key(p.room))
            .key(self.workspace_budget_key(p.workspace))
            .arg(if dry { "1" } else { "0" })
            .arg(p.cooldown_seconds)
            .arg(p.max_per_room)
            .arg(p.room_ttl_seconds.max(1))
            .arg(p.max_per_workspace)
            .arg(p.workspace_window_seconds.max(1))
            .invoke_async(&mut conn)
            .await?;
        Ok(match code.as_str() {
            "ok" => Ok(()),
            "cooldown" => Err(SlotDenied::Cooldown),
            "workspace_budget" => Err(SlotDenied::WorkspaceBudget),
            _ => Err(SlotDenied::RoomBudget),
        })
    }

    /// Read-only probe, used before the decide call so a doomed attempt spends no tokens.
    pub async fn probe(&self, p: &SlotParams) -> Result<Result<(), SlotDenied>, StateError> {
        self.run(p, true).await
    }

    /// Atomically take the next slot.
    pub async fn claim(&self, p: &SlotParams) -> Result<Result<(), SlotDenied>, StateError> {
        self.run(p, false).await
    }

    /// `SET NX EX`: true the first time `hash` is seen in this room within `ttl_seconds`.
    pub async fn first_seen(
        &self,
        room: RoomId,
        hash: &str,
        ttl_seconds: u32,
    ) -> Result<bool, StateError> {
        let mut conn = self.conn.clone();
        let r: Option<String> = redis::cmd("SET")
            .arg(format!("{}:dd:{room}:{hash}", self.prefix))
            .arg("1")
            .arg("NX")
            .arg("EX")
            .arg(ttl_seconds.max(1))
            .query_async(&mut conn)
            .await?;
        Ok(r.is_some())
    }
}
