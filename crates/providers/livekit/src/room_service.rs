//! LiveKit `RoomService` over Twirp (JSON), behind a trait so callers can fake it.

use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde::Serialize;
use serde_json::{Value, json};

use crate::{LiveKitConfig, LiveKitError, admin_token};

#[async_trait]
pub trait RoomService: Send + Sync {
    async fn create_room(
        &self,
        name: &str,
        empty_timeout_secs: u32,
        metadata: &str,
    ) -> Result<(), LiveKitError>;
    async fn delete_room(&self, name: &str) -> Result<(), LiveKitError>;
    async fn mute_published_track(
        &self,
        room: &str,
        identity: &str,
        track_sid: &str,
        muted: bool,
    ) -> Result<(), LiveKitError>;
    async fn remove_participant(&self, room: &str, identity: &str) -> Result<(), LiveKitError>;
}

/// Twirp client: `POST {url}/twirp/livekit.RoomService/{Method}` with an admin bearer token.
pub struct RoomServiceClient {
    cfg: LiveKitConfig,
    http: reqwest::Client,
}

impl RoomServiceClient {
    pub fn new(cfg: LiveKitConfig) -> Result<Self, LiveKitError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| LiveKitError::Http(e.to_string()))?;
        Ok(Self { cfg, http })
    }

    async fn call<B: Serialize + Sync>(
        &self,
        method: &str,
        room: &str,
        body: &B,
    ) -> Result<Value, LiveKitError> {
        let token = admin_token(&self.cfg, room)?;
        let url = format!("{}/twirp/livekit.RoomService/{method}", self.cfg.http_url());
        let resp = self
            .http
            .post(url)
            .bearer_auth(token)
            .json(body)
            .send()
            .await
            .map_err(|e| LiveKitError::Http(e.to_string()))?;
        let status = resp.status();
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| LiveKitError::Http(e.to_string()))?;
        if status.is_success() {
            return Ok(serde_json::from_slice(&bytes).unwrap_or(Value::Null));
        }
        let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        Err(LiveKitError::Api {
            status: status.as_u16(),
            code: v["code"].as_str().unwrap_or("unknown").to_owned(),
            msg: v["msg"].as_str().unwrap_or("").to_owned(),
        })
    }
}

#[async_trait]
impl RoomService for RoomServiceClient {
    async fn create_room(
        &self,
        name: &str,
        empty_timeout_secs: u32,
        metadata: &str,
    ) -> Result<(), LiveKitError> {
        self.call(
            "CreateRoom",
            "",
            &json!({ "name": name, "emptyTimeout": empty_timeout_secs, "metadata": metadata }),
        )
        .await
        .map(|_| ())
    }

    async fn delete_room(&self, name: &str) -> Result<(), LiveKitError> {
        self.call("DeleteRoom", "", &json!({ "room": name }))
            .await
            .map(|_| ())
    }

    async fn mute_published_track(
        &self,
        room: &str,
        identity: &str,
        track_sid: &str,
        muted: bool,
    ) -> Result<(), LiveKitError> {
        self.call(
            "MutePublishedTrack",
            room,
            &json!({ "room": room, "identity": identity, "trackSid": track_sid, "muted": muted }),
        )
        .await
        .map(|_| ())
    }

    async fn remove_participant(&self, room: &str, identity: &str) -> Result<(), LiveKitError> {
        self.call(
            "RemoveParticipant",
            room,
            &json!({ "room": room, "identity": identity }),
        )
        .await
        .map(|_| ())
    }
}

/// Does nothing and always succeeds.
pub struct NoopRoomService;

#[async_trait]
impl RoomService for NoopRoomService {
    async fn create_room(&self, _: &str, _: u32, _: &str) -> Result<(), LiveKitError> {
        Ok(())
    }
    async fn delete_room(&self, _: &str) -> Result<(), LiveKitError> {
        Ok(())
    }
    async fn mute_published_track(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: bool,
    ) -> Result<(), LiveKitError> {
        Ok(())
    }
    async fn remove_participant(&self, _: &str, _: &str) -> Result<(), LiveKitError> {
        Ok(())
    }
}

/// A call recorded by [`InMemoryRoomService`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomServiceCall {
    CreateRoom(String),
    DeleteRoom(String),
    MutePublishedTrack {
        room: String,
        identity: String,
        track_sid: String,
        muted: bool,
    },
    RemoveParticipant {
        room: String,
        identity: String,
    },
}

/// Test fake: records calls, always succeeds.
#[derive(Default)]
pub struct InMemoryRoomService {
    calls: Mutex<Vec<RoomServiceCall>>,
}

impl InMemoryRoomService {
    pub fn calls(&self) -> Vec<RoomServiceCall> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn push(&self, c: RoomServiceCall) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(c);
    }
}

#[async_trait]
impl RoomService for InMemoryRoomService {
    async fn create_room(&self, name: &str, _: u32, _: &str) -> Result<(), LiveKitError> {
        self.push(RoomServiceCall::CreateRoom(name.to_owned()));
        Ok(())
    }
    async fn delete_room(&self, name: &str) -> Result<(), LiveKitError> {
        self.push(RoomServiceCall::DeleteRoom(name.to_owned()));
        Ok(())
    }
    async fn mute_published_track(
        &self,
        room: &str,
        identity: &str,
        track_sid: &str,
        muted: bool,
    ) -> Result<(), LiveKitError> {
        self.push(RoomServiceCall::MutePublishedTrack {
            room: room.to_owned(),
            identity: identity.to_owned(),
            track_sid: track_sid.to_owned(),
            muted,
        });
        Ok(())
    }
    async fn remove_participant(&self, room: &str, identity: &str) -> Result<(), LiveKitError> {
        self.push(RoomServiceCall::RemoveParticipant {
            room: room.to_owned(),
            identity: identity.to_owned(),
        });
        Ok(())
    }
}
