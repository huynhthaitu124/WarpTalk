use serde::{Deserialize, Serialize};
use uuid::Uuid;
use warptalk_kernel::{RoomId, WorkspaceId};

/// Nil uuid stored as `scope_id` for platform rows.
pub(crate) const PLATFORM_SCOPE_ID: Uuid = Uuid::nil();

/// Where a settings document is stored. Precedence: `Room > Workspace > Platform`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "lowercase")]
pub enum Scope {
    Platform,
    Workspace(WorkspaceId),
    Room(RoomId),
}

impl Scope {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Scope::Platform => "platform",
            Scope::Workspace(_) => "workspace",
            Scope::Room(_) => "room",
        }
    }

    pub(crate) fn id(&self) -> Uuid {
        match self {
            Scope::Platform => PLATFORM_SCOPE_ID,
            Scope::Workspace(w) => w.as_uuid(),
            Scope::Room(r) => r.as_uuid(),
        }
    }
}

/// The scopes consulted for a read; platform is always included.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ScopeChain {
    pub workspace: Option<WorkspaceId>,
    pub room: Option<RoomId>,
}

impl ScopeChain {
    /// Platform only.
    pub fn platform() -> Self {
        Self::default()
    }

    pub fn workspace(workspace: WorkspaceId) -> Self {
        Self {
            workspace: Some(workspace),
            room: None,
        }
    }

    pub fn room(workspace: WorkspaceId, room: RoomId) -> Self {
        Self {
            workspace: Some(workspace),
            room: Some(room),
        }
    }

    /// Whether a change at `scope` can alter the merged value of this chain.
    pub fn is_affected_by(&self, scope: &Scope) -> bool {
        match scope {
            Scope::Platform => true,
            Scope::Workspace(w) => self.workspace == Some(*w),
            Scope::Room(r) => self.room == Some(*r),
        }
    }
}
