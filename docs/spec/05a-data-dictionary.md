# 05a - Data Dictionary

Column-level schema for every service database. Generated from `docs/erd/warptalk-erd.dbml` in [WarpTalk-CapstoneProject/warptalk-infrastructure](https://github.com/WarpTalk-CapstoneProject/warptalk-infrastructure/blob/main/docs/erd/warptalk-erd.dbml), branch `main`, last changed in commit `db4955e` (2026-10-02, USD accounting-currency migration). It is more detailed and more recent than the ER figures in Report 7; where they differ, this file wins and 05-data-model.md gives the narrative.

Conventions: schema prefix = owning service database. `pk` primary key, `nn` not null, `uq` unique, `default` default value. References that cross a schema are logical only (no Postgres foreign key); `check-database-boundaries.sh` must fail the build if one is created.

## Contents

- **Auth**: `auth.users`, `auth.user_settings`, `auth.roles`, `auth.permissions`, `auth.user_roles`, `auth.role_permissions`, `auth.refresh_tokens`
- **Voice**: `voice.voice_profiles`, `voice.voice_samples`
- **Workspace**: `workspace.workspaces`, `workspace.workspace_members`, `workspace.workspace_invitations`, `workspace.workspace_verified_domains`, `workspace.workspace_documents`, `workspace.workspace_document_access_policies`, `workspace.workspace_document_audits`, `workspace.workspace_entitlement_snapshots`, `workspace.workspace_admin_actions`
- **TranslationRoom**: `translation_room.translation_room_series`, `translation_room.translation_rooms`, `translation_room.translation_room_participants`, `translation_room.translation_room_invitations`, `translation_room.translation_room_sessions`, `translation_room.translation_room_artifacts`, `translation_room.translation_room_audio_routes`, `translation_room.translation_room_feedback`, `translation_room.supported_languages`
- **RtcSession**: `meeting.meeting_rooms`, `meeting.rtc_stream_participants`, `meeting.rtc_session_revocations`, `meeting.meeting_tracks`, `meeting.meeting_chat_messages`, `meeting.meeting_chat_translations`, `meeting.meeting_chat_assistant_requests`
- **Transcript**: `transcript.transcripts`, `transcript.transcript_segments`, `transcript.transcript_corrections`, `transcript.transcript_exports`, `transcript.translation_contents`, `transcript.segment_translation_links`, `transcript.audio_dubbings`, `transcript.glossaries`, `transcript.glossary_terms`, `transcript.global_glossary_terms`, `transcript.global_glossary_audits`
- **Billing**: `subscription.plans`, `subscription.subscriptions`, `subscription.payments`, `subscription.invoices`, `subscription.credit_transactions`, `subscription.credit_balance_snapshots`, `subscription.usage_records`, `subscription.usage_rate_card`, `subscription.workspace_entitlement_overrides`, `subscription.sales_inquiries`
- **Assistant**: `assistant.assistant_conversations`, `assistant.assistant_messages`, `assistant.assistant_tool_calls`
- **Notification**: `notification.notification_messages`, `notification.notification_preferences`, `notification.notification_templates`, `notification.push_subscriptions`, `notification.admin_notifications`

## Auth

### `auth.users`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| email | varchar(320) | nn, uq |
| password_hash | varchar(255) |  |
| full_name | varchar(150) | nn |
| avatar_url | varchar(500) |  |
| phone | varchar(20) |  |
| preferred_language | varchar(15) | nn, default: 'vi-VN' |
| timezone | varchar(50) | nn, default: 'Asia/Ho_Chi_Minh' |
| is_active | boolean | nn, default: true |
| is_locked | boolean | nn, default: false |
| failed_login_attempts | integer | nn, default: 0 |
| locked_until | timestamptz |  |
| email_verified | boolean | nn, default: false |
| email_verified_at | timestamptz |  |
| google_id | varchar(255) | uq |
| last_login_at | timestamptz |  |
| last_login_ip | varchar(45) |  |
| email_verification_token_hash | varchar(64) |  |
| email_verification_token_expires_at | timestamptz |  |
| password_reset_token_hash | varchar(64) |  |
| password_reset_token_expires_at | timestamptz |  |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `auth.user_settings`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| user_id | uuid | nn, uq |
| default_speak_language | varchar(15) | nn, default: 'vi-VN' |
| default_listen_language | varchar(15) | nn, default: 'en-US' |
| voice_clone_enabled | boolean | nn, default: false |
| mic_noise_suppression | boolean | nn, default: true |
| default_translation_room_type | varchar(20) | nn, default: 'group' |
| auto_record_translation_rooms | boolean | nn, default: false |
| auto_generate_summary | boolean | nn, default: true |
| default_max_participants | integer | nn, default: 10 |
| theme | varchar(10) | nn, default: 'system' |
| transcript_font_size | integer | nn, default: 14 |
| show_original_transcript | boolean | nn, default: true |
| show_translated_transcript | boolean | nn, default: true |
| high_contrast | boolean | nn, default: false |
| screen_reader_mode | boolean | nn, default: false |
| updated_at | timestamptz | nn |
| updated_by | uuid |  |

### `auth.roles`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| name | varchar(50) | nn, uq |
| description | varchar(255) |  |
| is_system | boolean | nn, default: false |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `auth.permissions`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| code | varchar(100) | nn, uq |
| description | varchar(255) |  |
| group_name | varchar(50) | nn |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `auth.user_roles`

workspace_id was dropped in migration 007-03-06-2026: workspace roles live in workspace.workspace_members.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| user_id | uuid | nn |
| role_id | uuid | nn |
| assigned_at | timestamptz | nn |
| assigned_by | uuid |  |
| revoked_at | timestamptz |  |
| revoked_by | uuid |  |

Indexes: `(user_id, role_id) [unique]`

### `auth.role_permissions`

| Column | Type | Constraints |
|---|---|---|
| role_id | uuid | nn |
| permission_id | uuid | nn |
| created_at | timestamptz | nn |
| created_by | uuid |  |

Indexes: `(role_id, permission_id) [pk]`

### `auth.refresh_tokens`

family_id carries reuse detection: replaying a rotated token revokes the whole family.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| user_id | uuid | nn |
| token_hash | varchar(255) | nn, uq |
| family_id | uuid | nn, default: `gen_random_uuid()` |
| device_info | varchar(255) |  |
| ip_address | varchar(45) |  |
| expires_at | timestamptz | nn |
| revoked_at | timestamptz |  |
| created_at | timestamptz | nn |

## Voice

### `voice.voice_profiles`

A profile is personal (user_id required) and only optionally scoped to a workspace.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| user_id | uuid | nn |
| workspace_id | uuid |  |
| display_name | varchar(100) |  |
| provider | varchar(50) |  |
| embedding_ref | varchar(500) |  |
| language | varchar(15) |  |
| status | varchar(20) | nn, default: 'active' |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `voice.voice_samples`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| voice_profile_id | uuid | nn |
| sample_type | varchar(30) | nn |
| file_url | varchar(500) |  |
| duration_seconds | integer |  |
| language | varchar(15) |  |
| contains_raw_audio | boolean | nn, default: true |
| retention_until | timestamptz |  |
| created_at | timestamptz | nn |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

## Workspace

### `workspace.workspaces`

No plan / trial / region columns: that state belongs to subscription.subscriptions.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| name | varchar(150) | nn |
| slug | varchar(100) | nn, uq |
| owner_id | uuid | nn |
| logo_url | varchar(500) |  |
| allow_external_collaboration | boolean | nn, default: false |
| require_verified_domain_for_internal | boolean | nn, default: true |
| allow_subdomains | boolean | nn, default: false |
| settings | jsonb | nn, default: `'{}'` |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `workspace.workspace_members`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| user_id | uuid | nn |
| role_id | uuid | nn |
| membership_type | varchar(20) | nn, default: 'internal' |
| status | varchar(20) | nn, default: 'active' |
| can_create_meetings | boolean | nn, default: true |
| joined_at | timestamptz | nn |
| removed_at | timestamptz |  |
| removed_by | uuid |  |

Indexes: `(workspace_id, user_id) [unique]`

### `workspace.workspace_invitations`

Also carries join requests: requested_by set, token_hash null until an admin approves.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| email | varchar(320) | nn |
| role_id | uuid | nn |
| membership_type | varchar(20) | nn, default: 'internal' |
| matched_domain_id | uuid |  |
| invited_by | uuid | nn |
| token_hash | varchar(255) | uq |
| status | varchar(20) | nn, default: 'pending' |
| expires_at | timestamptz | nn |
| accepted_at | timestamptz |  |
| delivery_status | varchar(50) | nn, default: 'NotSent' |
| provider_message_id | varchar(255) |  |
| last_sent_at | timestamptz |  |
| sent_count | integer | nn, default: 0 |
| requested_by | uuid |  |
| reviewed_by | uuid |  |
| reviewed_at | timestamptz |  |
| created_at | timestamptz | nn |

### `workspace.workspace_verified_domains`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| domain | varchar(255) | nn |
| status | varchar(20) | nn, default: 'pending' |
| verification_method | varchar(50) | nn |
| verification_token | varchar(255) | nn |
| verified_at | timestamptz |  |
| verified_by | uuid |  |
| revoked_at | timestamptz |  |

Indexes: `domain [unique, note: 'partial: WHERE status = verified']`

### `workspace.workspace_documents`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| uploaded_by | uuid |  |
| owner_id | uuid |  |
| name | varchar(255) | nn |
| file_name | varchar(255) | nn |
| file_extension | varchar(20) | nn |
| mime_type | varchar(100) | nn |
| size_bytes | bigint | nn |
| storage_provider | varchar(50) | nn |
| storage_key | varchar(500) | nn |
| source_type | varchar(50) | nn |
| source_id | uuid |  |
| document_type | varchar(50) | nn |
| source_language | varchar(20) |  |
| detected_language | varchar(20) |  |
| business_domain | varchar(100) |  |
| summary | text |  |
| keywords | jsonb |  |
| ai_eligible | boolean | nn, default: true |
| is_ai_allowed | boolean | nn, default: true |
| ai_usage_policy | jsonb |  |
| ingestion_status | varchar(30) | nn, default: 'pending' |
| last_indexed_at | timestamptz |  |
| index_version | varchar(50) |  |
| confidentiality_level | varchar(30) | nn, default: 'public_internal' |
| retention_state | varchar(30) | nn, default: 'active' |
| status | varchar(30) | nn, default: 'active' |
| created_at | timestamptz | nn |
| updated_at | timestamptz |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `workspace.workspace_document_access_policies`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| document_id | uuid | nn |
| workspace_id | uuid | nn |
| subject_type | varchar(30) | nn |
| subject_id | uuid |  |
| subject_key | varchar(150) |  |
| permission | varchar(30) | nn |
| effect | varchar(20) | nn |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |

### `workspace.workspace_document_audits`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| document_id | uuid | nn |
| workspace_id | uuid | nn |
| actor_id | uuid |  |
| action | varchar(50) | nn |
| action_at | timestamptz | nn |
| metadata | jsonb |  |
| ip_address | varchar(64) |  |
| user_agent | varchar(500) |  |

### `workspace.workspace_entitlement_snapshots`

Read-model projected from billing events over the outbox. Never written by hand.

| Column | Type | Constraints |
|---|---|---|
| workspace_id | uuid | pk |
| entitlements | jsonb | nn, default: `'{}'` |
| plan_slug | varchar(80) |  |
| has_active_subscription | boolean | nn, default: false |
| resolved_at | timestamptz | nn |
| last_event_id | uuid | nn |
| updated_at | timestamptz | nn |

### `workspace.workspace_admin_actions`

workspace_id is nullable: a platform-level action has no workspace.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid |  |
| action | varchar(30) | nn |
| reason | text | nn |
| performed_by | uuid | nn |
| performed_at | timestamptz | nn |
| correlation_id | varchar(100) |  |
| source_service | varchar(50) | nn, default: 'workspace-service' |
| entity_type | varchar(50) | nn, default: 'workspace' |
| entity_id | uuid |  |
| result | varchar(20) | nn, default: 'succeeded', — succeeded \| failed |
| before_summary | jsonb |  |
| after_summary | jsonb |  |

## TranslationRoom

### `translation_room.translation_room_series`

A series is a BOOKING, not a meeting. Fields below materialized_through_local_date are the template stamped onto each occurrence.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| host_id | uuid | nn |
| recurrence_type | varchar(20) | nn, default: 'DAILY', — DAILY \| WEEKLY \| MONTHLY |
| recurrence_interval | integer | nn, default: 1 |
| recurrence_by_weekdays | jsonb |  |
| recurrence_by_month_day | smallint |  |
| start_time_local | time | nn |
| time_zone | varchar(64) | nn |
| starts_on_local_date | date | nn |
| ends_on_local_date | date | nn |
| status | varchar(20) | nn, default: 'ACTIVE', — ACTIVE \| CANCELLED \| COMPLETED |
| materialized_through_local_date | date |  |
| title | varchar(255) | nn |
| description | text |  |
| translation_room_type | varchar(20) | nn |
| max_participants | integer | nn, default: 0 |
| source_language | varchar(15) | nn |
| target_languages | jsonb | nn, default: `'[]'` |
| settings | jsonb | nn, default: `'{}'` |
| invited_emails | jsonb | nn, default: `'[]'` |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |

### `translation_room.translation_rooms`

active_host_id (WT-359) records a host handover; effective host = COALESCE(active_host_id, host_id). host_id owns the booking and is never overwritten.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| host_id | uuid | nn |
| active_host_id | uuid |  |
| title | varchar(255) | nn |
| description | text |  |
| translation_room_code | varchar(12) | nn |
| status | varchar(255) | nn, default: 'SCHEDULED', — SCHEDULED \| WAITING \| IN_PROGRESS \| PAUSED \| ENDED \| CANCELLED \| EXPIRED \| FAILED |
| translation_room_type | varchar(20) | nn, default: 'GROUP' |
| max_participants | integer | nn, default: 10 |
| source_language | varchar(15) | nn |
| target_languages | jsonb | nn, default: `'[]'` |
| settings | jsonb | nn, default: `'{}'` |
| scheduled_at | timestamptz |  |
| started_at | timestamptz |  |
| ended_at | timestamptz |  |
| duration_seconds | integer |  |
| reminder_30min_sent_at | timestamptz |  |
| reminder_10min_sent_at | timestamptz |  |
| reminder_1min_sent_at | timestamptz |  |
| series_id | uuid |  |
| series_occurrence_local_date | date |  |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

Indexes: `translation_room_code [unique, note: 'partial: WHERE series_id IS NULL — occurrences of a series share one code']`; `(series_id, series_occurrence_local_date) [unique, note: 'partial: WHERE series_id IS NOT NULL']`

### `translation_room.translation_room_participants`

user_id is nullable: a guest can attend without an account.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| translation_room_id | uuid | nn |
| user_id | uuid |  |
| display_name | varchar(100) | nn |
| role | varchar(20) | nn, default: 'participant', — host \| participant |
| listen_language | varchar(15) | nn |
| speak_language | varchar(15) | nn |
| status | varchar(255) | nn, default: 'INVITED', — INVITED \| WAITING \| CONNECTED \| DISCONNECTED \| LEFT \| KICKED \| REJECTED |
| connection_type | varchar(20) | nn, default: 'webrtc' |
| is_translation_audio_enabled | boolean | nn, default: true |
| is_using_voice_clone | boolean | nn, default: false |
| joined_at | timestamptz |  |
| left_at | timestamptz |  |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

### `translation_room.translation_room_invitations`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| translation_room_id | uuid | nn |
| email | varchar(255) | nn |
| status | varchar(50) | nn, default: 'PENDING' |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

Indexes: `(translation_room_id, email) [unique]`

### `translation_room.translation_room_sessions`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| translation_room_id | uuid | nn |
| main_language | varchar(15) | nn |
| audio_url | varchar(500) |  |
| status | varchar(20) | nn, default: 'ACTIVE' |
| started_at | timestamptz |  |
| ended_at | timestamptz |  |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

### `translation_room.translation_room_artifacts`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| translation_room_id | uuid | nn |
| artifact_type | varchar(255) | nn, — TRANSCRIPT_EXPORT \| SUMMARY_EXPORT \| DEBUG_LOG \| OPTIONAL_RECORDING \| AUDIO_SAMPLE |
| file_url | varchar(500) |  |
| file_format | varchar(20) |  |
| file_size_bytes | bigint |  |
| content | text |  |
| provider_artifact_id | varchar(255) |  |
| contains_raw_audio | boolean | nn, default: false |
| contains_raw_video | boolean | nn, default: false |
| consent_required | boolean | nn, default: false |
| retention_until | timestamptz |  |
| status | varchar(20) | nn, default: 'active' |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `translation_room.translation_room_audio_routes`

A route is one speaker to one listener; both participant ids are required.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| translation_room_id | uuid | nn |
| source_participant_id | uuid | nn |
| target_participant_id | uuid | nn |
| source_language | varchar(15) | nn |
| target_language | varchar(15) | nn |
| voice_clone_enabled | boolean | nn, default: false |
| stream_id | varchar(100) |  |
| status | varchar(20) | nn, default: 'active' |
| is_current | boolean | nn, default: true |
| started_at | timestamptz |  |
| ended_at | timestamptz |  |

Indexes: `(source_participant_id, target_participant_id) [unique, note: 'partial: WHERE is_current']`

### `translation_room.translation_room_feedback`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| translation_room_id | uuid | nn |
| user_id | uuid | nn |
| overall_rating | integer | nn |
| translation_quality | integer |  |
| audio_quality | integer |  |
| voice_clone_quality | integer |  |
| ai_summary_quality | integer |  |
| comments | text |  |
| communication_insights | jsonb |  |
| created_at | timestamptz | nn |

Indexes: `(translation_room_id, user_id) [unique]`

### `translation_room.supported_languages`

Lookup read by language CODE, not by foreign key. Deliberately unconnected: an arrow here would assert a constraint the database does not have.

| Column | Type | Constraints |
|---|---|---|
| code | varchar(15) | pk |
| name | varchar(100) | nn |
| native_name | varchar(100) |  |
| is_active | boolean | nn, default: true |

## RtcSession

### `meeting.meeting_rooms`

The schema permits N sessions per translation room and constrains only the current one.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| translation_room_id | uuid | nn |
| provider_room_name | varchar(255) | nn |
| active_host_id | uuid |  |
| status | varchar(255) | nn, default: 'CREATED' |
| is_locked | boolean | nn, default: false |
| mute_on_entry | boolean | nn, default: false |
| active_egress_id | text |  |
| is_current | boolean | nn, default: true |
| is_active | boolean | nn, default: true |
| ended_at | timestamptz |  |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

Indexes: `translation_room_id [unique, note: 'partial: WHERE is_current — many rows allowed, one live session']`

### `meeting.rtc_stream_participants`

Renamed from meeting_participants on 2026-08-12. People with an RTC stream, not the meeting roster - that is translation_room.translation_room_participants. No language columns for the same reason.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| meeting_room_id | uuid | nn |
| user_id | uuid |  |
| provider_identity | varchar(255) | nn |
| display_name | varchar(255) |  |
| translation_room_participant_id | uuid |  |
| device_info | varchar(255) |  |
| is_active | boolean | nn, default: true |
| joined_at | timestamptz |  |
| left_at | timestamptz |  |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `meeting.rtc_session_revocations`

Renamed from meeting_invitations on 2026-08-12. A deny list for the live session: a row means kicked or rejected and may not rejoin. A ban targets a person OR a group, which is why it cannot fold into translation_room_invitations.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| meeting_room_id | uuid | nn |
| invitee_user_id | uuid |  |
| invitee_email | varchar(255) |  |
| group_id | uuid |  |
| workspace_id | uuid | nn |
| status | varchar(50) | default: 'PENDING', — written only as REVOKED |
| expires_at | timestamptz |  |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |

### `meeting.meeting_tracks`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| meeting_participant_id | uuid | nn |
| provider_track_id | varchar(255) | nn |
| media_type | varchar(255) | nn, default: 'VIDEO', — AUDIO \| VIDEO |
| is_muted | boolean | nn, default: false |
| is_active | boolean | nn, default: true |
| published_at | timestamptz |  |
| unpublished_at | timestamptz |  |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `meeting.meeting_chat_messages`

sender_user_id and participant_id are nullable: system messages have no sender.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| meeting_room_id | uuid | nn |
| workspace_id | uuid | nn |
| sender_user_id | uuid |  |
| participant_id | uuid |  |
| sender_display_name | varchar(255) | nn |
| sender_type | varchar(50) | nn, default: 'user' |
| message_type | varchar(50) | nn, default: 'text' |
| original_language | varchar(50) | nn |
| original_text | text | nn |
| translation_enabled | boolean | nn, default: false |
| mentions | jsonb | nn, default: `'[]'` |
| is_hidden | boolean | nn, default: false |
| file_url | text |  |
| file_name | varchar(255) |  |
| file_size_bytes | bigint |  |
| content_type | varchar(255) |  |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

### `meeting.meeting_chat_translations`

Translation cache. prompt_version is part of the key so a prompt change does not serve stale text.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| message_id | uuid | nn |
| meeting_room_id | uuid | nn |
| source_language | varchar(50) | nn |
| target_language | varchar(50) | nn |
| translated_text | text | nn |
| model_used | varchar(100) |  |
| confidence | numeric |  |
| prompt_version | integer | nn, default: 1 |
| created_at | timestamptz | nn |

Indexes: `(message_id, target_language, prompt_version) [unique]`

### `meeting.meeting_chat_assistant_requests`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| trigger_message_id | uuid | nn |
| meeting_room_id | uuid | nn |
| workspace_id | uuid | nn |
| requested_by_user_id | uuid | nn |
| prompt | text | nn |
| context_scope | varchar(100) | nn, default: 'current_meeting' |
| status | varchar(50) | nn, default: 'queued' |
| created_at | timestamptz |  |
| completed_at | timestamptz |  |

## Transcript

### `transcript.transcripts`

Version chain: previous_transcript_id is nullable and the head is null. No unique index on it, so a branch is possible.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| translation_room_id | uuid | nn |
| translation_room_session_id | uuid |  |
| version | integer | nn, default: 1 |
| status | varchar(255) | nn, default: 'RECORDING' |
| source_language | varchar(15) | nn |
| total_segments | integer | nn, default: 0 |
| total_duration_ms | integer | nn, default: 0 |
| is_current | boolean | nn, default: true |
| previous_transcript_id | uuid |  |
| engine_version | varchar(50) |  |
| last_sequence_order | integer | nn, default: 0 |
| is_active | boolean | nn, default: true |
| finalized_at | timestamptz |  |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

Indexes: `(translation_room_id, version) [unique]`; `translation_room_id [unique, note: 'partial: WHERE is_current']`

### `transcript.transcript_segments`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| transcript_id | uuid | nn |
| speaker_participant_id | uuid |  |
| speaker_name | varchar(100) | nn |
| original_text | text | nn |
| original_language | varchar(15) | nn |
| start_time_ms | integer | nn |
| end_time_ms | integer | nn |
| confidence | decimal(5,4) |  |
| sequence_order | integer | nn |
| is_corrected | boolean | nn, default: false |
| is_final | boolean | nn, default: true |
| matched_segment_id | uuid |  |
| clean_text | text | — WT-716 tier 1. NULL = not cleaned (read original_text); empty = filler only |
| clean_flags | text[] |  |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

Indexes: `(transcript_id, sequence_order) [unique]`

### `transcript.transcript_corrections`

Many corrections per segment and NO version chain: every edit keeps its own row, author and timestamp, which is what makes revert possible.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| segment_id | uuid | nn |
| user_id | uuid | nn |
| original_text | text | nn |
| corrected_text | text | nn |
| correction_type | varchar(255) | nn, — STT \| TRANSLATION |
| status | varchar(255) | nn, default: 'PENDING' |
| triggered_retranslation | boolean | nn, default: false |
| translation_content_id | uuid |  |
| reversal_credit_transaction_id | uuid |  |
| reviewed_by | uuid |  |
| reviewed_at | timestamptz |  |
| created_at | timestamptz | nn |

### `transcript.transcript_exports`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| transcript_id | uuid | nn |
| user_id | uuid | nn |
| format | varchar(10) | nn |
| file_url | varchar(500) | nn |
| included_languages | jsonb | nn, default: `'[]'` |
| created_at | timestamptz | nn |

### `transcript.translation_contents`

Content-addressed translation cache, deduplicated per workspace by text_hash.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| text_hash | varchar(64) | nn |
| target_language | varchar(15) | nn |
| translated_text | text | nn |
| translator_model | varchar(100) | nn |
| confidence | decimal(5,4) |  |
| source_stt_confidence | decimal(5,4) |  |
| is_retranslated | boolean | nn, default: false |
| previous_translation_content_id | uuid |  |
| latency_ms | integer |  |
| status | varchar(20) | nn, default: 'pending' |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

Indexes: `(workspace_id, text_hash, target_language) [unique]`

### `transcript.segment_translation_links`

| Column | Type | Constraints |
|---|---|---|
| segment_id | uuid | nn |
| translation_content_id | uuid | nn |
| target_language | varchar(15) | nn |
| is_current | boolean | nn, default: true |
| delivered_at | timestamptz |  |
| created_at | timestamptz | nn |

Indexes: `(segment_id, translation_content_id) [pk]`; `(segment_id, target_language) [unique, note: 'partial: WHERE is_current']`

### `transcript.audio_dubbings`

Deliberately NO link to a voice profile: voice_type, provider and provider_voice_id are stored instead, so the file stays attributable after the speaker deletes their profile.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| translation_content_id | uuid | nn |
| text_hash | varchar(64) | nn |
| voice_type | varchar(20) | nn, — cloned \| default |
| provider | varchar(50) | nn, default: 'cartesia' |
| provider_voice_id | varchar(255) | nn |
| previous_audio_dubbing_id | uuid |  |
| audio_url | varchar(500) |  |
| duration_ms | integer |  |
| status | varchar(20) | nn, default: 'pending' |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

Indexes: `(workspace_id, text_hash, provider_voice_id) [unique]`

### `transcript.glossaries`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| name | varchar(150) | nn |
| description | text |  |
| source_language | varchar(15) | nn |
| target_language | varchar(15) | nn |
| term_count | integer | nn, default: 0 |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

Indexes: `(workspace_id, name) [unique]`

### `transcript.glossary_terms`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| glossary_id | uuid | nn |
| source_term | varchar(255) | nn |
| target_term | varchar(255) | nn |
| context | text |  |
| domain | varchar(50) |  |
| priority | integer | nn, default: 5 |
| definition | text |  |
| usage_note | text |  |
| part_of_speech | varchar(50) |  |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

Indexes: `(glossary_id, source_term) [unique]`

### `transcript.global_glossary_terms`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| term | varchar(255) | nn |
| preferred_translation | varchar(255) | nn |
| source_language | varchar(15) | — NULL = language agnostic |
| target_language | varchar(15) | — NULL = language agnostic |
| business_domain | varchar(100) |  |
| definition | text |  |
| usage_note | text |  |
| priority | integer | nn, default: 5 |
| status | varchar(30) | nn, default: 'draft' |
| version | integer | nn, default: 1 |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `transcript.global_glossary_audits`

term_id carries no physical foreign key: the audit outlives a hard-deleted term.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| term_id | uuid | nn |
| action | varchar(30) | nn |
| before_json | jsonb |  |
| after_json | jsonb |  |
| actor_user_id | uuid | nn |
| created_at | timestamptz | nn |

## Billing

### `subscription.plans`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| name | varchar(100) | nn |
| slug | varchar(50) | nn, uq |
| tier | varchar(20) | nn |
| price | decimal(12,2) | nn |
| currency | char(3) | nn, default: 'USD' |
| billing_cycle | varchar(20) | nn, default: 'monthly', — monthly \| yearly |
| credits_per_cycle | integer | nn |
| max_participants | integer | nn, default: 2 |
| max_languages | integer | nn, default: 2 |
| max_active_rooms | integer | nn, default: 5 |
| voice_clone_enabled | boolean | nn, default: false |
| ai_assistant_enabled | boolean | nn, default: false |
| glossary_enabled | boolean | nn, default: false |
| dedicated_gpu | boolean | nn, default: false |
| overage_cap_credits | integer | nn, default: 0 |
| overage_price_per_credit | numeric(18,10) | nn, default: 0.0001520913, — in the plan currency |
| low_balance_threshold_credits | integer | nn, default: 0 |
| rollover_cap_credits | integer | nn, default: 0 |
| invoice_terms_days | integer | nn, default: 15 |
| invoice_grace_hours | integer | nn, default: 360 |
| features | jsonb | nn, default: `'{}'` |
| sort_order | integer | nn, default: 0 |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `subscription.subscriptions`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| user_id | uuid | nn |
| workspace_id | uuid |  |
| plan_id | uuid | nn |
| status | varchar(20) | nn, default: 'active' |
| credits_remaining | integer | nn, default: 0 |
| credits_used_this_cycle | integer | nn, default: 0 |
| current_period_start | timestamptz | nn |
| current_period_end | timestamptz | nn |
| auto_renew | boolean | nn, default: true |
| cancellation_reason | text |  |
| cancelled_at | timestamptz |  |
| trial_ends_at | timestamptz |  |
| entitlement_overrides | jsonb |  |
| credits_per_cycle_override | integer |  |
| contract_price_usd | numeric(14,2) | — USD, the accounting currency (was contract_price_vnd until 2026-10-02) |
| overage_cap_credits_override | integer |  |
| overage_price_per_credit_override | numeric(18,10) | — USD per credit |
| low_balance_threshold_credits_override | integer |  |
| invoice_terms_days_override | integer |  |
| billing_contact_email | varchar(255) |  |
| overage_credits_this_cycle | integer | nn, default: 0 |
| overage_started_at | timestamptz |  |
| service_state | varchar(20) | nn, default: 'healthy', — healthy \| low_balance \| in_overage \| suspended |
| suspended_reason | varchar(30) | — overage_cap \| invoice_overdue \| trial_ended |
| owner_email_domain | varchar(255) |  |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

Indexes: `workspace_id [unique, note: 'partial: WHERE is_active AND workspace_id IS NOT NULL — one live subscription per workspace']`

### `subscription.payments`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| subscription_id | uuid | nn |
| user_id | uuid | nn |
| amount | decimal(12,2) | nn |
| tax_amount | decimal(12,2) | nn |
| total_amount | decimal(12,2) | nn |
| currency | char(3) | nn, default: 'USD' |
| payment_method | varchar(30) | nn |
| provider | varchar(30) | nn, default: 'stripe' |
| provider_transaction_id | varchar(1024) | uq |
| provider_order_id | varchar(1024) |  |
| status | varchar(20) | nn, default: 'pending' |
| failure_reason | varchar(500) |  |
| provider_metadata | jsonb |  |
| paid_at | timestamptz |  |
| refunded_at | timestamptz |  |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

### `subscription.invoices`

The invoice belongs to the PAYMENT, not the reverse.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| payment_id | uuid | nn |
| user_id | uuid |  |
| workspace_id | uuid |  |
| invoice_number | varchar(30) | nn, uq |
| stripe_invoice_id | varchar(255) |  |
| amount | numeric(12,2) |  |
| subtotal | numeric(12,2) | nn |
| tax | numeric(12,2) | nn, default: 0 |
| total | numeric(12,2) | nn |
| currency | char(3) | nn, default: 'USD' |
| status | varchar(20) | nn, default: 'issued' |
| pdf_url | varchar(500) |  |
| invoice_pdf_url | varchar(500) |  |
| hosted_invoice_url | varchar(500) |  |
| line_items | jsonb | nn, default: `'[]'` |
| issued_at | timestamptz | nn |
| due_at | timestamptz | nn |
| paid_at | timestamptz |  |
| created_at | timestamptz | nn |

### `subscription.credit_transactions`

Three required owners, then six optional links describing WHY the charge happened.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| subscription_id | uuid | nn |
| user_id | uuid | nn |
| workspace_id | uuid |  |
| amount | integer | nn |
| type | varchar(20) | nn |
| charge_type | varchar(30) |  |
| description | varchar(255) |  |
| reference_id | uuid |  |
| reference_type | varchar(30) |  |
| balance_after | integer | nn |
| pricing_rate_card_id | uuid |  |
| usage_record_id | uuid |  |
| invoice_id | uuid |  |
| reversal_of_transaction_id | uuid | uq |
| unit_price_snapshot | numeric(18,6) |  |
| currency | char(3) |  |
| idempotency_key | varchar(255) |  |
| correlation_id | varchar(100) |  |
| status | varchar(20) | nn, default: 'committed' |
| triggered_by_participant_id | uuid |  |
| transcript_segment_id | uuid |  |
| created_at | timestamptz | nn |

Indexes: `(correlation_id, type) [unique, note: 'partial: WHERE correlation_id IS NOT NULL']`

### `subscription.credit_balance_snapshots`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| subscription_id | uuid | nn |
| credits_remaining | integer | nn |
| credits_used_this_cycle | integer | nn |
| snapshot_at | timestamptz | nn |

### `subscription.usage_records`

Usage is always billed to a workspace, but not always traceable to one meeting.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| subscription_id | uuid | nn |
| user_id | uuid |  |
| workspace_id | uuid | nn |
| translation_room_id | uuid |  |
| segment_id | uuid |  |
| usage_type | varchar(30) | nn |
| unit | varchar(20) | nn, default: 'credit' |
| quantity | decimal(12,4) | nn, default: 1 |
| credits_consumed | integer | nn |
| duration_seconds | integer |  |
| details | jsonb |  |
| recorded_at | timestamptz | nn |

### `subscription.usage_rate_card`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| charge_type | varchar(30) | nn |
| unit | varchar(30) |  |
| provider | varchar(50) |  |
| model | varchar(100) |  |
| source_language_code | varchar(15) |  |
| target_language_code | varchar(15) |  |
| unit_price | numeric(18,6) | nn |
| provider_unit_cost | numeric(18,10) |  |
| markup_multiplier | numeric(10,4) |  |
| currency | char(3) | nn |
| effective_from | timestamptz | nn |
| effective_to | timestamptz | — NULL = currently in force |
| is_active | boolean | nn, default: true |
| notes | text |  |

Indexes: `(charge_type, unit, currency, provider, model, source_language_code, target_language_code) [unique, note: 'partial: WHERE is_active AND effective_to IS NULL']`

### `subscription.workspace_entitlement_overrides`

| Column | Type | Constraints |
|---|---|---|
| workspace_id | uuid | nn |
| entitlement_key | varchar(60) | nn |
| value | varchar(40) | nn |
| set_by | uuid |  |
| updated_at | timestamptz | nn |

Indexes: `(workspace_id, entitlement_key) [pk]`

### `subscription.sales_inquiries`

Both ids are nullable: an inquiry can arrive before either exists.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| first_name | varchar(80) | nn |
| last_name | varchar(80) | nn |
| work_email | varchar(255) | nn |
| company | varchar(160) | nn |
| request_type | varchar(80) | nn |
| feature_interests | jsonb | nn, default: `'[]'` |
| target_languages | jsonb | nn, default: `'[]'` |
| current_monthly_meeting_volume | varchar(80) | nn |
| expected_monthly_meeting_volume_in_six_months | varchar(80) |  |
| use_case_notes | text |  |
| pricing_estimate_json | jsonb | nn, default: `'{}'` |
| consent | boolean | nn |
| source | varchar(80) | nn, default: 'landing_pricing' |
| status | varchar(30) | nn, default: 'new', — new \| reviewing \| quoted \| converted \| closed |
| workspace_id | uuid |  |
| subscription_id | uuid |  |
| created_at | timestamptz | nn |
| updated_at | timestamptz |  |
| converted_at | timestamptz |  |
| closed_at | timestamptz |  |

## Assistant

### `assistant.assistant_conversations`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| workspace_id | uuid | nn |
| user_id | uuid | nn |
| title | varchar(255) | nn, default: 'New chat' |
| context_scope | text |  |
| created_at | timestamptz | nn |
| last_message_at | timestamptz |  |
| is_archived | boolean | nn, default: false |

### `assistant.assistant_messages`

user_id is nullable: the assistant own replies have no user.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| conversation_id | uuid | nn |
| workspace_id | uuid | nn |
| user_id | uuid |  |
| role | varchar(20) | nn, — user \| assistant \| system \| tool |
| content | text | nn, default: '' |
| tool_calls_json | text |  |
| tool_results_json | text |  |
| status | varchar(20) | nn, default: 'completed' |
| created_at | timestamptz | nn |
| completed_at | timestamptz |  |

### `assistant.assistant_tool_calls`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| message_id | uuid | nn |
| tool_name | varchar(100) | nn |
| arguments_json | text | nn |
| status | varchar(20) | nn, default: 'pending' |
| result_json | text |  |
| created_at | timestamptz | nn |
| completed_at | timestamptz |  |

## Notification

### `notification.notification_messages`

PARTITION BY RANGE (created_at): partitions _y2025, _y2026, _y2027, _default. created_at is in the primary key because Postgres requires the partition key there.

| Column | Type | Constraints |
|---|---|---|
| id | uuid | nn |
| created_at | timestamptz | nn |
| user_id | uuid | nn |
| type | varchar(50) | nn |
| title | varchar(255) | nn |
| content | text | nn |
| action_url | varchar(500) |  |
| payload_json | jsonb | nn, default: `'{}'` |
| is_read | boolean | nn, default: false |
| read_at | timestamptz |  |

Indexes: `(id, created_at) [pk]`

### `notification.notification_preferences`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| user_id | uuid | nn |
| notification_type | varchar(50) | nn |
| email_enabled | boolean | nn, default: true |
| push_enabled | boolean | nn, default: true |
| in_app_enabled | boolean | nn, default: true |
| updated_at | timestamptz | nn |

Indexes: `(user_id, notification_type) [unique]`

### `notification.notification_templates`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| type | varchar(50) | nn |
| channel | varchar(20) | nn |
| subject | varchar(255) |  |
| body_template | text | nn |
| variables | jsonb | nn, default: `'[]'` |
| is_active | boolean | nn, default: true |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

Indexes: `(type, channel) [unique]`

### `notification.push_subscriptions`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| user_id | uuid | nn |
| device_token | varchar(500) | nn, uq |
| platform | varchar(20) | nn |
| device_name | varchar(100) |  |
| is_active | boolean | nn, default: true |
| last_used_at | timestamptz |  |
| created_at | timestamptz | nn |
| created_by | uuid |  |
| updated_at | timestamptz |  |
| updated_by | uuid |  |
| deleted_at | timestamptz |  |
| deleted_by | uuid |  |

### `notification.admin_notifications`

| Column | Type | Constraints |
|---|---|---|
| id | uuid | pk |
| title | varchar(255) | nn |
| content | text | nn |
| type | varchar(50) | nn |
| payload | jsonb | nn, default: `'{}'` |
| target_audience_mode | varchar(50) | nn |
| target_audience_data | jsonb | nn, default: `'{}'` |
| status | varchar(50) | nn, — Pending \| Sent \| Failed |
| sent_at | timestamptz | — set when the last delivery chunk is written |
| delivered_count | int | nn, default: 0 |
| delivery_chunk_count | int | nn, default: 1 |
| delivered_chunk_count | int | nn, default: 0 |
| created_by | uuid | nn |
| updated_by | uuid |  |
| created_at | timestamptz | nn |
| updated_at | timestamptz | nn |

## Relationships

`>` many-to-one, `-` one-to-one. Cross-schema references are logical (ID carried, consistency by events).

- `auth.user_settings.user_id - auth.users.id`
- `auth.refresh_tokens.user_id > auth.users.id`
- `auth.user_roles.user_id > auth.users.id`
- `auth.user_roles.role_id > auth.roles.id`
- `auth.role_permissions.role_id > auth.roles.id`
- `auth.role_permissions.permission_id > auth.permissions.id`
- `voice.voice_profiles.user_id > auth.users.id`
- `voice.voice_profiles.workspace_id > workspace.workspaces.id`
- `voice.voice_samples.voice_profile_id > voice.voice_profiles.id`
- `workspace.workspaces.owner_id > auth.users.id`
- `workspace.workspace_members.workspace_id > workspace.workspaces.id`
- `workspace.workspace_members.user_id > auth.users.id`
- `workspace.workspace_members.role_id > auth.roles.id`
- `workspace.workspace_invitations.workspace_id > workspace.workspaces.id`
- `workspace.workspace_invitations.role_id > auth.roles.id`
- `workspace.workspace_invitations.matched_domain_id > workspace.workspace_verified_domains.id`
- `workspace.workspace_verified_domains.workspace_id > workspace.workspaces.id`
- `workspace.workspace_documents.workspace_id > workspace.workspaces.id`
- `workspace.workspace_documents.owner_id > auth.users.id`
- `workspace.workspace_documents.source_id > workspace.workspace_documents.id`
- `workspace.workspace_document_access_policies.document_id > workspace.workspace_documents.id`
- `workspace.workspace_document_audits.document_id > workspace.workspace_documents.id`
- `workspace.workspace_document_audits.actor_id > auth.users.id`
- `workspace.workspace_entitlement_snapshots.workspace_id - workspace.workspaces.id`
- `workspace.workspace_admin_actions.workspace_id > workspace.workspaces.id`
- `translation_room.translation_room_series.workspace_id > workspace.workspaces.id`
- `translation_room.translation_room_series.host_id > auth.users.id`
- `translation_room.translation_rooms.series_id > translation_room.translation_room_series.id`
- `translation_room.translation_rooms.workspace_id > workspace.workspaces.id`
- `translation_room.translation_rooms.host_id > auth.users.id`
- `translation_room.translation_room_participants.translation_room_id > translation_room.translation_rooms.id`
- `translation_room.translation_room_participants.user_id > auth.users.id`
- `translation_room.translation_room_invitations.translation_room_id > translation_room.translation_rooms.id`
- `translation_room.translation_room_sessions.translation_room_id > translation_room.translation_rooms.id`
- `translation_room.translation_room_artifacts.translation_room_id > translation_room.translation_rooms.id`
- `translation_room.translation_room_feedback.translation_room_id > translation_room.translation_rooms.id`
- `translation_room.translation_room_feedback.user_id > auth.users.id`
- `translation_room.translation_room_audio_routes.translation_room_id > translation_room.translation_rooms.id`
- `translation_room.translation_room_audio_routes.source_participant_id > translation_room.translation_room_participants.id`
- `translation_room.translation_room_audio_routes.target_participant_id > translation_room.translation_room_participants.id`
- `meeting.meeting_rooms.translation_room_id > translation_room.translation_rooms.id`
- `meeting.meeting_rooms.active_host_id > auth.users.id`
- `meeting.rtc_stream_participants.meeting_room_id > meeting.meeting_rooms.id`
- `meeting.rtc_stream_participants.user_id > auth.users.id`
- `meeting.meeting_tracks.meeting_participant_id > meeting.rtc_stream_participants.id`
- `meeting.rtc_session_revocations.meeting_room_id > meeting.meeting_rooms.id`
- `meeting.rtc_session_revocations.invitee_user_id > auth.users.id`
- `meeting.meeting_chat_messages.meeting_room_id > meeting.meeting_rooms.id`
- `meeting.meeting_chat_messages.sender_user_id > auth.users.id`
- `meeting.meeting_chat_translations.message_id > meeting.meeting_chat_messages.id`
- `meeting.meeting_chat_assistant_requests.trigger_message_id > meeting.meeting_chat_messages.id`
- `transcript.transcripts.workspace_id > workspace.workspaces.id`
- `transcript.transcripts.translation_room_id > translation_room.translation_rooms.id`
- `transcript.transcripts.previous_transcript_id > transcript.transcripts.id`
- `transcript.transcript_segments.transcript_id > transcript.transcripts.id`
- `transcript.transcript_segments.speaker_participant_id > translation_room.translation_room_participants.id`
- `transcript.transcript_segments.matched_segment_id > transcript.transcript_segments.id`
- `transcript.transcript_clean_sentences.transcript_id > transcript.transcripts.id`
- `transcript.transcript_corrections.segment_id > transcript.transcript_segments.id`
- `transcript.transcript_corrections.user_id > auth.users.id`
- `transcript.transcript_corrections.translation_content_id > transcript.translation_contents.id`
- `transcript.transcript_exports.transcript_id > transcript.transcripts.id`
- `transcript.transcript_exports.user_id > auth.users.id`
- `transcript.translation_contents.workspace_id > workspace.workspaces.id`
- `transcript.translation_contents.previous_translation_content_id > transcript.translation_contents.id`
- `transcript.segment_translation_links.segment_id > transcript.transcript_segments.id`
- `transcript.segment_translation_links.translation_content_id > transcript.translation_contents.id`
- `transcript.audio_dubbings.translation_content_id > transcript.translation_contents.id`
- `transcript.audio_dubbings.previous_audio_dubbing_id > transcript.audio_dubbings.id`
- `transcript.glossaries.workspace_id > workspace.workspaces.id`
- `transcript.glossary_terms.glossary_id > transcript.glossaries.id`
- `transcript.global_glossary_audits.term_id > transcript.global_glossary_terms.id`
- `transcript.global_glossary_audits.actor_user_id > auth.users.id`
- `subscription.subscriptions.plan_id > subscription.plans.id`
- `subscription.subscriptions.workspace_id > workspace.workspaces.id`
- `subscription.subscriptions.user_id > auth.users.id`
- `subscription.payments.subscription_id > subscription.subscriptions.id`
- `subscription.payments.user_id > auth.users.id`
- `subscription.invoices.payment_id > subscription.payments.id`
- `subscription.invoices.user_id > auth.users.id`
- `subscription.credit_balance_snapshots.subscription_id > subscription.subscriptions.id`
- `subscription.usage_records.subscription_id > subscription.subscriptions.id`
- `subscription.usage_records.workspace_id > workspace.workspaces.id`
- `subscription.usage_records.translation_room_id > translation_room.translation_rooms.id`
- `subscription.usage_records.segment_id > transcript.transcript_segments.id`
- `subscription.credit_transactions.subscription_id > subscription.subscriptions.id`
- `subscription.credit_transactions.workspace_id > workspace.workspaces.id`
- `subscription.credit_transactions.user_id > auth.users.id`
- `subscription.credit_transactions.pricing_rate_card_id > subscription.usage_rate_card.id`
- `subscription.credit_transactions.usage_record_id > subscription.usage_records.id`
- `subscription.credit_transactions.invoice_id > subscription.invoices.id`
- `subscription.credit_transactions.transcript_segment_id > transcript.transcript_segments.id`
- `subscription.credit_transactions.reversal_of_transaction_id > subscription.credit_transactions.id`
- `transcript.transcript_corrections.reversal_credit_transaction_id > subscription.credit_transactions.id`
- `subscription.workspace_entitlement_overrides.workspace_id > workspace.workspaces.id`
- `subscription.sales_inquiries.workspace_id > workspace.workspaces.id`
- `subscription.sales_inquiries.subscription_id > subscription.subscriptions.id`
- `assistant.assistant_conversations.workspace_id > workspace.workspaces.id`
- `assistant.assistant_conversations.user_id > auth.users.id`
- `assistant.assistant_messages.conversation_id > assistant.assistant_conversations.id`
- `assistant.assistant_messages.user_id > auth.users.id`
- `assistant.assistant_tool_calls.message_id > assistant.assistant_messages.id`
- `notification.notification_preferences.user_id > auth.users.id`
- `notification.notification_messages.user_id > auth.users.id`
- `notification.push_subscriptions.user_id > auth.users.id`

Ungrouped tables: transcript.transcript_clean_sentences
