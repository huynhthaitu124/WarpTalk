# 11. Meeting Service (code-derived spec)

Source repo: https://github.com/WarpTalk-CapstoneProject/warptalk-backend
Commit: c51b39b
Scope: `meeting/` (API, Application, Domain, Infrastructure), `shared/WarpTalk.Shared` (ErrorCodes, EventEnvelope, protos), `contracts/events/v1/meeting-*.schema.json`
Notation: `MS` = `meeting/src/WarpTalk.MeetingService.`. Key words MUST/SHOULD describe behaviour enforced in code. Tests, DTOs and generated files were not read. Only the service folder was read; baseline schema and deployment files are out of scope (see Gaps).

## 1. Responsibilities and bounded context

The service is the media-plane companion of the Translation Room service. It owns the LiveKit room that backs a translation room, the RTC roster, tracks, host powers over media, recording (egress), in-meeting chat (with translation, moderation, files, WarpBot) and the Google Meet bridge seat. It does NOT own the business room, invitations or lobby (Translation Room service, reached by gRPC), transcripts (Transcript service) or billing.

| Concern | Owner in this service |
|---|---|
| Join, token minting, lobby gate | `MeetingRoomService.JoinMeetingAsync` |
| Host controls (lock, mute, kick, reject, transfer, end) | `MeetingRoomService` (service-layer host checks, no role attributes) |
| Recording via LiveKit egress | `MeetingRoomService` + LiveKit Twirp client + `EgressReconciliationWorker` |
| LiveKit webhooks to roster/tracks | `MeetingWebhookService` |
| Chat, translation, moderation, files, WarpBot | `MeetingChatService`, `MeetingChatHub`, assistant result consumer |
| History | `MeetingHistoryController` |

## 2. Hosting

| Item | Behaviour |
|---|---|
| Ports | HTTP API plus an extra HTTP/2-only Kestrel port (50055). No `MapGrpcService` is registered: the service exposes no gRPC server (`MS.API/Program.cs`). |
| Auth | JWT bearer (`Jwt:Secret/Issuer/Audience`). `?access_token=` is accepted only on the SignalR hub and chat file download. |
| Redis | Optional. Connected only if `Redis:ConnectionString` is set (abortConnect=false). Without it pub/sub, streams, locks and the SignalR backplane are degraded. |
| SignalR | Hub `/api/v1/meetings/chat-hub`, WebSockets only (skipNegotiation), Redis backplane with channel prefix `WarpTalk.Meeting`. |
| CORS | `AllowedOrigins`; default local dev ports plus the product and admin web origins. |
| Platform settings | LiveKit, OpenAI and object-storage integration status; a recording kill switch is read on every recording start. |
| Coordination | Distributed locks and leader election (shared coordination library). |

## 3. REST API

All controllers are `[Authorize]` except the LiveKit webhook. There are no role attributes; host checks live in the service layer.

### 3.1 MeetingsController, base `api/v1/meetings`

All routes are `POST rooms/{translationRoomId}/...`.

| Route | Who | Rule |
|---|---|---|
| `join` | Any authenticated user | See section 5.1. Optional body: display name. |
| `bridge-token` | Bridge audio owner only (`ExternalBridgeConstants.IsBridgeAudioOwner`) | WT-525. Publish-only token (canPublish true, canSubscribe false) for the EXTERNAL_BRIDGE stand-in seat. Errors NotFound, Forbidden, InvalidState (409). |
| `trigger-ai` | Authenticated | Wakes the AI worker (`TriggerAiRequest`). |
| `participant/{id}/reject` | Host | Lobby reject: gRPC `RejectRoomParticipant`, revocation set REVOKED, participant IsActive=false. |
| `transfer-host/{newHostUserId}` | Original or active host | New host MUST be an active participant (VALIDATION_ERROR). gRPC `TransferRoomHost`, then gateway command. |
| `participant/{id}/mute` | Host | Cannot mute self. LiveKit `MuteParticipantMicrophone`. |
| `participant/{id}/kick` | Host | gRPC `KickRoomParticipant`, REVOKED revocation, IsActive=false. Repeat returns CONFLICT "already removed" (WT-699). |
| `lock` | Host | Body `LockRoomRequest`. Publishes gateway command `RoomLockChanged{Locked}`. |
| `mute-on-entry` | Host | Body `MuteOnEntryRequest`. |
| `recording` | Host; in EXTERNAL_BRIDGE rooms host or capturer | Body `RecordingActionRequest` action `start` or `stop`. Room type undeterminable: refused with SERVICE_UNAVAILABLE (WT-916). |
| `end` | Original or active host | See section 4.3. |

Status mapping: NotFound 404, Forbidden 403, ValidationError and InvalidState 400 (join and bridge-token use 409 for InvalidState), Conflict 409, otherwise 500.

### 3.2 Other controllers

| Controller / route | Auth | Purpose |
|---|---|---|
| `MeetingChatController` base `api/v1/meetings/rooms/{roomId}/chat`: `GET` history, `POST` send, `POST {messageId}/translate`, `POST {messageId}/moderate`, `POST files` (form upload), `GET files/{messageId}/download` | JWT (download also `?access_token=`) | Chat, section 6. |
| `MeetingHistoryController` base `api/v1/meetings/history`: `GET` (paged), `GET {roomId}` | JWT | PageSize clamped 1..50; filters status, search (ProviderRoomName), from/to; scoped to rooms the user created or participated in; includes last 5 messages per room. Detail returns NOT_FOUND or FORBIDDEN. |
| `MeetingClientEventsController` `POST api/v1/meetings/rooms/{id}/client-events` | JWT | Client diagnostics, log only. Invalid kind: VALIDATION_ERROR. Kind `connected` logs at Information, others Warning. |
| `LiveKitWebhooksController` `POST api/v1/meetings/webhooks/livekit` | Anonymous, but verified | LiveKit JWT in `Authorization` (bare or `Bearer `, WT-824), signature checked with `LiveKit:ApiSecret`. Failure 401 UNAUTHORIZED. |

### 3.3 SignalR hub `MeetingChatHub` (`[Authorize]`)

| Method | Rule |
|---|---|
| `JoinMeetingRoom(roomId)` | Joins group `meeting_chat:{roomId}`. Allowed only if `room.CreatedBy == user` or a participant row exists; never creates participants. Errors: "Unauthorized", RoomNotReady (retryable), NotAParticipant. |
| `LeaveMeetingChat(roomId)` | Leaves the group. |

Server to client events: `ChatMessageReceived`, `ChatMessageHidden`, `ChatAssistantResponsePending`, `ChatAssistantReasoning`, `ChatAssistantChunk`, `ChatAssistantToolCallStarted`, `ChatAssistantToolCallCompleted`, `ChatAssistantQuestion`, `ChatAssistantHandoff`. Sending a chat message is REST, not a hub method.

### 3.4 gRPC

Server: none. Clients: `TranslationRoomService` (GetRoomDetail, GetParticipants, TransferRoomHost, KickRoomParticipant, RejectRoomParticipant) and `BillingService` (registered, no call site found).

## 4. Domain model and state machines

Schema `meeting`. Tables:

| Table (entity) | Key columns | Notes |
|---|---|---|
| `meeting_rooms` (MeetingRoom) | TranslationRoomId, ProviderRoomName (= translationRoomId string), ActiveHostId, Status, IsActive, IsLocked, MuteOnEntry, ActiveEgressId, EndedAt, audit + soft delete | `Status` is a string mirrored from the translation-room status. No StartedAt column. |
| `rtc_stream_participants` (RtcStreamParticipant) | MeetingRoomId, UserId, ProviderIdentity (= userId), DisplayName, IsActive, JoinedAt, LeftAt | |
| `meeting_tracks` (MeetingTrack) | RtcStreamParticipantId, ProviderTrackId, MediaType Audio/Video, IsMuted, IsActive, PublishedAt, UnpublishedAt | Upsert by ProviderTrackId from webhooks. |
| `rtc_session_revocations` (RtcSessionRevocation) | MeetingRoomId, InviteeUserId/Email, GroupId, WorkspaceId, Status PENDING/ACCEPTED/REVOKED/DECLINED, ExpiresAt | Effectively a deny list: REVOKED (kick, reject) and DECLINED block join. |
| `meeting_chat_messages` | MeetingRoomId, SenderUserId, SenderType user/assistant, MessageType text/file/assistant_response, OriginalLanguage/Text, TranslationEnabled, IsHidden, Mentions (json), File* fields, SourcesJson | |
| `meeting_chat_translations` | MessageId, Source/TargetLanguage, TranslatedText, ModelUsed, PromptVersion, Confidence | Cache key (message, target, PromptVersion). |
| `meeting_chat_assistant_requests` | TriggerMessageId, RequestedByUserId, Prompt, ContextScope `recent_messages`, Status, CompletedAt | |
| `meeting_chat_moderation_events` | MessageId, ModeratedByUserId, Action `hidden`, Reason | |
| `schema_migrations` | | |

Enum `MeetingStatus {Created, Active, Finished}` exists but is effectively unused; the string `Status` is authoritative. Constants: `MaxMessageLength = 1000`, `RevokedSessionStatus = "REVOKED"`.

### 4.1 Room state

The room status mirrors the translation room (SCHEDULED, WAITING, IN_PROGRESS, ...). The service adds one transition of its own: any state to FINISHED with `ended_at` set, by `TryMarkFinishedAsync`, a single conditional UPDATE so that only one concurrent End call wins.

### 4.2 Chat assistant request

`queued` -> `processing` -> `completed` or `failed`. A failed publish marks the request failed and stores a fallback assistant message.

### 4.3 End meeting

1. Caller MUST be the original or active host.
2. LiveKit `DeleteRoom` MUST succeed first.
3. `TryMarkFinishedAsync` marks the room finished (idempotent winner).
4. The winner publishes the `__MEETING_END__` marker to stream `stt:results` (skipped when the room's SaveTranscript is false, WT-870). This triggers the summary pipeline downstream.

## 5. Business rules

### 5.1 Join (`MeetingRoomService.JoinMeetingAsync`, lines 67-390)

1. Room detail from Redis cache `meeting:room:v2:{translationRoomId}` (TTL 24 h) or gRPC `GetRoomDetails`. Unknown: NOT_FOUND.
2. Ended statuses (ENDED, FINISHED, CANCELLED, EXPIRED) MUST be refused with INVALID_STATE. BR-159-015: a scheduled link MUST expire at scheduled start plus 2 h.
3. The meeting room row is created under a provisioning lock (`meeting_room_provision:` prefix). First creation, or a change to IN_PROGRESS, publishes `meeting.started`.
4. A locked room MUST reject any non-host who is not already an active participant (FORBIDDEN "Room is locked."). REVOKED or DECLINED revocations MUST be refused; an expired invite MUST be refused; PENDING becomes ACCEPTED. Otherwise any authenticated user is allowed.
5. The RTC participant row is created or updated.
6. Lobby: when the room requires approval, a non-host who is not an admitted gRPC participant gets `IsWaitingRoom=true` and an empty token. The host claims `ActiveHostId`.
7. Token (LiveKitTokenService): canPublish true, lifetime 4 h, display name from JWT or gRPC.
8. Publishes `meeting.track_published` (`audio_track_1`) to wake the AI worker.

### 5.2 Host and bridge rules

| Rule | Source |
|---|---|
| Host-only: reject, mute, kick, lock, mute-on-entry, moderation; cannot mute self. | `MeetingRoomService` |
| Host-transfer and End accept original or active host; new host MUST be active. | `MeetingRoomService` |
| Recording control in EXTERNAL_BRIDGE rooms: host or capturer (`CanControlBridgeRecording`). | `ExternalBridgeConstants` |
| Host going offline (translation-room pub/sub) clears `ActiveHostId` by compare-and-set `ClearActiveHostIfAsync`, so replicas are idempotent. | `HostFallbackConsumerWorker` |
| A repeated kick returns CONFLICT. | WT-699 |

### 5.3 Recording

| Rule | Detail |
|---|---|
| Start takes a 30 s exclusive lock (`RecordingStartLockResource`). | |
| Errors | "already in progress" (INVALID_STATE), recording disabled by kill switch (FORBIDDEN), `LIVEKIT_EGRESS_QUOTA_EXCEEDED` (HTTP 402, from LiveKit 429 or resource_exhausted), `LIVEKIT_EGRESS_START_FAILED`, invalid action (VALIDATION_ERROR). Stop with nothing active: INVALID_STATE. |
| Output | Room composite egress through LiveKit Twirp to S3-compatible storage; optional template URL. Admin tokens last 10 min. |
| Completion | `egress_ended` webhook, or the reconciliation sweep, publishes `meeting.recording_completed` or `recording_failed`; publish failures throw so LiveKit or the sweep retries. |
| Unknown egress | An egress LiveKit does not know is cleared from the room after a 1 h grace (`EgressReconciliationService.UnknownEgressGrace`). |

### 5.4 Chat (`MeetingChatService`)

| Rule | Detail |
|---|---|
| Access | Read, send, translate and upload require `room.CreatedBy == user` or a participant row. Send without a participant row: FORBIDDEN "Not an active participant." |
| Revoked users | A kicked or rejected user (REVOKED) cannot read. A user who merely left keeps read access. |
| Hidden messages | Visible only to `room.CreatedBy`. |
| Length | MUST be at most 1000 characters (VALIDATION_ERROR). |
| Moderation | Host only ("Only host can moderate messages"). Sets IsHidden, writes a moderation event, broadcasts `ChatMessageHidden`. |
| Files | At most 25 MB, non-empty; blocked extensions `.exe .bat .cmd .sh .msi .dll .scr`. Storage S3-compatible or local; outside Development the service MUST fail fast if not S3-compatible (WT-330). |
| Translate | Same language echoes the original. Otherwise a direct OpenAI call (`OpenAI:ChatTranslationModel`, default gpt-4o-mini), cached per (message, target, PromptVersion). WarpBot meeting markers are stripped first. |
| WarpBot | A mention with Type `agent` creates an assistant request (`queued`) and publishes to stream `assistant:chat_requests` (section 7). |

## 6. Integration

### 6.1 LiveKit

| Direction | Mechanism |
|---|---|
| Service to LiveKit | Hand-rolled Twirp HTTP client: mute, DeleteRoom, egress start/stop/list. |
| LiveKit to service | Webhook `POST api/v1/meetings/webhooks/livekit`. |

Webhook events: `participant_joined` (set JoinedAt, clear LeftAt, publish event); `participant_left` (LeftAt, IsActive=false, clear ActiveHostId if the identity matches, hand departure to the bridge watcher after save); `track_published`, `track_unpublished`, `track_muted`, `track_unmuted` (MeetingTrack upsert; published also emits an event); `room_finished`; `egress_started` and `egress_updated` (informational); `egress_ended` (completion).

### 6.2 Events published

Envelope `DomainEventEnvelope`, schema_version 1, producer `meeting-service`. Payloads are snake_case JSON. Catalog schemas live in `contracts/events/v1/meeting-*.schema.json`.

| Event | Payload fields | Transport |
|---|---|---|
| `meeting.started` | translation_room_id, workspace_id, title, description | Redis pub/sub channel named by the event type |
| `meeting.track_published` | room_name, participant_identity, track_id, published_at | pub/sub |
| `meeting.participant_joined` | room_name, participant_identity, joined_at | pub/sub |
| `meeting.recording_started` | translation_room_id, egress_id, started_at | Redis stream `meeting:domain-events` (fields event_id, event_type, schema_version, envelope) |
| `meeting.recording_completed` | translation_room_id, egress_id, file_url, file_format, file_size_bytes, contains_raw_audio, contains_raw_video | stream `meeting:domain-events` |
| `meeting.recording_failed` | translation_room_id, egress_id, reason, livekit_status, livekit_error | stream `meeting:domain-events` |

### 6.3 Other Redis keys, streams and channels

| Name | Kind | Direction | Notes |
|---|---|---|---|
| `stt:results` | stream | publish | `__MEETING_END__` marker: segment_id, meeting_id, speaker_id `system`, text, language `system`, confidence, start_ms, end_ms, chunk_index, is_final_chunk, timestamp_ms |
| `assistant:chat_requests` | stream | publish | request_id, conversation_id (= room id), workspace_id, user_id, origin `meeting_chat`, bearer_token, history_json, page_context_json, mentions_json, timestamp_ms |
| `assistant:chat_results` | stream | consume | group `meeting-chat-consumers`; result kinds chunk, tool_call_started, tool_call_completed, final content, reasoning, question, handoff. Final result is stored as a message with SenderType `assistant`, MessageType `assistant_response`, sources_json. |
| `assistant:chat_results:meeting-chat-dead-letter` | stream | dead letter | after 5 attempts |
| `assistant:chat_results:meeting-chat-retries` | hash | retry counters | |
| `translationRoom:participant-offline` | pub/sub | consume | host fallback |
| `warptalk:translation-room:commands` | pub/sub | publish | `{Command, RoomId, ...}`: `RoomLockChanged{Locked}`, `RecordingStateChanged{Recording}`, host transfer |
| `meeting:room:v2:{id}` | key | cache | 24 h |

No MassTransit or RabbitMQ usage exists in this service.

### 6.4 Google Meet bridge (EXTERNAL_BRIDGE)

A stand-in seat (identity `ExternalBridgeConstants.ParticipantUserId`) is published by the bridge audio owner via `bridge-token`. A capturer (`BridgeCapturerUserId`) and the host may control recording. When the bridge participant leaves, `BridgeRecordingEndWorker` ends the recording after a grace (see section 8).

## 7. Background jobs

| Job | Behaviour |
|---|---|
| `HostFallbackConsumerWorker` | Subscribes to `translationRoom:participant-offline` on every replica; retry delay 2 s doubling to 30 s; calls `HandleHostOfflineAsync`. |
| `MeetingChatAssistantResultConsumerService` | Redis stream consumer. Reclaim idle 30 000 ms, MaxAttempts 5, batch 10, poll 500 ms, leader lease `meeting:chat-assistant-results`. A Program.cs comment records that it was once never registered (bug, fixed). |
| `EgressReconciliationWorker` | Every 2 min, lock `meeting:egress-reconciliation` (TTL 1 min); reconciles active egress ids against LiveKit. |
| `BridgeRecordingEndWorker` | Singleton; up to 4 checks per departure; grace `Meeting:BridgeRecording:EndGraceSeconds`. |

## 8. Configuration

Secrets, connection strings and hostnames are omitted; values below are code or appsettings defaults.

| Key | Default / note |
|---|---|
| `Redis:ConnectionString` | optional |
| `Jwt:Issuer`, `Jwt:Audience`, `Jwt:Secret` | `WarpTalk.AuthService`, `WarpTalk`, secret supplied by environment |
| `GrpcUrls:TranslationRoomService` | local port 50052 (dev) |
| `GrpcUrls:BillingService` | local port 50056 (dev) |
| `LiveKit:Url`, `ApiKey`, `ApiSecret` | supplied by environment |
| `LiveKit:Egress:S3:*` | bucket `warptalk-recordings`, region `auto`; credentials and endpoint supplied by environment |
| `LiveKit:Egress:TemplateBaseUrl` | optional |
| `OpenAI:ApiKey`, `OpenAI:ChatTranslationModel` | model default `gpt-4o-mini` |
| `Meeting:BridgeRecording:EndGraceSeconds` | 10, clamped to [3, 60] |
| `Storage:Provider`, `Storage:*` | chat file storage, S3 or local |
| `AllowedOrigins` | local dev ports and product origins |
| `Smtp:*` | present, supplied by environment |
| Code constants | token lifetime 4 h, admin token 10 min, join cache 24 h, recording start lock 30 s, upload cap 25 MB, message cap 1000, history page 1..50 |

## 9. Error codes

Shared codes in `shared/WarpTalk.Shared/ErrorCodes.cs`: UNAUTHORIZED, FORBIDDEN, NOT_FOUND, INVALID_STATE, VALIDATION_ERROR, CONFLICT, INTERNAL_SERVER_ERROR, SERVICE_UNAVAILABLE. Service-specific: `LIVEKIT_EGRESS_QUOTA_EXCEEDED` (402), `LIVEKIT_EGRESS_START_FAILED`. The chat service uses the string literals NOT_FOUND, FORBIDDEN, VALIDATION_ERROR.

## 10. Migrations (`meeting/database/migrations`)

Only four exist; the baseline schema is not in this folder.

| Date | Change |
|---|---|
| 2026-08-12 14:00 | `rename_rtc_tables`: meeting_participants to rtc_stream_participants, meeting_invitations to rtc_session_revocations |
| 2026-08-12 20:00 | `add_rtc_participant_display_name` (WT-356): nullable display_name |
| 2026-08-20 21:05 | `meeting_chat_message_sources`: sources_json jsonb NULL |
| 2026-09-03 12:00 | `remove_retired_collaboration_features` (WT-603): drops polls, Q&A, breakouts; renames indexes |

## 11. Differences vs Report 7 (06-service-design.md section 4)

| # | Report 7 | Code |
|---|---|---|
| 1 | Events `meeting.end_room`, `meeting.billing.stop`; entities MeetingParticipant, MeetingChatSession | Absent. End deletes the LiveKit room and publishes `__MEETING_END__` on `stt:results`. Tables are renamed rtc_*. |
| 2 | Chat translation via a translation worker stream | Direct OpenAI call, cached in `meeting_chat_translations`. |
| 3 | Hub methods SendChatMessage, BroadcastAiSuggestion | Hub has only JoinMeetingRoom and LeaveMeetingChat; sending is REST. |
| 4 | Assistant answers saved through a SaveAssistantResponse flow | Redis-stream consumer with retries and a dead-letter stream. |
| 5 | Short endpoint list | Code adds lock, mute-on-entry, recording, kick, reject, transfer-host, client-events, file upload/download, moderation, history, bridge-token, webhook. |
| 6 | No recording lifecycle, tracks, revocations or Google Meet bridge | Present (sections 4, 5.3, 6.4). |
| 7 | MeetingRoom.StartedAt | No such column; only EndedAt. |
| 8 | Use of gRPC both ways | Meeting is a gRPC client only. |

## 12. Gaps

Baseline schema DDL, Dockerfile and deployment config, `.agents/resources` diagrams, and the contracts JSON schemas were not read. Billing client has no call site found. `MeetingStatus` enum use was not traced beyond a grep.
