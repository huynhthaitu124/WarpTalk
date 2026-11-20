# 06 - Service Design

Source: Report 7, SDD IV.3 (Detailed Design). The report's class specifications are largely boilerplate and its sequence diagrams are images; this file keeps what an implementer needs: responsibilities, endpoints/methods, DTO fields, events and states. Behavioral rules live in 01 and 03. Every service follows Clean Architecture: Controller -> Service -> Repository/UnitOfWork -> DbContext (Domain, Application, Infrastructure, API layers). Names are logical (as in the report); route paths are given only where the report states them.

## 1. Auth Service

### 1.1 Responsibilities
Account lifecycle (register, login, Google login, logout, token refresh), profile and settings, voice enrollment and biometric consent, voice carry-over from meetings, and gRPC lookups for other services.

### 1.2 API surface
| Controller | Operations | Key data |
|---|---|---|
| AuthController | Register, Login, Logout | AuthResponseDto: UserId, Email, AccessToken, RefreshToken |
| GoogleAuthController | GoogleLogin(IdToken) | Backend MUST verify the Google ID token (GoogleOAuthService.ValidateIdTokenAsync) before create/link/authenticate |
| TokenController | RefreshToken | Rotating refresh tokens with family reuse detection |
| ProfileController | GetProfile, UpdateProfile | FullName, AvatarUrl, PreferredLanguage |
| UserSettingsController | GetSettings, UpdateSettings | DefaultAudioInput, DefaultAudioOutput, AutoTranslateEnabled |
| VoiceProfilesController | EnrollVoiceProfile(AudioBase64, PassphraseSample), GetVoiceProfileStatus | VoiceProfileDto: ProfileId, UserId, IsEnrolled, QualityScore |
| VoiceConsentController | GrantBiometricConsent, RevokeBiometricConsent | VoiceConsent: Id, UserId, IsGranted, GrantedAt |

Services: IAuthService (RegisterAsync, AuthenticateAsync, AuthenticateGoogleAsync); IJwtTokenProvider.GenerateTokens; VoiceCloneAiService.ExtractVoiceEmbeddingAsync (provider call that creates the voice from the sample).
Entities: User (Id, Email, PasswordHash, FullName), RefreshToken (Id, UserId, Token, ExpiresAt), VoiceProfile, VoiceConsent.

### 1.3 Voice enrollment and consent
- Enrollment MUST require an active biometric consent (repository GetActiveConsentAsync); without it the call is rejected (BR-94).
- Revoking consent MUST disable cloning for the user; provider voice and stored samples SHOULD be deleted per policy.

### 1.4 Voice carry-over (asynchronous)
`VoiceCarryOverConsumerWorker` reads a Redis queue (ReadNextAsync, AckAsync). Message VoiceCarryOverMessage: UserId, ProviderVoiceId, AudioKey, CapturedAt.
1. Check HasActiveConsentAsync for the user. No consent: drop the message and delete the audio sample.
2. VoiceCarryOverService.ApplyAsync creates a VoiceProfile (Source = Enrollment or InMeeting, Provider, ProviderVoiceId).
3. CartesiaVoiceStorage.DeleteAudioSampleAsync removes the temporary sample from storage.

### 1.5 gRPC exposed to other services
GetUserSettings, GetUserByEmail, GetUserById / role lookup, voice-clone consent lookup (HasVoiceCloneConsent).

## 2. Workspace Service

### 2.1 Responsibilities
Workspaces (tenant), settings, members and roles, invitations and join requests, verified domains, knowledge documents (upload, security scan, embedding indexing), platform-admin workspace governance, admin audit log, ownership transfer, and meeting-policy gRPC for other services.

### 2.2 API surface
| Controller | Operations | Key data |
|---|---|---|
| WorkspacesController | CreateWorkspace(Name, Description), GetWorkspaces(Page, PageSize, SearchTerm), GetWorkspaceById, SelectWorkspace, DeleteWorkspace | Soft delete. The active workspace is cached per user in Redis (IWorkspaceCacheService Set/Get/ClearActiveWorkspaceId) |
| Workspace settings | GetWorkspaceSettings, UpdateWorkspaceSettings | MaxParticipantsLimit, AllowGuestAccess, DefaultAiModel, AudioRetentionDays |
| WorkspaceInvitationsController | InviteMember(Email, Role), GetInvitationPolicy, RetryDelivery, ListInvitations, RevokeInvitation | WorkspaceInvitation: Id, WorkspaceId, Email, Role, Token, Status, ExpiresAt, ProviderMessageId |
| Invitation acceptance | GetPendingInvitations, PreviewInvitation, AcceptInvitation(Token), AcceptInvitationById | InvitationPreviewDto: WorkspaceId, WorkspaceName, InviterEmail, Role |
| WorkspaceMembersController | RemoveMember, Leave, TransferOwnership(NewOwnerId) | Removal is soft (RemovedAt) with role-hierarchy checks; leave also clears the active workspace in Redis; ownership transfer is Owner-only |
| WorkspaceDocumentsController | Upload, List, GetById, Delete | Upload: File, Title, Category (GENERAL, TECHNICAL, FINANCIAL, LEGAL, MEETING_NOTES), AccessLevel (PUBLIC, MEMBERS_ONLY, ADMIN_ONLY) |
| Verified domains | AddDomain(Domain, ConsentVersion), ListDomains, RevokeDomain | VerifiedDomainDto includes VerificationMethod; uses IAuthIdentityClient (GetRoleNameByIdAsync, GetUserByIdAsync) |
| AdminWorkspacesController | GetDirectory, GetDetail, Suspend(Reason), Reactivate(Reason), Delete | AdminWorkspaceDto: WorkspaceId, Name, Status, TotalMembersCount; WorkspaceAdminAction: Id, TargetWorkspaceId, ActionType, Reason |
| AdminAuditLogController | Query(ServiceName, Action, StartDate, EndDate) | AdminAuditLogEntry: Id, ServiceName, Action, AdminUserId |

### 2.3 Invitation delivery
Workspace service -> NotificationGrpcClient.SendEmailInvitationAsync -> Notification gRPC server -> Resend; the provider message ID is stored on the invitation. Failed delivery MUST be retryable (RetryDelivery). Tokens MUST be stored hashed and expire (see BR in 03).

### 2.4 Document ingestion pipeline
1. Upload validates type/size, stores the file in encrypted object storage (S3-compatible, `S3EncryptedWorkspaceDocumentStorage`) and creates a metadata row.
2. A document-uploaded event goes to Redis stream `embedding:index_requests` (RedisEmbeddingIndexPublisher), carrying the ai-retrieval-allowed flag.
3. Security worker runs PII/DLP scanning first (SecurityWorker.ScanDocumentPiiDlpAsync via DocumentSecurityGuardrailConsumerService); blocked content is not indexed.
4. Embedding worker chunks and upserts vectors into Qdrant (EmbeddingWorker.ProcessChunkAndIndexAsync, UpsertChunksAsync).
5. Result is published on `embedding:index_results`; DocumentEmbeddingIndexResultConsumerService updates the document status (indexed, blocked, failed, deleted).

### 2.5 gRPC exposed
ValidateMeetingCreation, EnsureWorkspaceCanHostMeetings, GetWorkspacePreflightDetails (plan limits, suspension, member policy) used by Translation Room before create/start.

## 3. Translation Room Service

### 3.1 Responsibilities
Business-level meeting lifecycle (create, schedule, recurring series, invite, join/lobby, start, stop, end), participant roster, per-listener audio routes and voice-clone toggles, realtime translation orchestration over SignalR and Redis Streams, and recording-artifact metadata. The RTC runtime (LiveKit) belongs to the Meeting service.

### 3.2 API surface
| Controller / hub | Operations | Key data |
|---|---|---|
| TranslationRoomsController | CreateTranslationRoom, UpdateTranslationRoom, StartTranslationRoom, StartRoom, EndRoom, JoinTranslationRoom (by code), JoinTranslationRoomById, StopTranslation, SetVoiceCloneConsent | Create: Title, WorkspaceId, ScheduledAt, SourceLanguage -> RoomDto: RoomId, RoomCode, Title, Status. Stop: `POST /api/v1/translation-rooms/{id}/stop-translation` |
| Join | JoinTranslationRoomRequest: TranslationRoomCode, DisplayName, SpeakLanguage, ListenLanguage | Response: RoomId, RoomCode, ParticipantId, Role |
| TranslationRoomParticipantsController | GetParticipants, AdmitParticipant(roomId, participantId), KickParticipant, LeaveRoom, JoinRoom, UpdateParticipantSettings | ParticipantDto: ParticipantId, RoomId, UserId, DisplayName |
| TranslationRoomAudioRouteController | GenerateRoutes, ResolveAudioRoutes, SetVoiceCloneConsent | AudioRouteDto: SourceParticipantId, TargetParticipantId, SourceLanguage, TargetLanguage, VoiceCloneEnabled |
| TranslationRoomSeriesController | GetSeries, UpdateSeries (create via CreateTranslationRoom with recurrence) | SeriesDto: SeriesId, HostId, RecurrencePattern, TotalOccurrencesCount; CreateSeriesResponseDto: SeriesId, RecurrenceRule, FirstOccurrence |
| TranslationRoomHub (SignalR) | SendAudioChunk, BroadcastLiveEvents | Realtime audio in; captions, translations, room events out |

Update request (invitations and settings): UpdateRoomSettingsRequest = Title, Description, InvitedEmails, Settings; publishes RoomInvitedEvent (RoomId, RoomCode, HostName, InvitedEmails) which drives invitation emails.

### 3.3 Create and start
- Create: fetch user language defaults from Auth over gRPC (DefaultSourceLanguage, DefaultTargetLanguages); validate plan/policy through Workspace gRPC `ValidateMeetingCreation`; persist TranslationRoom (Id, WorkspaceId, HostId, Title) and invitations; send invitation emails (Resend adapter).
- Start: permission resolved by ResolveStartPermissionAsync (host only); preflight via `EnsureWorkspaceCanHostMeetings`; generate routes (TranslationRoomAudioRouteService.GenerateRoutesAsync -> AudioRouteMeshDto: RoomId, ActiveRoutesCount); notify invitees through Notification gRPC; publish state through RedisStateRepository.PublishAsync.
- Recurring series: CreateSeriesAsync stores TranslationRoomSeries (Id, HostId, RecurrenceRule, StartDate) and `MaterializeOccurrencesInsideHorizon` creates TranslationRoom occurrences (SeriesId, Title, ScheduledStartTime) inside a rolling horizon.

### 3.4 Join, lobby and participants
- Join validates room code/state, upserts the participant (GetByRoomAndUserIdAsync), stores speak/listen languages, and calls ReconcileRoutesAsync so audio routes match the new language mix.
- Rooms requiring approval place the participant in WAITING; AdmitParticipant (host/admin only) sets CONNECTED and publishes the admit command to live clients; Kick/Leave set KICKED/LEFT and reconcile routes.
- Participant status enum: WAITING, CONNECTED, LEFT (KICKED is also used by the product flows).

### 3.5 Realtime translation pipeline
1. Client sends audio chunks via `TranslationRoomHub.SendAudioChunk`; RealtimeTranslationService.ProcessAudioChunkAsync resolves routes and RedisStreamPublisher.PublishAudioChunkAsync writes to the audio stream.
2. AI workers consume streams: SttWorker.ProcessSpeechToTextAsync -> TranslationWorker.TranslateTextAsync -> TtsWorker.SynthesizeAudioAsync (or TtsVoiceCloneWorker.SynthesizeClonedAudioAsync when the listener's route has voice cloning enabled).
3. AiResultConsumerService consumes results: ConsumeSttResultsAsync, ConsumeTranslationResultsAsync, ConsumeTtsResultsAsync, ConsumeTtsClonedAudioResultsAsync (TtsClonedAudioDto: RoomId, TargetParticipantId, AudioPayload, IsVoiceCloned).
4. Captions/translations are pushed to clients via SignalR; segments are persisted to the Transcript service through TranscriptGrpcService.SaveSegmentAsync.
DTOs: SegmentDto (RoomId, SpeakerId, Text, AudioBase64), TranslationResultDto (RoomId, OriginalText, TranslatedText, SourceLanguage), TtsResultDto (RoomId, AudioPayload, TargetLanguage, TargetParticipantId).
Entities: TranslationRoomSession (Id, TranslationRoomId, MainLanguage, Status, StartedAt, EndedAt), TranslationRoomAudioRoute (Id, TranslationRoomId, SourceParticipantId, TargetParticipantId, IsVoiceCloneEnabled).

### 3.6 Voice-clone toggle
SetVoiceCloneConsent(Enabled) updates route consent (UpdateAudioRouteConsent). The service MUST check the speaker's consent via VoiceConsentGrpcDirectory.HasVoiceCloneConsentAsync (Auth gRPC); without consent the route stays on standard TTS. Participant flag: IsUsingVoiceClone. Failure of cloned synthesis falls back to standard TTS (NFR-RL-03).

### 3.7 Stop translation and end room
StopTranslation(id, hostId) ends the active TranslationRoomSession (Status ENDED, EndedAt), tears down routes (AudioRouteEventProcessor, event `session_ends`) and publishes `AUDIO_ROUTES_UPDATED` on Redis Pub/Sub. The room itself remains active so the meeting can continue without translation; EndRoom ends the room.

### 3.8 Recording artifacts
RecordingCompletedEventConsumerService consumes `meeting.recording_completed` from the Meeting domain-events stream (message: TranslationRoomId, FileUrl, EgressId, DurationSeconds). The processor MUST be idempotent: skip when an artifact with the same ProviderArtifactId (EgressId) exists, else insert TranslationRoomArtifact (Id, TranslationRoomId, ArtifactType, FileUrl, ProviderArtifactId, DurationSeconds, CreatedAt). Recording files live in object storage; LiveKit Egress produces them.

### 3.9 External Bridge (Google Meet) on desktop
The desktop client uses a VirtualAudioDriverAdapter (WriteToVirtualMic, ReadFromVirtualSpeaker, CheckDriverStatus), DesktopAudioRuntime (detect physical mic/headphones), LocalStateStore (SaveSessionSettings, GetDriverCapability), PopupWidget (StartSession, StopSession, RenderSubtitles) and WebAudioEngine (QueueAudioBuffer, ResampleAudio). Capability model: DesktopRuntimeCapability (DeviceId, DeviceName, SupportsVirtualDriver, IsDriverInstalled), VirtualAudioStatus (IsActive, ActiveChannelCount, LatencyMs). Without a driver the bridge degrades to captions only (LI-8). Audio routes for the bridge use SpeakerUserId/ListenerUserId and IsVoiceCloneEnabled.

### 3.10 State machines
- Room: SCHEDULED -> WAITING -> IN_PROGRESS <-> PAUSED -> ENDED; SCHEDULED/WAITING -> CANCELLED or EXPIRED. "DRAFT" is not persisted.
- Participant: WAITING -> CONNECTED -> LEFT (or KICKED).
- Audio route: READY -> BROADCASTING -> COMPLETED.
- Session: ACTIVE -> ENDED.

## 4. Meeting Service

### 4.1 Responsibilities
RTC runtime bound to a Translation Room: LiveKit room creation and join tokens, bridge tokens for external platforms, LiveKit webhooks, in-meeting chat (with translation and WarpBot), meeting history, and ending a meeting (LiveKit teardown and billing stop).

### 4.2 API surface
| Controller / hub | Operations | Key data |
|---|---|---|
| MeetingsController | JoinMeeting, GenerateBridgeToken, EndMeeting(translationRoomId) | JoinMeetingResponseDto: RoomName, ServerUrl, Token, ParticipantIdentity; BridgeTokenResponseDto: ServerUrl, BridgeToken (publish-only); EndMeetingResponseDto: MeetingRoomId, IsFinished, EndedAt |
| LiveKitWebhooksController | ReceiveWebhook | Room, participant, track and egress events (signature MUST be validated) |
| MeetingChatController | GetChatHistory | ChatMessageDto: MessageId, SenderId, SenderName, Content |
| MeetingHistoryController | GetMeetingHistory(PageIndex, PageSize, SearchQuery), GetMeetingRoomDetail | MeetingHistoryDto: MeetingId, RoomTitle, StartedAt, EndedAt; MeetingDetailDto adds DurationSeconds, Participants |
| SignalR hub | SendChatMessage, BroadcastChatMessage, BroadcastAiSuggestion | Live chat and AI suggestions |

### 4.3 Join and token flow
JoinMeetingAsync loads the active MeetingRoom for the Translation Room (GetActiveMeetingByRoomIdAsync), creates the LiveKit room if needed (CreateRoomAsync), and ILiveKitTokenProvider.GenerateToken issues a signed room-scoped token with a participant identity. Tokens MUST be short-lived and scoped to one room. Entities: MeetingRoom (Id, TranslationRoomId, RoomName, Status, StartedAt, EndedAt), MeetingParticipant (Id, MeetingSessionId, UserId, Identity, JoinedAt).

### 4.4 Chat
- SaveMessageAsync persists the message, broadcasts via SignalR and publishes it for translation (RedisStreamPublisher.PublishChatForTranslationAsync); TranslationWorker.TranslateChatTextAsync returns per-language text (TranslatedChatDto: MessageId, OriginalText, TranslatedText, TargetLanguage) consumed by AiResultConsumerService.ConsumeChatTranslationResultsAsync.
- WarpBot prompts in chat go to the assistant stream (PublishAiAssistantPromptAsync, AssistantWorker.ProcessLlmPromptAsync); the answer is stored with SaveAssistantResponseAsync (WarpBotResponseDto: MessageId, SenderName, ResponseText, SentAt).
- SuggestionWorker.DetectActionItemsAsync emits AiSuggestionDto (SuggestionId, SuggestionType, Content) via ConsumeSuggestionResultsAsync.
- Moderation hides messages without erasing them (BR-54). Entities: MeetingChatMessage, MeetingChatSession (TotalMessagesCount).

### 4.5 End meeting
EndMeetingAsync(translationRoomId, hostUserId): host only; set MeetingRoom status FINISHED and EndedAt; publish `meeting.end_room` and `meeting.billing.stop` on Redis (RedisEventPublisher); delete the LiveKit room (DeleteRoom), which disconnects all participants. Downstream: billing stops metering; transcript finalization and AI post-meeting jobs start.

### 4.6 Events published
`meeting.end_room`, `meeting.billing.stop`, `meeting.recording_completed` (from LiveKit Egress completion; consumed by Translation Room).

## 5. Transcript Service

### 5.1 Responsibilities
Persist speaker-attributed segments, transcript versions and AI summaries, corrections, workspace and global glossaries, exports, and finalization.

### 5.2 API surface
| Controller | Operations | Key data |
|---|---|---|
| TranscriptsController | GetTranscript, FinalizeTranscript (`POST /api/v1/transcripts/{id}/finalize`) | TranscriptDto: TranscriptId, RoomId, FullText, AiSummary |
| TranscriptExportsController | ExportTranscript | ExportTranscriptDto: Format (PDF, DOCX, TXT, VTT, SRT), IncludeAudio |
| TranscriptCorrectionsController | SubmitCorrection, GetCorrections | CreateCorrectionDto: Text, Reason; TranscriptCorrectionDto: Id, SegmentId, UserId, Text |
| GlossariesController | GetPublishedGlobalTerms, CreateGlossary, GetGlossary, GetGlossariesByWorkspace, Update, Delete | CreateGlossaryDto: WorkspaceId, Name, SourceLanguage, TargetLanguage; GlossaryDto adds TotalTermsCount; GlossaryTerm: SourceTerm, TargetTerm |
| GlobalGlossariesController | GetTerms, GetTerm, CreateTerm (System Admin; publish/archive) | CreateGlobalGlossaryTermDto: Key, DomainCategory, TargetLanguage, TranslatedTerm; query: Page, PageSize, Status; every change writes a GlobalGlossaryAudit (Action, ActorId) |
| gRPC (inbound) | SaveSegment from Translation Room | Segments are stored as they arrive |

### 5.3 Behaviors
- Finalize: caller MUST be the room Host, verified by gRPC `GetTranslationRoomById` to Translation Room (TranslationRoomGrpcClient). Status moves FINALIZING -> FINALIZED; later ARCHIVED. Entity Transcript: Id, TranslationRoomId, MainLanguage, AudioUrl, Status, IsActive, FinalizedAt.
- Corrections are stored as separate records and never overwrite history (NFR-RL-05); the segment keeps current text, corrections keep original, corrected text, author and review status.
- Export (GenerateArtifactFileAsync) renders from segments plus current translations; files go to object storage and an export log row is written.
- Global glossary terms are draft/published/archived; workspace glossaries take precedence over global ones during translation (see 03 for BRs).
- Transcript state machine: (live) -> FINALIZING -> FINALIZED -> ARCHIVED.

## 6. Assistant Service (WarpBot)

### 6.1 Responsibilities
Conversational assistant for meetings and workspaces: LLM chat with conversation history, and workspace knowledge retrieval (RAG) constrained by the caller's access.

### 6.2 API surface
| Controller | Operations | Key data |
|---|---|---|
| AssistantConversationsController | SendMessage, GetConversationHistory | SendMessageDto: RoomId, Prompt; AssistantMessageDto: MessageId, ConversationId, Sender, Content |
| AssistantKnowledgeController | GetKnowledge(workspaceId, query) | GetKnowledgeQuery: Search, SourceType, Page, PageSize -> KnowledgeRetrievalResultDto: Items, TotalCount, NextCursor |

### 6.3 Behaviors
- ProcessUserPromptAsync: find or create the active AssistantConversation (Id, RoomId, UserId, CreatedAt), append the user AssistantMessage (SenderRole, Content), call LlmProviderService.GenerateCompletionAsync, store and return the answer. Tool calls are logged in assistant_tool_calls. Write actions on external data MUST require explicit user confirmation (LI-14).
- GetKnowledgeAsync(workspaceId, query, userId): (1) obtain WorkspaceContextDto (WorkspaceId, MemberId, RoleName, AllowedDocumentIds) over gRPC from Workspace (`GetWorkspaceContext`); (2) query Qdrant collection `workspace_{workspaceId}` with filters (workspace, ai_retrieval=true, AllowedDocumentIds, SourceType) via QdrantKnowledgeRetriever; (3) map points to KnowledgeChunkDto (ChunkId, SourceType, Title, FactText, Score).
- KnowledgeSourceType: Document, MeetingSummary, Glossary, WorkspaceContext. FactCategory: Decision, Requirement, Definition, Commitment, Risk, Reference.
- Retrieval MUST be scoped by membership and document access; instructions inside retrieved content are never user authorization.

## 7. Billing Service

### 7.1 Responsibilities
Plan catalog, workspace subscriptions, Stripe checkout and webhooks, credit wallet (top-up, deduction, adjustment), usage metering, invoices, and platform-admin analytics.

### 7.2 API surface
| Controller | Operations | Key data |
|---|---|---|
| PlansController | GetPlans (active), GetAllPlans (admin), GetPlanById, CreatePlan | CreatePlanDto/PlanDto: Name, Description, PriceMonthly, TranslationMinutesQuota (the report names minutes quota; credits are the implemented unit) |
| SubscriptionsController | GetCurrentSubscription, CancelSubscription | SubscriptionDto: SubscriptionId, WorkspaceId, PlanName, MonthlyMinutesQuota; one active subscription per workspace (BR-65) |
| PaymentsController | CheckoutSubscription (PurchaseSubscriptionRequest: WorkspaceId, PlanId, PaymentProvider), CheckoutCreditTopUp (TopUpCreditRequest: WorkspaceId, CreditAmount), HandleWebhook | Stripe Checkout session creation; webhook verifies signature |
| CreditsController | GetWorkspaceCredits, AdjustWorkspaceCredits, GetCreditHistory | CreditBalanceDto: WorkspaceId, BalanceMinutes, LastTopUpAt; CreditTransactionDto: TransactionId, Amount, TransactionType, CreatedAt |
| UsagesController / InvoicesController | GetCurrentUsage, ListInvoices, DownloadInvoice | CurrentUsageDto: WorkspaceId, TokensConsumed, AudioMinutesConsumed, TotalCostUSD; InvoiceDto: Id, InvoiceNumber, TotalAmountUSD, IssuedAt |
| AdminWorkspaceAnalyticsController | GetAnalytics, GetCreditTransactions | WorkspaceAnalyticsDto: WorkspaceId, TotalCreditsPurchased, TotalCreditsConsumed, ActiveSubscriptionTier |

### 7.3 Flows
- Subscribe: create checkout session (CreateSubscriptionCheckoutSessionAsync) and a pending Payment (Id, WorkspaceId, SessionId, Amount, Currency, Status); on Stripe webhook (ProcessSubscriptionWebhookAsync, lookup by SessionId) mark paid and ActivateWorkspaceSubscriptionAsync. State changes MUST happen only after webhook or verified session (NFR-IF-07). Webhook handling MUST be idempotent.
- Top-up: CreateCreditTopUpCheckoutSessionAsync; on confirmation AddCreditsAsync writes a CreditTransaction (AmountMinutes, TransactionType) and updates the balance. Manual AdjustWorkspaceCredits is admin-only and audited.
- Metering: billing worker consumes `translate:results` and `tts:results` (and meeting stop events) from Redis; every deduction carries an idempotency key; usage is per-second with a one-credit minimum and an overage cap (see 03 and 99).
- Plans gate features and concurrent rooms; entitlement snapshots are pushed to the Workspace service.
- Invoices derive from payments; analytics aggregate usage_records and credit_transactions (AmountPaidUSD, CreditAmountAdded).

## 8. Notification Service

### 8.1 Responsibilities
In-app notifications, email and push delivery, per-user preferences, admin announcements, and templates. Also a gRPC server used by other services (SendNotification, SendEmailInvitation, SendMeetingInvitation).

### 8.2 API surface
| Controller | Operations | Key data |
|---|---|---|
| NotificationsController | GetNotifications(page, pageSize), MarkAsRead, MarkAllAsRead | NotificationDto: Id, UserId, Title, Content; NotificationMessage: Id, UserId, Title, Content, Type, IsRead, CreatedAt (queries and updates scoped to the owning user) |
| AdminNotificationSettingsController | GetNotificationTemplates, UpdateNotificationTemplate | UpdateNotificationTemplateRequest/NotificationTemplateDto: Type, Channel, Subject, BodyTemplate, Variables, IsActive; ValidateTemplate checks content and channel/type rules |

### 8.3 Broadcast system notification (Platform Admin)
| Item | Specification |
|---|---|
| API | NotificationsController: CreateAdminNotification, GetAdminNotifications, GetAdminNotificationDetail (System Admin only) |
| Input | CreateAdminNotificationDto: Title, Content, TargetRole, NotificationType (the tested build supports a SpecificUsers audience mode) |
| Output | AdminNotificationDto: Id, AdminUserId, Title, Content |
| Behavior | AdminNotificationService persists an AdminNotification (Id, AdminUserId, Title, Content) through the repository and unit of work, creates per-user notification messages for the audience, and pushes them live through the SignalR hub (BroadcastNotificationAsync). |

### 8.4 Template configuration
UpdateNotificationTemplate edits a NotificationTemplate (Id, Type, Channel, Subject, BodyTemplate, Variables, IsActive, UpdatedAt). IsActive controls whether a template is usable by notification workflows. Templates MUST be validated for channel and type rules before save.

### 8.5 Delivery rules
- Per-user preferences (Email, Push, In-app; default all enabled) are checked before each channel send.
- Email goes through Resend/SMTP; push through FCM / Web Push subscriptions; in-app via persisted notification_messages plus SignalR.
- Notification reads and updates MUST be scoped to the owning user.

## 9. Cross-service state machines (summary)
| Machine | States |
|---|---|
| Room lifecycle | DRAFT -> SCHEDULED or WAITING -> IN_PROGRESS <-> PAUSED -> ENDED; CANCELLED (from SCHEDULED or WAITING); EXPIRED (start time missed, waiting timeout) |
| Participant | INVITED -> WAITING -> CONNECTED <-> DISCONNECTED; MUTED (toggle from CONNECTED); terminal LEFT, REJECTED, KICKED (reconnect timeout leads to LEFT) |
| Runtime audio routing | IDLE -> ROUTING_READY -> AUDIO_ROUTING_ACTIVE -> STOPPING -> FINALIZING_ARTIFACTS -> COMPLETED; degraded branches TRANSLATION_DEGRADED (STT or translation latency high), VOICE_OUTPUT_DEGRADED (TTS or cloning unavailable), TEXT_ONLY_MODE (audio output unavailable); each recovers back to active routing |
| Transcript | live -> FINALIZING -> FINALIZED -> ARCHIVED |


