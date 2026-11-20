# 05 - Data Model

Source: Report 7, SDD IV.2 (Database Design, Table 22 and Table 23) plus entity fields named in the detailed-design class specifications. This file gives the narrative view (purpose and key fields). Column-level detail (types, constraints, indexes, relationships) is in [05a-data-dictionary.md](05a-data-dictionary.md), generated from the team ERD source, which is authoritative where the two differ.

## 1. Principles

- Database-per-service on PostgreSQL 18; one schema namespace per service. No cross-service foreign keys; cross-service references are by ID and kept consistent by events (MassTransit/RabbitMQ, Redis Streams).
- Primary keys are UUIDs. Soft delete is used for workspaces and rooms (BR-43, BR-45).
- Schema evolution: forward-only ordered SQL migrations applied by a migrator service; applied versions tracked in `public.schema_migrations` (per-service `schema_migrations` tables appear in the physical layout).
- Vector data lives in Qdrant, not PostgreSQL. Binary objects (audio, exports, documents, avatars) live in MinIO / Cloudflare R2; the database stores keys/URLs.
- Redis holds transient state: active workspace per user, streams, idempotency keys, pub/sub.

## 2. Tables by service (63 tables)

### Auth service (schemas `auth`, `voice`)
| Table | Purpose / key fields |
|---|---|
| auth.users | Accounts: Id, Email (unique, case-insensitive), PasswordHash, FullName, AvatarUrl, PreferredLanguage, status (active/disabled/locked/pending verification), failed-login counters and lock-until, soft delete |
| auth.user_settings | 1:1 with users: default audio input/output, AutoTranslateEnabled, default room settings, VoiceCloneEnabled |
| auth.roles / auth.permissions / auth.role_permissions / auth.user_roles | System-level RBAC catalog (for example System Admin) |
| auth.refresh_tokens | Token (stored hashed), UserId, ExpiresAt, token family id, revoked/replaced markers for reuse detection |
| voice.voice_profiles | Per-user voice: UserId, Provider, ProviderVoiceId, Source (Enrollment or InMeeting), status, quality |
| voice.voice_samples | Audio sample references (storage key) used as cloning input |

Voice consent records (VoiceConsent: UserId, IsGranted, GrantedAt, revocation) are part of the Auth service data (BR-94).

### Workspace service (schema `workspace`)
| Table | Purpose / key fields |
|---|---|
| workspaces | Id, Name, Description, status (active/suspended/inactive), settings (MaxParticipantsLimit, AllowGuestAccess/external collaboration, DefaultAiModel, AudioRetentionDays, language policy, invitation window), soft delete |
| workspace_members | WorkspaceId, UserId, Role (Owner/Admin/Member), MembershipType (Internal/External), JoinedAt, status; unique active Internal membership per user (BR-31) |
| workspace_invitations | Invitations and join requests: email, role, membership class, token hash, status (PENDING, ACCEPTED, REVOKED, REPLACED, EXPIRED, REJECTED), expiry, reviewer |
| workspace_verified_domains | Verified domain per workspace; an active verified domain belongs to one workspace only (BR-28) |
| workspace_documents | Uploaded document metadata: owner, type, size, storage key, ingestion status, IsAiAllowed, AiEligible, public/restricted, retention |
| workspace_document_access_policies | Fine-grained allow/deny rules per document (deny overrides, BR-59) |
| workspace_document_audits | Audit of upload, view, download, update, delete (BR-64) |
| workspace_entitlement_snapshots | Read model of plan entitlements and quotas, fed by billing events |
| workspace_admin_actions | Audit of workspace/system admin actions (suspend, reactivate, ownership transfer) |

### Translation Room service (schema `translation_room`)
| Table | Purpose / key fields |
|---|---|
| translation_room_series | Recurring series template (recurrence rule, host, settings) |
| translation_rooms | Central entity: WorkspaceId, host, code/link, type, source language, target languages, status (SCHEDULED, WAITING, IN_PROGRESS, PAUSED, ENDED, CANCELLED, EXPIRED), schedule, requires_approval, lock/mute settings, artifactAccess (HOST_ONLY default or ALL_PARTICIPANTS), external-bridge fields, soft delete |
| translation_room_participants | Roster: user or email, role (Host/Participant), lifecycle status (WAITING, CONNECTED, LEFT, KICKED, ...), per-participant listening language |
| translation_room_invitations | Email invitations to a room |
| translation_room_sessions | Runtime sessions of a room (TranslationRoomSession): start, end, status |
| translation_room_artifacts | Generated outputs: transcript exports, audio samples, summaries; storage key, type, retention |
| translation_room_audio_routes | Route from source speaker to target listener: status READY or BROADCASTING (plus terminal COMPLETED), language pair, voice-clone flag |
| translation_room_feedback | User feedback on room and translation quality |
| supported_languages | Language catalog (Vietnamese, English, Japanese in Release 1.0) |

### Meeting service (schema `meeting`)
| Table | Purpose |
|---|---|
| meeting_rooms | LiveKit RTC room bound to a Translation Room |
| rtc_stream_participants | Active WebRTC connections |
| meeting_tracks | Audio/video track metadata |
| rtc_session_revocations | Ban/removal list per RTC session |
| meeting_chat_messages | In-meeting chat (moderation hides without erasing, BR-54) |
| meeting_chat_translations | Cached chat translations |
| meeting_chat_assistant_requests | WarpBot requests triggered from chat (BR-100) |

### Transcript service (schema `transcript`)
| Table | Purpose |
|---|---|
| transcripts | Header and versioning; status including FINALIZED and ARCHIVED |
| transcript_segments | STT segments with speaker, timing, language, source text |
| transcript_corrections | Edit history for segments or segment translations (original, corrected, author, review status) |
| transcript_exports | Export log (PDF, DOCX, TXT, VTT, SRT) |
| translation_contents | Content-addressed translation cache |
| segment_translation_links | Many-to-many bridge segment to translation content; current vs non-current history |
| audio_dubbings | Cache of TTS/cloned audio files |
| glossaries / glossary_terms | Workspace-scoped glossary and terms |
| global_glossary_terms / global_glossary_audits | Platform-wide catalog (System Admin) and change audit; terms draft/published/archived, minimum length 3 |

### Billing service (schema `subscription`)
| Table | Purpose |
|---|---|
| plans | Free, Pro, Enterprise catalog; limits (concurrent rooms, languages, feature flags, included credits); active flag |
| subscriptions | One active per workspace (BR-65); status, period, cancel-at-period-end |
| payments | Stripe payment history (the table description also names VNPay; see open issues) |
| invoices | Invoices derived from payments |
| credit_transactions | Credit ledger: top-up, deduction, purchase, renewal, adjustment; references and idempotency key |
| credit_balance_snapshots | Point-in-time balance |
| usage_records | Usage per event (translation, STT, voice clone, TTS), unit, quantity |
| usage_rate_card | Rates per feature/model |
| workspace_entitlement_overrides | Per-workspace quota overrides |
| sales_inquiries | Enterprise sales requests |

### Assistant service (schema `assistant`)
assistant_conversations; assistant_messages; assistant_tool_calls (function-calling history).

### Notification service (schema `notification`)
| Table | Purpose |
|---|---|
| notification_messages | User notifications, partitioned by created_at |
| notification_preferences | Per-user channel settings (Email, Push, In-app); default all enabled |
| notification_templates | Templates by type and channel |
| push_subscriptions | FCM / Web Push device tokens |
| admin_notifications | Manually created admin announcements (audience mode, targets) |

## 3. Key relationships

- users 1-1 user_settings; users 1-N refresh_tokens, voice_profiles, voice_samples.
- workspaces 1-N workspace_members, invitations, verified_domains, documents, glossaries; documents 1-N access_policies and audits.
- translation_rooms 1-N participants, invitations, sessions, artifacts, audio_routes, feedback; rooms N-1 series.
- meeting_rooms 1-1 translation room (by ID); meeting_rooms 1-N participants, tracks, chat messages.
- transcripts 1-N segments; segments N-M translation_contents; segments 1-N corrections.
- subscriptions N-1 plans; payments 1-N invoices; credit_transactions form the ledger referenced by usage_records.

## 4. Vector database (Qdrant)

| Element | Specification |
|---|---|
| Collection | One per workspace, named `workspace_{workspaceId}` |
| Point | One normalized text chunk. Document chunks use deterministic IDs from documentId and chunkIndex; transcript chunks use segment IDs |
| Vector | 1536 dimensions, `text-embedding-3-small`, cosine distance |
| Payload | workspace_id, source_type, source_id, chunk_id, text, provenance, retention_state, deletion_state, ai_retrieval |
| source_type | document, transcript, meeting_summary, glossary_term, global_glossary_term, workspace_context |
| Ingestion | Workspace service publishes `embedding:index_requests` (carries ai_retrieval_allowed); embedding worker upserts to Qdrant and replies on `embedding:index_results` with indexed, blocked, failed or deleted, which updates document status |
| Retrieval | Filter by workspace_id and ai_retrieval=true; collection derived from route workspaceId; role-based access checks applied by the Workspace Knowledge API (BR-63) |

## 5. Storage conventions

Object storage keys hold voice samples, document files, avatars, transcript exports, summary files and dubbing audio. Retention is governed by workspace AudioRetentionDays and artifact retention (BR-83 future).
