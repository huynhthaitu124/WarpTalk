# 10. Translation Room Service (code-derived spec)

Source repo: https://github.com/WarpTalk-CapstoneProject/warptalk-backend
Commit: c51b39b
Scope: `translation-room/` (layers Domain, Application, Infrastructure, API), plus `shared/WarpTalk.Shared/Protos/translation_room.proto` and the Gateway `TranslationRoomHub`. Read-only static reading; nothing was built or run. Configuration values are placeholders; no secrets or hosts are reproduced.
Notation: `TR` = `translation-room/src/WarpTalk.TranslationRoomService.`; `Svc` = `TR.Application/Services/TranslationRoomService.cs`. Line numbers refer to commit c51b39b.

## 1. Responsibilities and bounded context

The service owns the business entity "Translation Room" and everything attached to it: lifecycle, roster, invitations, recurring series, per-pair audio routes, sessions, post-meeting artifacts, minutes, action items and feedback. Data lives in Postgres schema `translation_room`.

| Concern | Owner | Evidence |
|---|---|---|
| Room lifecycle, roster, series, routes, artifacts, minutes, feedback | This service | controllers listed in section 2 |
| LiveKit room creation, join tokens, kick at media level, `GetMediaUsage` data collection | Meeting service (not in this repo slice) | LiveKit appears here only in migration comments and proto comments; no LiveKit client in `translation-room/` |
| Realtime hub (SignalR) | Gateway | `gateway/src/WarpTalk.Gateway/Hubs/TranslationRoomHub.cs` |
| Plan limits, workspace language policy, membership | Workspace service (gRPC client) | `IWorkspaceMeetingPolicy`, `IWorkspaceMemberDirectory` |
| Voice-clone consent, user language defaults | Auth service (gRPC client) | `VoiceConsentGrpcDirectory`, `UserSettingsGrpcDirectory` |
| Credit/subscription suspension flags | Billing (writes Redis keys; this service only reads them) | `EnsureTranslationCreditsAsync` |
| STT/MT/TTS, summaries, facts | AI workers (via Redis streams) | section 7 |

The service uses Redis (pub/sub, streams, keys) as its only message bus. No MassTransit or RabbitMQ is used in this service.

## 2. Interfaces

### 2.1 REST (base `api/v1`, class-level `[Authorize]` unless noted)

`TranslationRoomsController` (`translation-rooms`)

| Method | Route | Purpose |
|---|---|---|
| GET / POST | `` | list / create room (rate limited, see 5.7) |
| GET | `{id}` | room detail |
| POST | `join`; `{id}/join` | join by code / by id |
| POST | `{id}/waiting` | enter waiting state |
| POST | `{id}/start`, `{id}/pause`, `{id}/resume`, `{id}/translation/stop`, `{id}/end`, `{id}/cancel` | lifecycle (section 3.1) |
| PUT | `{id}/settings`; `{id}/artifact-access` | settings (locked once IN_PROGRESS); artifact visibility |
| POST | `{id}/languages` | add a room target language |
| GET | `join-language-policy/{code}`; `history`; `documents`; `my-meetings` | pre-join policy; personal lists |
| GET | `{id}/artifacts`; `{id}/calendar.ics` | artifacts; calendar file |
| GET/POST | `{id}/invitations`; POST `{id}/invitations/accept` | invitations |
| GET/POST | `{id}/feedback/me`; `{id}/feedback` | feedback |

`TranslationRoomParticipantsController`: GET `{id}/participants`; PUT `{id}/participants/{participantId}/audio`, `/admit`, `/kick`; PUT `{id}/participants/me/leave`.

`TranslationRoomSessionsController` (`translation-rooms/{roomId}/sessions`): POST, GET, PATCH `{sessionId}`, POST `{sessionId}/end`.

`TranslationRoomAudioRouteController` (`translation-rooms/{roomId}/audio-routes`): POST `generate`; GET; PATCH `{routeId}/runtime`, `{routeId}/voice-clone`; POST `dub-voice/refresh`, `voice-clone-consent`, `noise-suppression/report`; GET/PUT `flash-mode`, `noise-reduction`.

`TranslationRoomSeriesController` (`translation-room-series`): GET `{id}`, PATCH `{id}`, POST `{id}/cancel`.

`BridgeRoomsController` (`translation-rooms`): POST `bridge/claim`; POST `{id}/bridge/capturer/heartbeat`, `/takeover`; PUT `{id}/bridge/audio-mode`, `/voice-clone-consents`; POST `{id}/bridge/voice-clone-consents/status`.

`PublishedLanguagesController`: GET `translation-rooms/published-languages`.

`RoomArtifactsController` (`room-artifacts`): GET `{id}/download`; POST `{id}/consent`; GET `rooms/{roomId}/summary`, `/summary/renderings`, `/summary/regenerate/{requestId}`; POST `rooms/{roomId}/summary/regenerate`.

`MeetingMinutesController` (`rooms/{roomId}/minutes`): GET `translation`, ``, `export.docx`, `export.pdf`, `share`; POST `draft`, `{minutesId}/sign`, `/approve`, `/revise`, `share/people`; PUT `{minutesId}`, `{minutesId}/secretary`; PATCH/DELETE `share`; DELETE `share/people`.
`SharedMinutesController` (`shared/minutes`, `[AllowAnonymous]`): GET `{token}`, `{token}/export.docx`, `{token}/export.pdf`. `WorkspaceMinutesController`: GET `workspaces/{workspaceId}/minutes`.
`MeetingActionItemsController`: GET/POST `rooms/{roomId}/action-items`; GET `workspaces/{workspaceId}/action-items/mine`; PUT `action-items/{itemId}/status`.

Admin (`api/v1/admin/...`): `AdminMeetingsController` (`meetings`: GET ``, `counts`, `insights`) is guarded by `[RequirePermission(AdminPermissions.MeetingsRead)]` (`AdminMeetingsController.cs:22`), metadata only, no join/control. `AdminFeedbackController` (`summary`, `comments`), `AdminReportsController` (POST `pdf`), `AdminLanguagesController` (GET, POST, PUT `{code}`, POST `{code}/enable|disable`): permission attributes were not individually verified (gap).

### 2.2 gRPC server (`TR.API/GrpcServices/TranslationRoomGrpcService.cs`, proto `TranslationRoomService`)

| RPC | Purpose / rule |
|---|---|
| GetTranslationRoomById, GetTranslationRoomByCode, GetParticipantsByRoomId | room and roster lookup |
| GetRoomPeopleNames (WT-422) | names for STT bias in Transcript service |
| GetActiveRoomCountByWorkspace | active room count |
| TransferRoomHost (WT-359) | Meeting service owns the action; this service owns the data (3.3) |
| KickRoomParticipant (WT-564) | sets KICKED (terminal, BR-010 on join) |
| RejectRoomParticipant (WT-699) | host only; WAITING or INVITED to REJECTED |
| GetMediaUsage | LiveKit usage per UTC hour and workspace for billing; window over 120 days returns INVALID_ARGUMENT (`MediaUsageCalculator`) |

### 2.3 SignalR (Gateway, `/hubs/translation-room`, `[Authorize]`, `Program.cs:339`)

`TranslationRoomHub` methods: JoinTranslationRoom, LeaveTranslationRoom, ToggleMute, RaiseHand, SendReaction, SendCollaborativeNoteDelta, AdmitWaitingParticipant, SpotlightParticipant, MuteAll, SetListenLanguage, SetSpeakLanguage, SetExternalMeetingLanguage, ReportFarSpeakerHints, SetVoicePreference, GetVoiceCatalog, SendChatMessage, EndTranslationRoom, SendAudioChunk. `TranslationRoomRedisSubscriberService` (Gateway) subscribes to `warptalk:translation-room:commands` and relays commands to clients.

## 3. Domain model and states

### 3.1 Room lifecycle

Statuses (`RoomStatus`): SCHEDULED, WAITING, OPEN, IN_PROGRESS, PAUSED, ENDED, CANCELLED, EXPIRED, FAILED. Terminal for guards: ENDED, CANCELLED, EXPIRED (`TranslationRoomConstants.TerminalStatuses`); bridge rooms also treat FAILED as closed (`BridgeRoomConstants.ClosedStatuses`).

| Transition | Method | Guard |
|---|---|---|
| create to SCHEDULED or WAITING | `CreateTranslationRoomCoreAsync` (Svc:404) | SCHEDULED if `ScheduledAt` set, else WAITING |
| SCHEDULED to WAITING | `OpenWaitingRoomAsync` | host only |
| SCHEDULED to OPEN | `OpenScheduledRoomAsync` (worker) | idempotent compare-and-set; publishes `MeetingOpened` |
| SCHEDULED/WAITING/OPEN to IN_PROGRESS | `StartTranslationRoomAsync` | host; or any accessible user when `requires_approval` is false; workspace must be able to host; compare-and-set via `TryTransitionStatusAsync`, concurrent start is a no-op success |
| IN_PROGRESS to PAUSED | `PauseTranslationRoomAsync` | effective host; ends active session; audio event `room_pause` |
| PAUSED/IN_PROGRESS to IN_PROGRESS | `ResumeTranslationRoomAsync` | start-translation access (host, current bridge capturer, or participant when `participants_can_start_translation`); credit check; new session; audio event `room_resume` |
| IN_PROGRESS stays | `StopTranslationAsync` | effective host or bridge capturer; ends session only; event `translation_stopped` |
| SCHEDULED/WAITING/OPEN to CANCELLED | `CancelTranslationRoomAsync` | host only; CONNECTED become DISCONNECTED |
| SCHEDULED/WAITING/OPEN to EXPIRED | `ExpireTranslationRoomAsync` | system, idempotent |
| IN_PROGRESS/PAUSED/WAITING/OPEN to ENDED | `EndTranslationRoomAsync` | effective host or workspace owner/admin (`RoomHostAccess`); already ENDED returns success |

Start does not check credits and does not create a session (`StartTranslationRoomAsync` has no call to `EnsureTranslationCreditsAsync` or `StartNewTranslationSessionAsync`); both happen on Resume. A start on an already IN_PROGRESS room only republishes languages and regenerates routes. End: CONNECTED participants become DISCONNECTED, `ApplyRecordAutoShareAtEndAsync` runs in the transaction, then metrics, `RoomEnded` command and audio event `session_ends`, and artifact finalization is queued (section 6).

Room types (`TranslationRoomTypes`): EVENT, CHANNEL_MEETING, WEBINAR, COMPANY_MEETING, VIRTUAL_APPOINTMENT, LIVE_EVENT, EXTERNAL_BRIDGE; legacy INSTANT and SCHEDULED. Defaults (`TranslationRoomTypePolicy`):

| Type | Approval | Mute on entry | Auto record | Breakouts | Max participants |
|---|---|---|---|---|---|
| EVENT, INSTANT, SCHEDULED | no | no | no | yes | 100 |
| CHANNEL_MEETING | no | no | no | yes | 50 |
| WEBINAR | yes | yes | yes | no | 500 |
| COMPANY_MEETING | no | yes | yes | yes | 500 |
| VIRTUAL_APPOINTMENT | yes | no | no | no | 2 |
| LIVE_EVENT | yes | yes | yes | no | 1000 |
| EXTERNAL_BRIDGE | no | no | no | no | 20 (stand-in seat excluded) |

Room settings JSON defaults (`TranslationRoomSettings`): requires_approval true, artifact_access HOST_ONLY (or ALL_PARTICIPANTS), auto_share_record null, mute_on_entry false, auto_record false, breakouts_enabled true, participants_can_start_translation false, save_transcript true. Settings MUST NOT change once IN_PROGRESS (`ErrorSettingsLocked`). A language can be added only while WAITING, IN_PROGRESS or PAUSED (`RoomLanguageAddableStatuses`).

### 3.2 Participants

Roles: PARTICIPANT, HOST. Statuses: INVITED, WAITING, CONNECTED, DISCONNECTED, LEFT, KICKED, REJECTED. Only CONNECTED holds a seat (`HoldsSeat`).

| Action | Rule (`TranslationRoomParticipantService`) |
|---|---|
| Join (`JoinTranslationRoomAsync`, Svc:1157) | order: room exists; not terminal; workspace can host; languages fit room and workspace allow-list; KICKED/REJECTED refused (Forbidden); capacity (Conflict, non-host, not already seated); status WAITING if approval required and caller is not host, else CONNECTED; PENDING invitation by email becomes ACCEPTED; if room IN_PROGRESS add routes; if room OPEN and caller CONNECTED auto-start |
| Admit | host authority; participant must be WAITING; to CONNECTED; command `ParticipantAdmitted` |
| Reject | host only; WAITING or INVITED to REJECTED; command `ParticipantRejected` |
| Kick | effective host only; host cannot be kicked; already KICKED is Conflict; KICKED is terminal |
| Leave | KICKED/REJECTED no-op; WAITING to INVITED; otherwise LEFT |
| Disconnect (offline event) | CONNECTED to DISCONNECTED; WAITING to INVITED |
| Reconnect | DISCONNECTED to CONNECTED only if room IN_PROGRESS, WAITING or PAUSED and capacity allows |

External flag: `isExternal` = not host and not a workspace member (`IsExternal`).

### 3.3 Active host and host transfer

`EffectiveHostId = ActiveHostId ?? HostId` (`TranslationRoom`). `TransferHostAsync` (`TranslationRoomDirectoryService`, line 444): caller MUST be the current effective host; the new host MUST already be a participant; `ActiveHostId` is set to null when the new host is the original `HostId`, else to the new host; roles swap (old host becomes PARTICIPANT). Authority checks use `RoomHostAccess` (effective host, or workspace owner/admin).

### 3.4 Series (one room code per series)

`TranslationRoomSeriesService`: recurrence types DAILY, WEEKLY, MONTHLY; weekdays ISO 1-7. Limits (`RecurrenceLimits`): horizon 14 days, default duration 30 days, max duration 365 days, 32 occurrences per pass, 200 series per sweep.
- `CreateSeriesAsync` (L102) validates with `RecurrencePlanner.Plan`, materializes occurrences inside the horizon in one transaction; all occurrences reuse the first occurrence's code (`SharedRoomCode`); invitation emails go only for the first occurrence.
- Idempotency: unique index on (series_id, occurrence date) `translation_rooms_series_id_occurrence_date_key`.
- `CancelSeriesAsync` (L467): host only; sets CANCELLED then cancels future cancellable occurrences (optional `keepOccurrenceId`); already cancelled returns 0.
- `GetByCodeAsync`: shared code resolves to the live occurrence, else the next due, else the most recent.
- Series status: ACTIVE, CANCELLED, COMPLETED (COMPLETED when fully materialized).

### 3.5 Invitations

`TranslationRoomInvitation` (Email, Status PENDING/ACCEPTED/declined). Created at room creation with email and in-app notification; accepted via `invitations/accept` or automatically on join by matching email. Read access (`RoomReadAccess.IsReadableBy`): host, active host, participant, or invitee with PENDING/ACCEPTED.

### 3.6 Sessions

`TranslationRoomSession` statuses ACTIVE, PAUSED, ENDED. A new session starts on Resume under `AcquireSessionStartLockAsync`; Pause/Stop/End close it. Session endpoints are host-managed (`ErrorUnauthorizedManageSession`, Forbidden); a session not belonging to the room is `ValidationError`.

### 3.7 Audio routes (who hears which language)

`GenerateRoutesAsync` (`TranslationRoomAudioRouteService.cs:218`): for every ordered pair (speaker, listener) with different ids, source language = speaker `SpeakLanguage`, target language = listener `ListenLanguage`. If the languages are equal no route exists (any existing one is removed). Routes of departed participants are removed; new routes start PENDING. Preconditions: room policy complete (`ErrorRoomPolicyIncomplete`), at least one participant (`ErrorNoParticipantsInRoom`).
- Effective status precedence (`AudioRoutePriorityResolver`): TEXT_ONLY delivery gives CAPTION_ONLY; clone FALLBACK gives STANDARD_VOICE; TTS degraded gives VOICE_DELAYED; translation degraded gives TRANSLATION_DELAYED; STT degraded gives SPEECH_DELAYED; else BROADCASTING. Other statuses: PENDING, READY, PAUSED, ENDING, SAVING_OUTPUTS, COMPLETED.
- Voice clone: route seeded with clone only when the speaker has consent (`HasVoiceCloneConsentAsync`) and either Redis `translationRoom:{id}:clone_intent` or the user's voice preference says so. `ToggleVoiceCloneAsync`: caller MUST be the route's source participant (`ErrorNotRouteSpeaker`); enabling without consent is refused (`ErrorVoiceCloneConsentMissing`).
- Runtime updates: host only; COMPLETED route is InvalidState; BROADCASTING sets `StartedAt`. Participant on a host-only action gets Forbidden; outsider gets NotFound.
- Publication (`AudioRouteCacheService`): Redis key `translationRoom:{roomId}:audio_routes` (TTL 12 h; payload with generated_at, room_status, translation_active, room_languages) and pub/sub `translationRoom:{roomId}:events` with `{type:"AUDIO_ROUTES_UPDATED", data}`.
- Runtime events (`AudioRoutingEventType`) include config_ready, session_starts, translation_stopped, room_pause, room_resume, session_ends, stt/translation/tts latency_high and recovered, stt_unavailable, tts_unavailable, voice_clone_unavailable/recovered/ready, token_exhausted/recovered, participant_language_changed, audio_unavailable/recovered, finalization_failed, finalization_abandoned.

### 3.8 External bridge (Google Meet)

Fixed stand-in participant (`ExternalBridgeParticipantUserId`, display name "External Meeting"). One open bridge room per workspace and Meet code (partial unique index `translation_rooms_open_bridge_meet_code_key`; violation maps to Conflict "An open room for this Google Meet call already exists."). Capturer heartbeat every 15 s, considered stale after 45 s (`CapturerHeartbeatInterval`, `CapturerStaleAfter`); takeover endpoint reassigns the capturer. Audio modes voice and text; text-only MUST NOT return to voice while translation runs (`BRIDGE_AUDIO_MODE_LOCKED`). `MeetConferenceEndWorker` ends bridge rooms when the Meet conference ends.

### 3.9 Artifacts, consent, retention

- Types (`ArtifactType`): TRANSCRIPT_EXPORT, SUMMARY_EXPORT, DEBUG_LOG, OPTIONAL_RECORDING, AUDIO_SAMPLE. Status: Active, Expired, Completed, Processing, Failed. Fields include ContainsRawAudio/Video, ConsentRequired, RetentionUntil, ProviderArtifactId, RecordingStartedAt, FailureReason.
- Recording artifacts are created with `ConsentRequired = true` (`RecordingLifecycleEventProcessor:362`) and released when the host approves (`ApproveArtifactConsentAsync`: `CanReleaseArtifact` else Unauthorized; sets false) or by auto-share at end (`IsReleasedByAutoShareAsync`, setting `auto_share_record`). Download of a consent-required artifact is refused with "Consent is required before downloading this recording..." (`TranslationRoomArtifactService:829`).
- Artifact visibility follows room setting `artifact_access` (HOST_ONLY or ALL_PARTICIPANTS); only the host MAY change it.
- Transcript retention (`TranscriptRetention.IsSaved`): when `save_transcript` is false the meeting keeps no transcript, summary or minutes; summary requests fail with code `TRANSCRIPT_NOT_SAVED`.
- `RetentionUntil` is carried from the create-artifact request DTO (`ArtifactMapper:26`); no retention period constant or purge job was found in this service (gap).

### 3.10 Feedback

`SubmitFeedbackAsync` (Svc:4617): room MUST exist and be accessible (else NotFound); room status MUST be ENDED ("Feedback is only available after a room ends.", InvalidState); one feedback per user per room ("Feedback has already been submitted for this room.", InvalidState). Fields: OverallRating, TranslationQuality, AudioQuality, VoiceCloneQuality, AiSummaryQuality, Comments (trimmed, blank stored as null), CommunicationInsights (JSON). No numeric range validation was found in the service (gap; may be in the request validator).

### 3.11 Minutes and action items

`MeetingMinutes` has versions (number unique per version), secretary edit, sign, approve, revise, DOCX/PDF export (`MeetingMinutesDocxWriter`, `GotenbergPdfConverter`) and sharing by link token (anonymous read) or by named grants. `MeetingActionItem` can be created manually or requested from the assistant; status updatable per item. Detailed state rules were not read (gap).

## 4. Business rules enforced in code

| Rule | Where | Result |
|---|---|---|
| Room code is exactly 12 characters | join validation | ValidationError "Translation room code must be exactly 12 characters." |
| Missing languages default from user settings; source and target MUST be supported | `CreateTranslationRoomCoreAsync` | ValidationError |
| `workspaceId` required; creation permission, plan quota and language policy | `ValidateMeetingCreationAsync` (Workspace gRPC) | Forbidden / ValidationError |
| Room code unique among non-terminal rooms (regenerated until unique); series occurrences share one code | `RoomCodeGenerator`, migration 20260812090000 | |
| Existing open bridge room for same workspace and Meet code is returned instead of creating | create | idempotent |
| Capacity check for non-host, not already seated | `CountSeatsAgainstCapacityAsync` | Conflict "This room is full ({0} participants)..." |
| KICKED cannot rejoin | join | Forbidden "You have been permanently removed from this room and cannot rejoin." |
| REJECTED cannot rejoin | join | Forbidden "The host declined your request to join this meeting." |
| Workspace language allow-list narrows start languages | `ResolveEffectiveStartLanguagesAsync` | warning or `ErrorStartLanguagesNotAllowed` |
| Credits and subscription on Resume | `EnsureTranslationCreditsAsync` (Svc:2736) | see 5.4 |
| Idempotent start, open, expire, end | `TryTransitionStatusAsync(id, fromStatuses, to)` | concurrent callers succeed as no-ops |
| Meeting-started metric recorded once | Redis `StringSetIfAbsent` (`MeetingLifecycleKeys`) | |
| Recording artifact idempotent by provider artifact id | unique index (migration 20260727090000) | |

## 5. Integration

### 5.1 LiveKit

None in this service. Token and room creation, bridge tokens and webhooks belong to the Meeting service. This service consumes the Meeting service's `meeting.recording_completed` event (5.3) and exposes media usage over gRPC.

### 5.2 Redis pub/sub

| Channel | Direction | Content |
|---|---|---|
| `warptalk:translation-room:commands` | publish | `Command` field: RoomStarted (state: roomId, code, status, participants[userId, displayName, speakLanguage, listenLanguage, isMuted, joinedAt]), RoomEnded, CancelRoom, Kick, ParticipantAdmitted, ParticipantRejected, ParticipantWaiting, TranslationStopped, RoomLanguagesChanged |
| `warptalk:meetings:events` | publish | MeetingOpened (eventType, workspaceId, roomId, title, status), MeetingInvited |
| `translationRoom:{roomId}:events` | publish | AUDIO_ROUTES_UPDATED |
| `translationRoom:telemetry` | subscribe | STT/MT/TTS latency telemetry (`TelemetryRedisSubscriber`) |
| `translationRoom:participant-offline`, `translationRoom:participant-online` | subscribe | presence (`ParticipantOfflineConsumerWorker`) |

### 5.3 Redis streams

| Stream | Group | DLQ | Consumer | Behavior |
|---|---|---|---|---|
| `translationRoom:system_events` | `translationRoom_backend_consumer` | `translationRoom:system_events:dlq` | `TranslationRoomEventConsumerService` | leader-only; fields event_type, route_id, room_id, payload; feeds audio route event processors; retry then DLQ with original_message_id, error_message, failed_at |
| `meeting:domain-events` | `translation-room.recordings.v1` | `meeting:domain-events:translation-room:dlq` | `RecordingCompletedEventConsumerService` | recording artifact creation (3.9) |
| `workspace-events` | `translation-room-group` | none | `WorkspaceEventConsumerWorker` | handles `WorkspaceDeleted` (field workspace_id) and `MemberRemoved` (field user_id); effects publish gateway commands (details not read) |
| `assistant:summary_results` | `translation-room-summary-consumers` | none | `SummaryResultConsumerWorker` | stores summary variants |
| `assistant:summary_requests` | n/a (written) | | `ArtifactsFinalizer`, `TranslationRoomArtifactService` | request summary from AI assistant |
| `knowledge:fact_requests` | n/a (written) | | `RedisKnowledgeFactRequestPublisher` | consumed by the AI knowledge fact worker |

Summary state keys: `summary_rewrite:` (TTL 15 min), `summary_variant_inflight:` (TTL 5 min), `summary_variant_requeued:`.

### 5.4 Billing, Workspace, Transcript, AI

- Billing (indirect): on Resume, `HasActiveSubscriptionAsync(workspace) == false` refuses with the subscription_expired message; then Redis keys `translationRoom:{id}:ai_service_suspended` / `ai_service_state`, falling back to `workspace:{wid}:ai_service_suspended` / `ai_service_state`; any hit refuses with a reason-specific message (invoice_overdue, trial_ended, subscription_expired, overage_cap or insufficient_credits, default). Redis failure fails open. The writer of these keys is outside this slice.
- Workspace gRPC: `ValidateMeetingCreationAsync`, `EnsureWorkspaceCanHostMeetingsAsync`, `HasActiveSubscriptionAsync`, `IsMemberAsync`, `IsOwnerOrAdminAsync`.
- Other gRPC clients: user settings, voice consent, dub voice, admin audit, Meet conference records. Notification gRPC for invites, reminders and "summary ready". Transcript service is called for names and receives room data by gRPC (it calls this service back for host verification).
- Documents: object storage URLs signed by `S3ArtifactUrlSigner`.

## 6. Background jobs

| Worker | Interval | Behavior |
|---|---|---|
| `ScheduledRoomLifecycleWorker` | 1 min, lease 2 min | SCHEDULED to OPEN at slot time (WT-612); expires missed bookings after 2 h grace (WT-714) |
| `IdleRoomMonitoringWorker` | 1 min; idle timeout 5 min | WAITING/IN_PROGRESS rooms with nobody online; hub liveness plus Redis `translationRoom:{id}:idle_since`; ends as host |
| `AbandonedRoomSweepWorker` | 5 min, max 20 rooms | grace 20 min, lookback 7 days; decision Leave / StartGrace / End; ends as host |
| `MeetConferenceEndWorker` | 30 s, lease 2 min | ends bridge rooms when the Meet conference has ended |
| `ReminderNotificationWorker` | 1 min | MEETING_REMINDER at 30, 10, 1 minute; per-recipient marker TTL 1 h |
| `RecurringSeriesMaterializationWorker` | 15 min, lease 10 min | rolls the 14-day horizon |
| `ArtifactsFinalizationWorker` | drains in-memory queue | per-room lease 2 min; concurrency 4; notifies MEETING_SUMMARY_READY |
| `ArtifactsReconciliationWorker` | 5 min | ended rooms without artifacts; grace 10 min, lookback 7 days, max 20 |
| `ArtifactsRecoveryWorker` | 5 min | retries failed finalization; counter TTL 7 days |
| Consumers | continuous | section 5.3 |

Sweeps take a Redis lease (`LockTakeAsync`) so one replica runs each. The finalization queue is an in-memory `Channel`, which is why reconciliation and recovery exist.

## 7. Configuration knobs

| Key | Default |
|---|---|
| `Telemetry` (`TelemetrySettings`) | SttDegradedMs 3000, SttRecoveryMs 1500, TranslationDegradedMs 2500, TranslationRecoveryMs 1200, TtsDegradedMs 6000, TtsRecoveryMs 3000, WarmupCount 3 |
| `ArtifactFinalization` | MaxLocalRetries 3, MaxRecoverySweeps 5, StorageBaseUrl `<internal>`, transcript path `/workspace/rooms/{0}/transcript.md`, summary path `/workspace/rooms/{0}/summary.md`, recording path `/workspace/rooms/{0}/full_recording.wav` |
| `Jwt` | Issuer `WarpTalk.AuthService`, Audience `WarpTalk`, signing key `<secret>` |
| `ConnectionStrings:TranslationRoomDb`, `Redis:ConnectionString`, `Smtp:*`, `App:FrontendBaseUrl` | `<placeholder>` |
| `GrpcSettings` | Auth, Transcript, Notification, Workspace, Assistant endpoints, local-dev ports |
| Meeting create rate limit | platform setting `MeetingCreatePerMinute`, default 5 per workspace per minute (`RateLimitingFilter`, Redis `ratelimit:workspace:{id}:createmeeting`, TTL 1 min), overridable per workspace |
| Constants (not configurable) | worker intervals (section 6), recurrence limits (3.4), bridge timings (3.8), audio routes cache TTL 12 h |

## 8. Error codes and messages

Codes (`ErrorCodes`): NotFound, InvalidState, ValidationError, Forbidden, Unauthorized, Conflict, InternalServerError. Special codes: `TRANSCRIPT_NOT_SAVED`, `BRIDGE_AUDIO_MODE_LOCKED`. Notable messages:

| Code | Message |
|---|---|
| NotFound | "TranslationRoom not found" |
| InvalidState | "This room has already ended or has been cancelled." |
| InvalidState | "Room must be SCHEDULED to open waiting room." |
| InvalidState | "Only scheduled, open or waiting rooms can be started." |
| InvalidState | "Room settings cannot be updated after the room has entered IN_PROGRESS status." |
| Unauthorized | "Unauthorized. Only host can end translationRoom." |
| Forbidden | "Cannot kick the host." is returned for kick of host; KICKED/REJECTED join messages in 3.2 |
| Conflict | room full; open bridge room exists (section 4) |
| InvalidState | "Feedback is only available after a room ends." / "Feedback has already been submitted for this room." |
| Unauthorized | `ErrorUnauthorizedConsentArtifact` on consent release by a non-releaser |
| InternalServerError | `ErrorUnexpected` (all services wrap exceptions) |

## 9. Database migrations (`translation-room/database/migrations`, 24 files; key ones)

| Date | Effect |
|---|---|
| 20260727090000 | unique index on artifact `provider_artifact_id` (recording idempotency) |
| 20260730141000 | seeds supported languages |
| 20260806120000 | adds series table, series columns, unique (series_id, occurrence date) (WT-327) |
| 20260807090000 | adds `reminder_30min_sent_at` |
| 20260812090000 | replaces global unique room code with partial unique index for one-off rooms (one code per series) |
| 20260812160000 | adds `active_host_id` |
| 20260817060000 | backfill: ends rooms held open by a phantom host seat (WT-450) |
| 20260817070000 | adds participant `is_external` |
| 20260817150000 | adds artifact `recording_started_at` (WT-473) |
| 20260820120000 / 20260820160000 | minutes tables / action items table |
| 20260905090000 | external meeting metadata columns |
| 20260908170000 | grandfathers shared records so ended meetings stay readable (WT-653) |
| 20260909060000 | minutes share links and grants |
| 20260910090000 | summary language variants |
| 20260923090000 | action items from assistant; artifact `failure_reason` (WT-824) |
| 20261001090000 | unique open bridge room per Meet code |
| 20261001150000 | adds participant `is_bridge_text_only` |

## 10. Differences vs Report 7 (06-service-design.md section 3, 01 UC-12/30/31/32/34)

1. Transport: Report 7 describes Redis streams for the AI pipeline; the code also uses a Redis commands channel, pub/sub and several consumer groups, and no RabbitMQ/MassTransit. Report 7 event names (`RoomInvitedEvent`) were not found; invitations publish `MeetingInvited` on `warptalk:meetings:events` plus email/notification.
2. Stop route: Report 7 gives `POST .../stop-translation`; code is `POST {id}/translation/stop`.
3. Report 7 lists `UpdateTranslationRoom`, `SetVoiceCloneConsent` on rooms controller; code has `PUT {id}/settings` and voice-clone consent under audio-routes (`voice-clone-consent`).
4. Room states: Report 7 and UC-31 omit OPEN and FAILED, which exist in code. OPEN is a scheduled-opened state that auto-starts on first CONNECTED join.
5. UC-31 says Start from SCHEDULED/WAITING and "Start/resume translation" creates the session; code agrees (session on Resume) but Start also accepts OPEN, and non-hosts MAY start when approval is not required. Report 7 says start is host only.
6. Credit and subscription check happens on Resume only, not Start; Report 7 does not state this.
7. Participant statuses: Report 7 lists WAITING, CONNECTED, LEFT (+KICKED); code adds INVITED, DISCONNECTED, REJECTED. UC-30 says host starts INVITED; code creates the host participant at create and CONNECTED on join.
8. Participant roles: code has only PARTICIPANT and HOST; effective host via `ActiveHostId` and workspace owner/admin authority, not in Report 7.
9. Audio-route statuses: Report 7 lists READY, BROADCASTING, COMPLETED; code has 12 statuses including degraded and caption-only states. Route lifecycle rules in 3.7 are not in Report 7.
10. Idle auto-end (BR-92) is implemented as 5 min idle plus a separate 20 min abandoned-room sweep; scheduled expiry is 2 h grace. Matches UC-32 except the extra sweep.
11. Series: Report 7 lists UpdateSeries and recurrence rule; code adds one shared room code per series, 14-day horizon, MONTHLY/WEEKLY/DAILY, and cancel endpoint.
12. Report 7 lacks: minutes, action items, summary variants and rewrite, share links, feedback, bridge capturer endpoints, admin controllers, reminder and reconciliation workers, per-room-type defaults.
13. LiveKit is not in this service (consistent with Report 7 3.1); recording artifact fields `ConsentRequired`, `RecordingStartedAt`, `FailureReason` extend Report 7 3.8.

## 11. Gaps (not verified)

Admin controller permissions except meetings; `CanReleaseArtifact` details; retention period and purge; feedback value ranges; minutes and action-item state rules; `WorkspaceEventConsumerWorker` effects; writer of the billing suspension keys; `MediaUsageCalculator` rules; Meeting service gRPC implementation (`meeting/`) and `.agents/resources` design diagrams were not read.
