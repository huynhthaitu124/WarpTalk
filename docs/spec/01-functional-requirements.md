# 01 - Functional Requirements

Source: Report 7, Section III (SRS) 2.2, 3.x. Business rules (BR-xx) are in 03-business-rules-and-messages.md. Requirement keywords MUST/SHOULD follow RFC 2119.

## 1. Use case catalog (UC-01..UC-77)

| Group | UC IDs and names | Actor(s) |
|---|---|---|
| Public / auth | UC-01 View Landing Page; UC-02 Register Account; UC-03 Log In; UC-04 Reset Password; UC-05 Log Out | Unauthenticated / Authenticated User |
| Profile / personal | UC-06 Manage Profile; UC-07 Configure Preferences; UC-08 Purchase Subscription Plan; UC-09 Join Workspace | Authenticated User |
| Member | UC-10 View Members; UC-11 Upload Document Library; UC-12 Join Meeting; UC-13 View Transcripts; UC-14 View Meeting History; UC-15 View Meeting Artifacts; UC-16 Chat with AI Assistance; UC-17 Upload Voice Profile | Workspace Member |
| Participant | UC-18 Set Up Device; UC-19 Select Speak/Listen Language; UC-20 Use AI Assistance; UC-21 Use Real-time Translation; UC-22 View Real-time Transcript; UC-23 Open Subtitle; UC-24 Use Voice Cloning; UC-25 Open Translated Audio; UC-26 View Personal Meeting History; UC-27 Submit Room Feedback; UC-28 Chat In Meeting | Participant |
| Host | UC-29 Schedule Meeting; UC-30 Create Meeting; UC-31 Start Live Translation; UC-32 End Meeting; UC-33 Edit Transcript; UC-34 Configure Meeting Language; UC-35 Manage Participants (admit/kick/mute/lock); UC-36 Manage Meeting Minutes (draft, review, export DOCX/PDF) | Host |
| Workspace Owner | UC-37 Create Workspace; UC-38 Deactivate Workspace | Owner |
| Owner / Admin | UC-39 Manage Members; UC-40 Invite Internal Members; UC-41 Invite External Members; UC-42 Manage Document Library; UC-43 Top Up Credits; UC-44 Configure Workspace Policies; UC-45 Manage Workspace Glossary Terms; UC-46 Configure Verified Domain; UC-47 Configure External Meeting Platform Settings; UC-48 View Workspace Dashboard; UC-49 Manage Workspace Billing; UC-50 Monitor AI Token Usage | Owner, Admin |
| System Handler | UC-51 Translate Text with Workspace Library and Glossary; UC-52 Process STT; UC-53 Process TTS; UC-54 Clone Voice; UC-55 Ingest Document Context for RAG; UC-56 Process Post-meeting Transcript; UC-57 Generate AI Summary; UC-58 Detect Issues; UC-59 Suggest Context; UC-60 Send Notifications; UC-61 Route Audio to External Platform; UC-62 Scan Document Security Guardrails; UC-63 Send Meeting Reminder; UC-64 Process Payment; UC-65 Process AI Chat Assistant Query; UC-66 Translate In-meeting Chat Message; UC-67 Send Invitation via Mailbox; UC-68 Soft-delete Expired Artifacts; UC-69 Deduct Real-time AI Credit; UC-70 Write Audit Log | System Handler |
| System Admin | UC-71 Manage Accounts; UC-72 Manage Workspaces; UC-73 Configure Platform Policy; UC-74 View Admin Dashboard; UC-75 Manage Subscription Plans; UC-76 Manage Global Glossary Templates; UC-77 Configure System Notifications | System Admin |

## 2. Screens and access control

49 screens (landing, auth, workspace, meetings, docs, WarpBot, voice, billing, admin) are defined in the report (Table 13); access matrix is Table 14. Key rules:
- Auth pages (Landing, Login, Register, Verify Email, Forgot/Reset Password): Unauthenticated only. Invitation page: Unauthenticated and Authenticated.
- Workspace Dashboard, Settings, Billing, Plans, Stripe Checkout, Knowledge, Workspace Document Ingestion screen: Owner/Admin only. Member Roles and Advanced (ownership transfer, deletion): Owner only.
- Admin Dashboard/Workspaces/Billing/Global Glossary: System Admin only (Workspace Terminology screen also System Admin).
- Live Meeting, Waiting Room, Room Detail, Ended Room, Artifacts, Feedback: Participant/Host (and Owner/Admin where listed). Host Transfer modal: Host and Owner/Admin. Knowledge Page: Owner/Admin.
- Payment Success/Cancelled pages: Unauthenticated, Authenticated and Owner/Admin may view.
- Web and Desktop (Electron) MUST present the same UI.

## 3. Authentication and account management

### UC-02 Register and verify email (FE-01)
- Actor: Unauthenticated User. Trigger: Register page; submit Full Name, Email, Password, default speak/listen languages.
- Main flow: validate fields (required name, email format, password min length, duplicate email) -> create user + default UserSettings -> generate expiring verification token (store hash only) -> send email -> account PENDING until verified.
- Alt: invalid/expired/missing token rejected; resend limited to 60 s cooldown and 5 per 15 min (BR-87); login blocked while pending (BR-02).
- Acceptance: verified link activates account and clears token; token single-use (BR-90); passwords never stored in plain text. BR: BR-02, BR-87, BR-90.

### UC-03 / UC-04 / UC-05 Log in, reset password, log out
- Main flow: email/password or Google ("Continue with Google") -> app session = access token + secure refresh session -> redirect to workspace or dashboard.
- MUST: 5 consecutive failures lock account for 15 min (configurable) (BR-03); disabled/locked/pending/soft-deleted accounts get no session (BR-01).
- Google: valid identity bound to WarpTalk OAuth client (BR-04); auto-link to existing local account only when that account is email-verified; otherwise block and possibly trigger verification email (rate-limited); manual link requires same email (BR-05); unlink rejected if no local password.
- Logout revokes refresh-token family; password change/reset revokes all refresh tokens (BR-06). Reset tokens valid 30 min, single-use (BR-90).

### UC-06 / UC-07 Profile and preferences
- User reads/updates only own profile and settings (BR-07); languages are BCP-47 tags; timezone UTC or valid IANA ID.

### UC-17 / UC-24 / UC-54 Voice profile and consent (FE-03)
- Trigger: Profile/Voice Profiles screen; record/upload sample and confirm consent; select dub voice; delete profile; revoke consent.
- Validations: display name, language, audio presence, size, format, file signature, consent confirmation.
- Flow: store profile + sample metadata + consent -> queue audio to TTS worker -> provider voice reference attached on completion.
- Revocation: block future clone usage; disable carried-over cloned voices or queue provider deletion. Failure leaves profile in a safe non-usable state. BR-94.

## 4. Workspace and membership (FE-01)

### UC-37 Create workspace
- Authenticated user submits workspace name. Validation: name; user not already an Internal member of another Enterprise Workspace (BR-31); creator may claim only own email domain; public domains (Gmail, Yahoo) cannot be verified domains (BR-27); domain not claimed elsewhere (BR-28).
- Result: workspace with unique slug; creator is first active member with Owner role, Internal type (BR-09); non-public email domain stored as verified domain. BR: BR-09, BR-24, BR-27, BR-28, BR-31.

### UC-39 Manage member role (Owner only)
- Owner lists eligible internal active members; can promote Member to Admin or demote Admin to Member. Not eligible: Owner, external, removed members, actor's own role.
- MUST create a role-change preview token (signed, valid 15 min, BR-88); promotion to Admin requires 60 s cooling-off; apply requires valid preview token + idempotency key; publishes member-role-changed event. BR: BR-10, BR-14, BR-15, BR-20, BR-21, BR-88.

### UC-40 / UC-41 / UC-09 Invitations and join requests
- Owner/Admin create email-bound invitations (only Owner can invite Admin; never Owner) (BR-17); expiry default 7 days (BR-33); only newest pending token acceptable (BR-34/35); acceptance identity-bound to invited email (BR-36); no silent reclassification of Internal/External (BR-37..39); external collaboration toggle Owner-only (BR-40).
- Join requests create Member-level access only, approved/rejected by Owner/Admin, duplicate request returns the open one, approval atomically creates exactly one membership (BR-41, 42, 44).

### UC-11 / UC-42 / UC-55 / UC-62 Workspace documents and RAG (FE-04)
- Upload (pdf, docx, xlsx, md, png, jpg, jpeg, webp, bmp, gif; max 10 MB; content must match extension, no empty files) (BR-62).
- Pipeline: store record with ingestion status -> safety/DLP scan (PII masking) -> extract text -> chunk -> embed -> vector index with workspace/document metadata.
- Retrieval MUST exclude unsafe, pending, soft-deleted, archived, expired or AI-ineligible docs (BR-63). Access policy is deny-overrides (BR-59); pending/awaiting-approval docs fail-closed (BR-60); external members need explicit grant or meeting-scoped grace window (default 24 h) (BR-61); upload/view/download/delete MUST be audited (BR-64); approval is Owner/Admin only, policy mutation Owner/Admin or owner (BR-95).
- Delete/archive/ineligible removes related retrieval context from future AI requests.

### UC-45 / UC-76 Glossary management
- Workspace terms: create, update, activate/deactivate, bulk import, delete. Global terms (System Admin): draft, published, archived, deleted; short terms prevented; definition required to publish; only published global terms feed prompts/retrieval. Embedding index entries updated/removed on change. Changes apply to meetings started after the change; running meetings may keep cached prompts up to 24 h (BR-98). BR: BR-57, BR-98.

## 5. Translation room and session management (FE-02)

### UC-30 / UC-34 Create translation room
- Member with meeting-creation authority (BR-81) submits title, description, meeting type, source/target languages, optional schedule, invited emails, settings. Languages must satisfy language policy (BR-52); quotas per plan (BR-86).
- Room status: WAITING if unscheduled, SCHEDULED if scheduled. Unique room code generated. Host participant created (role HOST, status INVITED). External-bridge type also creates an external bridge participant. Invitation records + emails stored. Abandoned draft creates nothing (BR-49).

### UC-12 Join translation room
- Via invite link or room code -> Room Setup/Waiting Room (camera/mic/speaker select) -> join.
- Rules: room exists, not soft-deleted, not terminal (ENDED/CANCELLED/EXPIRED) (BR-45); workspace allowed to host meetings (BR-97); speak/listen languages within room config; capacity check for non-host.
- Host: CONNECTED/HOST directly. Non-host: WAITING when requires_approval (default true), else CONNECTED/PARTICIPANT (BR-46). LEFT/DISCONNECTED may reconnect without approval; KICKED cannot rejoin (BR-47). If room IN_PROGRESS, audio routes added for the joiner. Non-workspace users denied by room ID/link (BR-82).

### UC-31 / UC-32 Manage room session
Room state machine: SCHEDULED, WAITING, IN_PROGRESS, PAUSED, ENDED, CANCELLED, EXPIRED.

| Action | Allowed from | Effect |
|---|---|---|
| Open waiting room | SCHEDULED | -> WAITING |
| Start room | SCHEDULED, WAITING | -> IN_PROGRESS, set StartedAt, build audio routes (READY); no TranslationRoomSession yet |
| Start/resume translation | IN_PROGRESS, PAUSED | creates new active TranslationRoomSession; routes READY -> BROADCASTING |
| Stop translation | IN_PROGRESS | ends session, status stays IN_PROGRESS, event translation_stopped, routes -> READY |
| Pause | IN_PROGRESS | -> PAUSED, closes session, event room_pause |
| End | authorized host (or Owner/Admin) | -> ENDED, set EndedAt, participants DISCONNECTED, event session_ends |
| Idle auto-end | WAITING/IN_PROGRESS with no connected participants | auto-ended after idle window (5 min) (BR-92) |

Invalid transitions or unauthorized actors MUST be rejected. Scheduled links expire 2 h after scheduled start (BR-92). BR: BR-49, BR-50, BR-91, BR-92.

### UC-35 Participant moderation, host transfer
- Host/Owner/Admin admit/reject waiting users, kick, mute, lock room; live host controls (lock, mute-on-entry) and artifact-sharing may change after start; other settings only while SCHEDULED/WAITING (BR-48).
- Host transfer: only current effective host, target must be in the live session; incoming gets HOST, previous demoted to PARTICIPANT; the active host is stored separately from the creator. When "participants may start translation" is enabled, eligible participants can start/resume only; other controls remain host-only. BR: BR-50, BR-51, BR-99.

### UC-29 Recurring meeting series
- Rule: daily/weekly/monthly with start time, time zone, start date, end date (default duration applied if absent), weekdays or day of month. Reject missing/unsupported rule, malformed time, unknown time zone, past start, end before start.
- Occurrences materialized as scheduled rooms without duplicates per series date; first occurrence provides the room code for the series link; worker materializes additional occurrences within a rolling horizon. Only host updates (applies to future eligible occurrences) or cancels (stops materialization, cancels upcoming rooms). BR: BR-81, BR-93.

### UC-63 Meeting reminders
- Reminders at T-30, T-10, T-1 minutes, at most once per window, retry-safe (BR-53).

## 6. Live meeting (FE-02, FE-03, FE-04)

### UC-28 / UC-66 / UC-20 Meeting chat and WarpBot
- Only host/active participants may read/send chat (BR-54); message length validated; stored with sender, original language, translation flag, mentions; broadcast to room.
- Translate-on-request by target language; same language returns original; cached by message + target + prompt version.
- Only host hides messages (hidden from participants, retained for audit).
- @WarpBot mention creates MeetingChatAssistantRequest (queued) published to assistant chat request stream; worker uses chat history, transcript context and eligible workspace knowledge; progress/tool calls/chunks/final answer may stream to chat; answers may include cited sources; publish/processing failure marks request failed and stores a fallback WarpBot message. BR: BR-54, BR-63, BR-100.

### UC-21 / UC-22 / UC-23 / UC-25 / UC-51..53 Live translation and transcript
- Pipeline: LiveKit ingress worker -> VAD -> audio chunk (room, speaker, language, timing, VAD metadata) -> STT (TranscriptSegment) -> translation per required target language (TranslationContent + SegmentTranslationLink) -> optional TTS (AudioDubbing: voice type, provider, URL, duration, status).
- Processing governed by room status, active session and audio-route state. Voice cloning default disabled; used only when speaker enabled it and consent valid; otherwise fallback to standard TTS. If subscription/credit/route checks fail the paid AI step is prevented or stopped. BR: BR-52, BR-66, BR-68, BR-69, BR-91, BR-94.
- Acceptance: transcript and translated text delivered to clients in near real time (see NFR latency targets); subtitles original/translated selectable; translated audio selectable per participant listen language.

### UC-33 Transcript correction
- Edit STT or MT segment. STT: update segment text, mark corrected, queue retranslation if translations exist. MT: TargetLanguage required; link to current translation; store human translation as current TranslationContent (prior versions kept as non-current history, BR-56).
- Reject if segment/transcript missing, mismatch, archived/finalized, or no access. Each correction stored as TranscriptCorrection (original, corrected, type, user, status, retranslation flag); ReversalCreditTransactionId remains empty until billing reversal is implemented. BR: BR-56.

### UC-13..15 / UC-26 Transcript, history, artifacts
- Transcript readable by host and actual participants; after meeting only if host shared (artifactAccess=ALL_PARTICIPANTS, default HOST_ONLY); invited-but-never-joined emails cannot read (BR-55).
- Artifacts: summaries, minutes, transcripts, recordings, issues, context suggestions; expired artifacts soft-deleted per workspace retention (UC-68, BR-83 future/proposed).

### UC-56..59 / UC-36 Post-meeting AI
- After meeting ends: summary, action items/issues, context suggestions, knowledge extraction; minutes drafted, reviewed and exported DOCX/PDF by host. Target completion 30-45 s after meeting end (NFR). Summary regeneration limited to 2,000 segments (LI-12).

## 7. Billing and usage (FE-06)

### UC-08 / UC-43 / UC-49 / UC-64 Subscribe and pay
- Owner/Admin (or System Admin) selects a plan or top-up; system creates Stripe Checkout session; top-up amount computed server-side from configured credit price; no credits/subscription activated at checkout creation.
- On confirmed payment (webhook or session verification, idempotent): create/update active subscription and grant plan credits, or add top-up credits to the active subscription; record CreditTransaction, Payment and Invoice where applicable. Failed/cancelled/unconfirmed payment activates nothing. One active subscription per workspace. BR: BR-60, BR-61, BR-62, BR-65, BR-69, BR-73.

### UC-50 / UC-69 Usage metering
- Billing Settlement Worker consumes `translate:results` and `tts:results` (STT and assistant events not billed currently). Resolves workspace from meeting-room projection and active subscription; skips early translation segments, cached TTS and system/invalid speakers; quantity in seconds x active rate card (min 1 credit per event, BR-68); deterministic idempotency key; no active subscription means no charge; repeated failure moves event to dead-letter stream.
- When credits run out: overage only if allowed and under cap, else charge refused and AI suspended (BR-67). Ledger entry for every charge/top-up/purchase/renewal/adjustment (BR-69). BR: BR-66..BR-72.

## 8. Notification and administration (FE-07)

### UC-60 / UC-67 / UC-77 Notifications
- NotificationMessage stores type, title, content, action URL, payload JSON, read status/time. Users see only own notifications; paginated, page size clamped 1-100 (default 50) (BR-76); mark one/all read; default preference (email, push, in-app all enabled) created on first use; email honors preference (BR-75).
- Admin announcements (System Admin): payload validated per type, HTML rejected, targets deduplicated, cap 10,000 users, fan-out chunks of at most 1,000 (BR-77); published to `admin-notifications-delivery` stream, publish is the synchronous success boundary (BR-78); repeated failure goes to dead-letter stream; only specific-users audience supported (BR-96).

### UC-71..75 / UC-48 / UC-70 Admin
- System Admin: manage accounts (incl. unlock), workspaces (suspend/reactivate gated by System Admin policy and written to admin audit log, BR-79), plans (deactivated plans not selectable, BR-74), global glossary, dashboard, system config and AI model settings.
- Workspace suspension/inactivity blocks new meeting creation, admission, start, document ops and join/leave requests (BR-97); workspace deletion cancels in-progress meetings (BR-85); removing a member evicts them from live meetings (BR-84, future/proposed).
- Workspace dashboard (Owner/Admin): usage, credits, activity, health indicators.

## 9. External platform integration (FE-05)

- Host links a Google Meet to a WarpTalk External Bridge room; Desktop app opens a publish-only LiveKit "bridge" connection carrying Google Meet audio into the room for STT/translation as native audio; WarpBot can schedule bridge rooms; Desktop app can detect live meetings.
- Audio via virtual drivers (VB-CABLE Windows, BlackHole macOS); cloned audio returned through virtual mic; unsupported OS/driver = captions only; only WarpTalk users cloned, others stock voice (LI-4, LI-8, LI-10). UC-47 configures platform settings; UC-61 routes audio.
- See 06-service-design.md for the Google Meet bridge design (Report Section IV and supporting spec).

## 10. Non-screen system functions (Table 15)

Real-time audio ingress, STT processor, real-time translation engine (glossary-aware), TTS synthesizer (voice profile lifecycle by consent), smart suggestion engine, real-time delivery gateway (SignalR), knowledge indexing worker, AI meeting assistant, document processing pipeline, glossary synchronization, DLP scanner, semantic search engine, transcript archiver, meeting event handler (LiveKit webhooks), workspace sync (member revocation propagation), meeting reminder service, schedule materialization, meeting lifecycle manager, usage metering service, payment gateway integration, OAuth verification service, billing maintenance jobs (rollovers, invoice scan, entitlement reconciliation).
