# 12. Transcript Service (code-derived spec)

Source repo: https://github.com/WarpTalk-CapstoneProject/warptalk-backend
Commit: c51b39b
Scope: `transcript/` (API, Application, Domain, Infrastructure), `shared/WarpTalk.Shared` (protos, ErrorCodes, FarSpeakerNames), `transcript/database/migrations`
Notation: `TS` = `transcript/src/WarpTalk.TranscriptService.`. Key words MUST/SHOULD describe behaviour enforced in code. Tests, DTOs and generated files were not read.

## 1. Responsibilities and bounded context

The service persists what the live pipeline produces and serves it afterwards. It owns the transcript record (segments, clean sentences, corrections, exports, pause windows), the translation content store with per-segment links, audio dubbing records, workspace glossaries and the platform global glossary, and the post-meeting far-speaker relabel. It does NOT produce STT, translation or TTS (AI workers do), does not own rooms or membership (Translation Room and Workspace services, via gRPC) and does not meter billing (billing_worker, per comments at `TranscriptRedisConsumerService.cs:615,842`). No minutes or summary entity exists here; the summary is produced by the assistant worker after the `__MEETING_END__` marker (see 11-meeting-service.md).

Ingestion is Redis-stream driven. There is no SaveSegment RPC; gRPC is read-only.

## 2. Hosting

| Item | Behaviour |
|---|---|
| Ports | HTTP 5103, gRPC (HTTP/2) 50053 (`Program.cs`). |
| Redis | `Redis:ConnectionString` is REQUIRED. Consumers retry their consumer groups with bounded backoff (2 s doubling to 30 s) so the service starts while Redis is down. |
| Auth | JWT bearer plus the shared staff (system-admin) authorization policy for `api/v1/admin/*` (WT-205). |
| gRPC clients | Auth (user lookup), TranslationRoom, Billing, Workspace, Admin audit (`AddWarpTalkAdminAuditing`), MeetConference (far-speaker source). |
| Hosted services | `TranscriptRedisConsumerService`, `GlossaryStartedEventConsumer`, `GlossaryIndexResultConsumer`, `FarSpeakerRelabelWorker`. |

## 3. REST API

All routes require JWT unless stated. Access rules are in section 5.1.

| Method and route | Auth | Purpose |
|---|---|---|
| `GET api/v1/transcripts/{id}` | read access | Transcript header. |
| `GET api/v1/transcripts/by-room/{translationRoomId}` | read access | Current transcript of a room. |
| `POST api/v1/transcripts/{id}/finalize` | effective host | Lock the transcript (204). |
| `POST .../by-room/{roomId}/pause`, `.../resume` | host (bridge: capturer too) | Pause or resume recording of the transcript (WT-605). |
| `GET .../by-room/{roomId}/pause-windows` | read access | Pause windows. |
| `GET api/v1/transcripts/{tid}/segments` | read access | Segments. |
| `GET api/v1/transcripts/{tid}/clean-sentences` | read access | Tier-2 clean sentences (WT-716). |
| `POST api/v1/transcripts/{tid}/segments/{sid}/correct` | see 5.2 | Submit correction (201). |
| `GET .../segments/{sid}/corrections` | read access | Correction history. |
| `POST api/v1/transcripts/{tid}/exports`, `GET .../exports/{id}/download` | read access | Create and download an export. |
| `GET api/v1/transcripts/{tid}/translations`, `GET .../translations/coverage`, `POST .../translations/backfill` | read access | Translation list, per-language coverage, post-meeting backfill (429 on budget exhaustion). |
| Glossaries: `GET global`; `POST`; `GET/PUT/DELETE {id}`; `GET workspace/{workspaceId}`; `PUT {id}/languages`; `GET {id}/warpbot-status`; `GET/POST {id}/terms`; `POST {id}/terms/bulk`; `PUT/DELETE {id}/terms/{termId}` (base `api/v1/glossaries`) | workspace member to read; workspace Owner or Admin to write | Workspace glossary CRUD. `GET global` returns published global terms only. |
| Global glossary (base `api/v1/admin/global-glossary`): `GET`, `GET {id}`, `POST`, `PUT {id}`, `DELETE {id}`, `POST {id}/publish`, `POST {id}/archive`, `POST bulk-import`, `GET {id}/audits` | system admin (shared staff policy) | Platform terms with audit trail. |

Error mapping used by controllers: NOT_FOUND 404, FORBIDDEN or UNAUTHORIZED 403, BAD_REQUEST 400, CONFLICT 409, translation budget 429, otherwise 500. Note that the correction and finalize services return "UNAUTHORIZED" for authorization failures, which controllers map to 403.

### 3.1 gRPC server `TranscriptGrpcService` (`transcript.proto`)

`GetTranscriptById`, `GetTranscriptsByTranslationRoomId`, `GetTranscriptSegments`, `GetTranscriptTranslations`. Read-only. No hub exists.

## 4. Domain model and state machines

Schema `transcript`.

| Entity | Key fields and rules |
|---|---|
| Transcript | WorkspaceId, TranslationRoomId, Version, Status, SourceLanguage, TotalSegments, TotalDurationMs, IsActive, FinalizedAt, IsCurrent (head pointer, unique per room), PreviousTranscriptId, EngineVersion, TimelineAnchorAt (WT-473, set once), LastSequenceOrder, soft delete. |
| TranscriptSegment | SpeakerParticipantId, SpeakerName, OriginalText/Language, StartTimeMs, EndTimeMs, Confidence (nullable; avg_logprob, NULL = unknown, WT-277), SequenceOrder, IsCorrected, IsFinal, CleanText/CleanFlags, FarSpeakerKey/Source/Confidence. |
| TranscriptCleanSentence | Id = producer `sentence_id`, SegmentIds Guid[] (no FK), CleanText, Language, Flags, Source, Revision, StartTimeMs, ProducedAt. A higher revision replaces the row; lower or equal is ignored. |
| TranscriptCorrection | SegmentId, UserId, Original/CorrectedText, CorrectionType (STT or MT), Status (Pending/Approved/Rejected), TriggeredRetranslation, ReviewedBy/At, TranslationContentId, ReversalCreditTransactionId. |
| TranscriptExport | TranscriptId, UserId, Format, FileUrl (internal download path), IncludedLanguages (json). |
| TranscriptPauseWindow | TranslationRoomId, StartedAt, EndedAt, PausedBy, ResumedBy. Display metadata only. |
| TranslationContent | Dedup key (WorkspaceId, TextHash = md5, TargetLanguage); TranslatedText, TranslatorModel, SourceSttConfidence (STT signal, not translation quality, WT-278), IsRetranslated, PreviousTranslationContentId, LatencyMs, Status. |
| SegmentTranslationLink | Key (SegmentId, TranslationContentId), TargetLanguage, IsCurrent (at most one per segment and language), IsStale (WT-704), DeliveredAt. |
| AudioDubbing | WorkspaceId, TranslationContentId, TextHash, VoiceType (cloned/default), Provider (default `cartesia`), ProviderVoiceId, PreviousAudioDubbingId, AudioUrl, DurationMs, Status. Dedup (workspace, text_hash, provider_voice_id). |
| Glossary / GlossaryTerm | Workspace scope; Glossary has Name, SourceLanguage, TargetLanguage, TermCount, IsActive; term has SourceTerm, TargetTerm, Context, Domain, Definition, UsageNote, PartOfSpeech, Priority, IsActive. |
| GlobalGlossaryTerm | Term, PreferredTranslation, languages, BusinessDomain, Definition, UsageNote, Priority, Status draft/published/archived, Version. |
| GlobalGlossaryAudit | TermId, Action (created/updated/published/archived/deleted), BeforeJson, AfterJson, ActorUserId. |
| FarSpeakerRelabelJob | TranslationRoomId, Status (pending/done/no_transcript/abandoned/skipped), Attempts, NextAttemptAt, RoomEndedAt, LastError, SegmentsRelabeled, CompletedAt. |

### 4.1 Transcript status

`Recording` -> `Finalizing` -> `Finalized` -> `Archived` is the enum. In code the only transitions written are: a new head is created by ingestion, and `FinalizeTranscriptAsync` sets `FINALIZED` (IsActive=false, FinalizedAt). Repeating finalize on a FINALIZED transcript is a success no-op; ARCHIVED cannot be finalized (BAD_REQUEST). No code path was found that writes FINALIZING or ARCHIVED.

### 4.2 Correction and translation link

STT correction: segment text replaced, CleanText cleared, all current links marked IsStale, retranslation requested. MT correction: stores a human translation, replaces the current link for that language.

## 5. Business rules

### 5.1 Read access (`TranscriptReadAccess`, BR-55)

| Rule | Detail |
|---|---|
| Host | The effective host (follows host transfer) MUST always be admitted. |
| Live meeting | A roster participant is admitted while the meeting runs. |
| Ended meeting | A participant is admitted only while the room's ArtifactAccess says the record is shared. |
| WT-849 | An invited-but-absent user is admitted only after the meeting has ended and ArtifactAccess is ALL_PARTICIPANTS. |
| Pause or resume | Narrower: host only, plus the bridge capturer in EXTERNAL_BRIDGE rooms (`TranscriptPauseAccess`). Pause when paused or resume when not paused: INVALID_STATE. |

### 5.2 Corrections

- Segment MUST belong to the transcript (else NOT_FOUND).
- ARCHIVED or FINALIZED transcripts MUST refuse corrections (BAD_REQUEST). This is enforced on the server, not only in the web UI.
- Caller MUST have read access (else UNAUTHORIZED).
- MT corrections require TargetLanguage, MUST be made by the effective host, and the language (normalized, `en-US` to `en`) MUST be allowed by the room policy (L2 intersect L1, WT-704) else `TranscriptLanguageErrors.LanguageNotAllowed`.
- STT correction of a segment with links triggers a retranslation request (failures are logged, never fail the edit).

### 5.3 Finalize

Caller MUST be the room's effective host, resolved by gRPC `GetTranslationRoomById` (falls back to HostId for older rooms). Room not found: NOT_FOUND.

### 5.4 Ingestion filters

| Rule | Detail |
|---|---|
| Empty text | The trailing empty `is_final_chunk` marker is acknowledged and not stored. |
| Control markers | Text matching `^__[A-Z0-9_]+__` or language `system` is never stored, counted or translated. |
| SaveTranscript false (WT-587) | Ephemeral rooms are acknowledged without persisting (result TRUE so no dead letter). Cached 4 h per room. |
| Paused (WT-605) | While Redis set `translationRoom:{roomId}:transcript_paused_segments` gate is active, segments are not stored; their ids are remembered (TTL 30 min) so later translate or TTS results for them are treated as intentional skips. |
| Transcript head | Created on first segment from gRPC room details; linked to the previous transcript of the room. |
| Bridge speakers | Stand-in lines are saved under the live far-speaker name only when confidence reaches the shared threshold (`FarSpeakerNames`). |
| Billing | Segments carry `IsFinal`; metering is done elsewhere. |

### 5.5 Translation backfill and coverage

- Locked (FINALIZED or ARCHIVED) transcripts MUST refuse backfill.
- TargetLanguage required (VALIDATION_ERROR); no access: FORBIDDEN.
- At most 3000 missing segments per request (TOO_LARGE); at most 2 x 3000 queued segments per transcript per budget window, else `TRANSLATION_BUDGET_EXHAUSTED` (HTTP 429).
- Segments are sent 20 per request on stream `translate:backfill_requests` (approx. max length 10 000). Coverage status: idle, running, complete, failed (marker is a UI hint, not a lock).
- Results come back on `translate:backfill_results`, a separate stream so that tts_worker does not dub them.

### 5.6 Exports

Format is stored lower-cased; code generates `csv` (text/csv) and otherwise falls back to plain text (`transcript_{id}.txt`). Content is segments ordered by SequenceOrder with timestamps, speaker, language, text and the requested translations. Report 7 formats PDF, DOCX, VTT and SRT are NOT generated. The export row's FileUrl is the service's own download route; no object storage is used.

### 5.7 Glossaries

| Rule | Detail |
|---|---|
| Workspace glossary | Reads need active workspace membership (403 "not an active member"); writes need Owner or Admin (403). Languages MUST be codes such as en, vi, ja (BAD_REQUEST). Duplicate term: CONFLICT. Over-long value: BAD_REQUEST (terms and translations 200 chars, field 50). Bulk import is all-or-nothing on duplicates. |
| Global glossary | Admin only. Duplicate (term, source, target, domain): BAD_REQUEST. Publishing requires a definition and is capped at 200 published terms (prompt-injection bound). Every change writes a GlobalGlossaryAudit row. |
| Use at runtime | On `meeting.started`, `GlossaryStartedEventConsumer` merges workspace terms with published global terms (workspace wins on key collision; workspace may opt out through the AI usage policy `UseGlobalGlossary`; global side gated by `GlobalGlossary:Enabled`) and writes Redis keys `translationRoom:{roomId}:stt_prompt`, `:stt_keywords` (also participant names, WT-422) and `:mt_glossary`, each with TTL 24 h. |

## 6. Integration

### 6.1 Streams consumed (`TranscriptRedisConsumerService`)

Consumer group `transcript-persistence`, consumer name `transcript-{machine}-{guid}`, read count 10, idle poll 250 ms, metrics key `transcript:persistence:consumer:metrics`.

| Stream | Kind | Payload fields used |
|---|---|---|
| `stt:results` | STT segment | segment_id, meeting_id (room), speaker_id, text, language, start_ms, end_ms, anchor_ms, confidence, is_final_chunk (`1`), clean_flags, far_speaker_* |
| `translate:results` | Translation | segment_id, target_lang, translated_text, translator_model, latency_ms, is_retranslated, previous_translation_content_id, source_stt_confidence |
| `translate:backfill_results` | Translation (same path) | as above |
| `tts:results` | Dubbing | segment_id, target_lang, voice_type, provider_voice_id, clone_provider, anchor_provider, duration_ms |
| `transcript:clean` | Clean sentence | sentence_id, revision, clean_text, segment_ids, language, flags, source, timestamp_ms, meeting_id |
| far-speaker late-name stream (`FarSpeakerNames.LateNameStream`) | Late bridge names | segment_id, far_speaker_name, far_speaker_source, far_speaker_confidence |

Reliability: pending messages are claimed after 1 min idle every 30 s (batch 10); after 5 delivery attempts a message moves to `{stream}:transcript-persistence:dead-letter` (approx. capped). Dead consumers are removed after 1 h idle, with housekeeping every 5 min.

### 6.2 Other Redis and consumers

| Name | Direction | Purpose |
|---|---|---|
| `meeting.started` (pub/sub) | consume | Glossary publication. |
| `embedding:index_requests` (stream, max 10 000) | publish | One entry per persisted segment: job_id, workspace_id, collection_id `workspace_{id}`, source_type `transcript`, source_id, chunks_json, external_llm_allowed, ai_retrieval_allowed, retention_state, deletion_state, timestamp_ms. External-LLM flag is cached 5 min per workspace. |
| `embedding:index_results` (group `transcript-glossary-index`) | consume | Glossary index results (`GlossaryIndexResultConsumer`). |
| `translate:backfill_requests` | publish | See 5.5. |
| Gateway commands | publish | Pause and resume state change commands. |

### 6.3 gRPC and events

gRPC clients: TranslationRoom (room details, effective host, roster, people names), Workspace (membership, AI policy), Auth (user names), Billing (registered), MeetConference (post-meeting far-speaker names), admin audit. No MassTransit or RabbitMQ usage exists. Domain events are not published; only the Redis streams above.

## 7. Background jobs

| Job | Behaviour |
|---|---|
| `TranscriptRedisConsumerService` | Section 6.1, per replica. |
| `GlossaryStartedEventConsumer` | Subscribes to `meeting.started`; retry 2 s doubling to 30 s; participant name lookup timeout 3 s. |
| `GlossaryIndexResultConsumer` | `BaseRedisConsumer` stream consumer. |
| `FarSpeakerRelabelWorker` | Every 1 min under lock `transcript:far-speaker-relabel` (lease 5 min); processes `far_speaker_relabel_jobs` (retry with attempts, gives up 30 days after the room ended). The host's choice always wins over `google_transcript`. |

## 8. Configuration

| Key | Default / note |
|---|---|
| `ConnectionStrings:TranscriptDb` | Postgres, search path `transcript,public`; credentials supplied by environment |
| `Redis:ConnectionString` | required |
| `GrpcUrls:AuthServiceUrl`, `TranslationRoomServiceUrl`, `BillingServiceUrl`, `WorkspaceServiceUrl`, `AssistantServiceUrl` | local dev ports 50051, 50052, 50057, 50056, 50058 |
| `GlobalGlossary:Enabled` | true |
| `Jwt:Issuer`, `Audience`, `Secret` | `WarpTalk.AuthService`, `WarpTalk`, secret supplied by environment |
| Far-speaker min confidence | `FarSpeakerNames.MinConfidenceConfigKey`, default `FarSpeakerNames.DefaultMinConfidence` |
| Code constants | pending claim idle 1 min, recovery 30 s, max deliveries 5, workspace policy cache 5 min, room retention cache 4 h, pause skip TTL 30 min, prompt TTL 24 h, backfill 20 per request and 3000 per run, global glossary cap 200 published |

## 9. Error codes

Codes used: NOT_FOUND, FORBIDDEN, UNAUTHORIZED (mapped to 403), BAD_REQUEST, VALIDATION_ERROR, CONFLICT, INVALID_STATE, TOO_LARGE, INTERNAL_ERROR, `TRANSLATION_BUDGET_EXHAUSTED` (429), `TranscriptLanguageErrors.LanguageNotAllowed` (WT-704). Unlike the Meeting service, this service uses BAD_REQUEST and INTERNAL_ERROR rather than INVALID_STATE/INTERNAL_SERVER_ERROR in most places.

## 10. Migrations (`transcript/database/migrations`)

| Date | Change |
|---|---|
| 2026-08-06 09:02 | `quarantine_translation_stt_confidence` (WT-277/278/294): moves STT confidence out of translation quality semantics (expand half) |
| 2026-08-08 13:00 | `seed_global_glossary_proper_nouns`: seed of global glossary terms |
| 2026-08-17 16:00 | `persist_the_transcript_timeline_anchor` (WT-473): timeline anchor on transcripts |
| 2026-09-03 13:48 | `add_transcript_pause_windows` (WT-605) |
| 2026-09-17 07:20 | `add_transcript_clean_text` (WT-716): clean text and clean sentences |
| 2026-09-18 02:50 | `add_segment_translation_link_is_stale` (WT-704) |
| 2026-10-01 12:00 | `add_segment_far_speaker`: far-speaker columns on segments |
| 2026-10-01 12:01 | `add_far_speaker_relabel_jobs`: relabel job table |

## 11. Differences vs Report 7 (06-service-design.md section 5)

| # | Report 7 | Code |
|---|---|---|
| 1 | Inbound gRPC `SaveSegment` from translation-room | No. Segments are persisted from Redis streams; gRPC is read-only with four getters. |
| 2 | Status FINALIZING -> FINALIZED -> ARCHIVED | Enum has all four values, but code writes only FINALIZED; FINALIZING and ARCHIVED are never set. |
| 3 | Export PDF, DOCX, TXT, VTT, SRT with IncludeAudio | Only csv and txt are generated; no audio export. |
| 4 | Corrections are separate records | True, plus: STT correction marks links stale, MT correction host-only with language policy, locked after finalize. |
| 5 | Finalize by host via GetTranslationRoomById | True, using the effective host. |
| 6 | Global glossaries managed by System Admin | True; plus audit table, publish and archive, 200 published cap, seeded terms. |
| 7 | Not described | Pause and resume with pause windows, clean sentences, translation backfill and coverage, translation dedup and link model, audio dubbing records, far-speaker relabel, embedding index publication, per-workspace glossary prompts to STT and MT. |
| 8 | Not described | Read access depends on ArtifactAccess after the meeting (BR-55, WT-849). |

## 12. Gaps

Not read in depth: `TranscriptQueryService`, `GlossaryWarpBotStatus`, `TranscriptRoomLanguagePolicy` internals, the full body of `FarSpeakerRelabelWorker` and `FarSpeakers/*`, `UnitOfWork`, the `.agents` diagrams and `transcript_batching_plan.md`, proto files, migration bodies beyond headers, Dockerfile and deployment config. Whether any path writes FINALIZING or ARCHIVED was checked only by targeted grep of the services above.
