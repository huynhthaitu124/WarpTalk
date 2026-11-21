# 13 - Voice Clone and Translation Models

Sources (read-only, shallow clones): `WarpTalk-CapstoneProject/warptalk-ai` @ `d0db9b9`, `warptalk-backend` @ `c51b39b` (sparse), `warptalk-web` @ `e1dd35d` (sparse, `src/` only).
Paths are relative to warptalk-ai unless prefixed `backend:`. Line numbers refer to those commits. No secrets, keys or hostnames are reproduced. Latency budgets and measurements are out of scope here: see [08-realtime-latency.md](08-realtime-latency.md). The live suggestion agent is in [09-ai-suggestion-agent.md](09-ai-suggestion-agent.md).

## 1. Scope and pipeline position

```
stt:results -> translation_worker (split sentences, translate)  -> translation results stream
                                   -> tts_worker (voice select, synthesize, prosody) -> LiveKit / client audio
backend auth: voice-consent + voice-profiles  --(Redis)-->  tts_worker clone / delete
```

- Translation: `translation_worker/translator.py` (`OpenAITranslator`) driven by `translation_worker/worker.py`.
- Speech: `tts_worker/worker.py` plus `tts_worker/synthesizer.py` (Cartesia), `shared/prosody.py`, `shared/isochrony.py`, `shared/clone_sample_quality.py`.
- Enrollment, consent and profile storage: backend auth service (section 9).
- Settings: `TranslationSettings` and `TTSSettings` in `shared/config.py:594-900`.

## 2. Real-time translation

### 2.1 Provider and models

Single provider by design: there is no cross-provider fallback; resilience is inside OpenAI (`translator.py`, class docstring). Default `TRANSLATION_PROVIDER=openai`.

| Path | Model (default) | Role |
|---|---|---|
| Primary: OpenAI Realtime over a warm WebSocket pool, text-only output | `gpt-realtime-2.1-mini` | `_translate_realtime`, `translator.py:515`; `conversation: none` (stateless), `reasoning.effort`, `max_output_tokens` |
| Fallback: Chat Completions | `gpt-5.4-nano` | `_create_with_retry`: 2 retries, backoff 0.5 s doubling, then reduced `completion_options` |

Settings (`shared/config.py:594-656`, env prefix `TRANSLATION_`):

| Knob | Default | Meaning |
|---|---|---|
| `provider` | `openai` | Only value implemented |
| `model` | `gpt-5.4-nano` | Chat Completions fallback |
| `realtime_model` | `gpt-realtime-2.1-mini` | Primary |
| `realtime_reasoning_effort` | `minimal` | Reasoning level for the realtime path |
| `realtime_pool_size` | 4 | Hot WebSocket connections |
| `realtime_timeout_seconds` | 2.0 | Per-call bound before falling to Chat Completions |
| `realtime_max_output_tokens` | 128 | Cap per sentence |
| `max_tokens` | 512 | Chat Completions cap |
| `temperature` | 0.0 | Deterministic output |

Reasoning and sampling options are filtered per model family through `shared.openai_options.completion_options`, so a parameter a model rejects is dropped rather than failing the call.

### 2.2 Streaming mode

There is no token-level partial streaming to the client. "Streaming" means sentence-level pipelining of one STT result:
- `split_into_sentences` (`translation_worker/worker.py:608`) cuts the result.
- Sentence 0 is translated alone, so its dub can start while the remainder is processed.
- Sentences 1..N-1 are sent in a single `translate_batch` call.
- Each target language is translated independently; the language set comes from the room's language hash.

### 2.3 Prompt and context window

| Element | Source | Detail |
|---|---|---|
| Recent speech | Last 3 source utterances per room (`worker.py:309-312`) | Only the last 4 context lines reach the prompt (`meeting_context[-4:]`) |
| Relevance filter | `_select_relevance_context` (`worker.py:110`) | Drops context lines unrelated to the current utterance |
| Meeting context | `_get_meeting_context` | Static title and description, fetched once and cached |
| Glossary injection | Redis `translationRoom:{id}:mt_glossary` (`_get_mt_glossary`, `worker.py:1147`) | `_select_relevant_glossary_terms` injects only terms present in the utterance; a fuzzy sounds-like match is flagged "possible" and the prompt carries an ASR-repair clause so the model may fix a mis-heard term toward the glossary form |
| Scope marker | `OUT_OF_MEETING_SCOPE` | Model returns this marker for off-topic/ambient speech and the worker suppresses the dub |
| Valence marker | `shared/valence.py` | Model appends a trailing emotion token; `valence_mod.split_valence` strips it from text and forwards it to TTS (section 8) |

### 2.4 Language pairs

- Source and target codes are normalised to the base subtag (`vi-VN` to `vi`); equal languages skip translation.
- Names come from `shared/languages.py` `LANGUAGE_NAMES`, which mirrors the web `SUPPORTED_LANGUAGES`; the model is prompted with the language name, so any pair in that table is accepted (no per-pair allow-list).
- Same-language passthrough: `polish` plus `choose_transcript` (`transcript_guardian.py`) accepts the polished text only if a subsequence check says it stays faithful to the original; otherwise the raw transcript is kept.

### 2.5 Skips and failure handling

| Condition | Behaviour |
|---|---|
| Meeting not started | `translation_skipped_not_started`, no model call |
| Credits exhausted | `credits_exhausted`, no model call |
| Realtime timeout/error | Fall to Chat Completions (2.1) |
| Chat Completions exhausted | Error result; the source transcript is still delivered (translation is additive) |
| External-LLM policy | See the policy gate in [09](09-ai-suggestion-agent.md) section 8 for the analogous consent gate; whether translation reads `ai_policy` was not determined |

## 3. TTS providers and models

Single provider: Cartesia (`tts_worker/synthesizer.py`).

| Item | Value |
|---|---|
| Model | `sonic-3.5` |
| Sample rate | 44100 Hz |
| Speed baseline | `fast` |
| Transport | WebSocket with a warm connection pool (`warm_up`) |
| Operations | `synthesize`, `open_prosody_context`, `clone_voice` (via `voices.clone`), `delete_voice`, `list_voices_by_language` |
| Concurrency | `generation_slot` bounds in-flight generations (`cartesia_max_concurrency`) |

Output goes to LiveKit when `stream_to_livekit` is true, otherwise to the client over the results stream.

## 4. Voice sources and selection

`_get_voice_id` (`tts_worker/worker.py:2874`) resolves a voice for a speaker in this order:

1. Consent re-check on every call (`BaseWorker.is_voice_clone_consented`, `shared/base_worker.py:556`); fail closed.
2. Platform flag `flags.voice_clone` and `_voice_clone_allowed` (`worker.py:845`).
3. Redis hash `voice:{meeting}:{speaker}` field `voice_id` (the in-meeting clone).
4. `carried_clone`: a clone promoted from an earlier meeting (section 6).
5. `chosen_dub_voice`: a voice the speaker picked for their own dub.
6. Catalog voice, hashed from `speaker_id` so un-cloned speakers sound different from each other.
7. Last resort `_default_voice_id`: built-in ids exist only for `en` and `vi`; anything else falls back to `en`.

Per-listener variants (`_resolve_voice_variants`, `worker.py:1985`):
- Always one `default` variant per (speaker, target language): the speaker's clone, else the hashed catalog voice.
- Only a speaker with no own voice gets additional variants: one per distinct voice a listener chose through `SetVoicePreference`, read by `_get_explicit_voice_choices` from Redis `translationRoom:{id}:voice_preferences` joined with the `languages` hash. A cloned speaker is never overridden by a listener's choice.

Catalog: `voice_catalog_size` 6 voices per language, cached 21600 s (`list_voices_by_language`), refreshed hourly.

## 5. Consent gate (runtime)

| Gate | Where | Default |
|---|---|---|
| Route-level `VoiceCloneEnabled` per `SourceUserId` | `BaseWorker.is_voice_clone_consented`, `shared/base_worker.py:556` | false; fail closed (`backend: TranslationRoomAudioRouteMapper.cs:22`) |
| Platform flag `flags.voice_clone` | `_voice_clone_allowed` | on, narrowable per workspace |
| Text-only routes | `text_only_dub_for`, `base_worker.py:664` | no synthesis at all |

Consent is re-evaluated per utterance, so a revoke stops cloned speech mid-meeting without waiting for deletion.

## 6. Clone creation

### 6.1 In-meeting clone (progressive ladder)

The worker accumulates the speaker's own audio and clones at rising sample durations. Rungs are provisional (`offer_carry_over=False`); only the final one is eligible for carry-over.

| Knob (`TTSSettings`, `shared/config.py:657-900`) | Default |
|---|---|
| `voice_clone_min_seconds` | 20.0 (platform key `meetings.voice_clone.min_sample_seconds`) |
| ladder (seconds) | 3.0, 8.0, 45.0 |
| `first_wait_ms` | 2500 |
| `max_buffer_seconds` | 90 |
| `voice_clone_max_upgrades` | 1 |
| `upgrade_margin` | 0.15 (platform key `meetings.voice_clone.upgrade_margin`) |
| `min_clone_chars` | 8 (text shorter than this uses the stock voice) |

Quality gate (`shared/clone_sample_quality.py`, mirrors web `voice-sample-quality.ts`):

| Check | Threshold |
|---|---|
| `MIN_RMS` | 0.008 |
| `MIN_ACTIVE_SPEECH_RATIO` | 0.18 |
| `MAX_CLIPPED_RATIO` | 0.02 |
| `MIN_ACTIVE_ENERGY_VARIATION` | 0.08 |
| Pitch coverage | Score only, not a gate; used to decide a later upgrade is worthwhile |

A clone is accepted as an upgrade only if its score beats the current one by `upgrade_margin`. The clone request wraps PCM in a RIFF header (`_clone_and_cache`, ~`worker.py:4217`) and caches the voice id (`cache_enabled`, ttl 3600 s).

### 6.2 Far speaker (platform bridge) clone (WT-933)

Speakers heard through a meeting-platform bridge cannot click a consent box locally. `far_speaker_clone.py` carries consent fields from the route; `far_speaker_clone_enabled` default true; `_withdraw_far_clone` and `_reconcile_far_clones` delete the clone when consent is withdrawn or the participant leaves.

### 6.3 Upload enrollment (voice profile)

1. Web posts a sample to backend `voice-profiles` (section 9), which validates and stores it.
2. Auth leaves the audio in Redis `voice:clone_sample:{profile_id}` and writes a stream notice.
3. Worker `_handle_upload_clone_request` (`worker.py:2974`) clones with the same quality gate, writes `voice:clone_result:{profile_id}` (JSON `voiceId`, `provider`, `error`, `errorCode`; TTL 7 days) and deletes the sample key.
4. Error code `CLONE_SAMPLE_EXPIRED` when the sample key has expired before pickup; backend marks the profile `clone_failed` and offers `POST {id}/clone/retry`.

Previews use `voice:preview_requests` and are cached at `voice:preview:{voice_id}:{language}` for 24 h.

### 6.4 Carry-over

`VoiceCarryOverService` and `VoiceCarryOverConsumerWorker` (backend auth) promote an in-meeting clone to a `voice_profiles` row (source `in_meeting`) only with active consent. An existing voice is replaced when the new quality score is better (a null existing score counts as improvable); the replaced Cartesia voice is deleted. In later meetings the promoted voice surfaces as `carried_clone` in the order of section 4.

## 7. Backend voice data model

Entities (backend auth service):

| Entity | Fields of note |
|---|---|
| `VoiceProfile` | `UserId`, `WorkspaceId`, `DisplayName` (max 100), `Language`, `Provider`, `EmbeddingRef` (the provider voice id), `Status`, `Source` (`upload`, `in_meeting`, `library`), `QualityScore`, `CloneErrorCode`, `CloneError`, `IsActive`, soft-delete fields |
| `VoiceSample` | `SampleType`, `FileUrl`, `DurationSeconds`, `Language`, `ContainsRawAudio`, `RetentionUntil`, soft-delete fields |
| `VoiceConsent` | `ConsentType` = `VOICE_CLONE`, `ConsentStatus` (`GRANTED`, `REVOKED`, `EXPIRED`), `ConsentTextVersion` (current `2026-09-29.v2`), `GrantedAt`, `RevokedAt`, `IpAddress`, `UserAgent` |

`EmbeddingRef` is a provider voice id, not a biometric embedding stored by WarpTalk.

API (`api/v1/auth`):

| Route | Purpose |
|---|---|
| `voice-consent` GET, POST grant, POST revoke | Consent lifecycle |
| `voice-profiles` GET, POST | List, create with sample |
| `voice-profiles/challenges` POST | Liveness phrase (WT-888) |
| `voice-profiles/catalog` GET | Stock voices |
| `voice-profiles/preferred-voice` PUT | Per-user stock voice |
| `voice-profiles/preview` POST | Audition |
| `voice-profiles/{id}/sample` GET | Retrieve sample |
| `voice-profiles/{id}/clone/retry` POST | Retry after `clone_failed`; re-checks consent |
| `voice-profiles/{id}` DELETE | Delete (section 11) |
| `voice-profiles/dub-voice` GET, PUT | Speaker-chosen dub voice (`DubVoiceId`) |

## 8. Enrollment checks and consent capture

`VoiceProfileService` rules: sample max 20 MB; magic-byte validation of the container (extension is not trusted); display name max 100; consent contract `VoiceProfileConsentContract` requires a consent text version that includes a hash of the submitted audio, so consent is bound to the exact sample.

Enrollment challenge (`VoiceEnrollmentChallengeService`, WT-888):
- The user reads a generated phrase; challenge lifetime 10 minutes; at most 10 issued per 10-minute window.
- The sample is transcribed by `ISpeechTranscriber`; accepted when `VoiceChallengeMatcher.PassThreshold` is at least 0.75.
- Errors: `VOICE_CHALLENGE_REQUIRED`, `VOICE_CHALLENGE_INVALID`, `VOICE_CHALLENGE_MISMATCH`.
- Purpose: show a live human, not a replayed recording of someone else.

Consent granted on enrollment is stored with IP and user agent; the in-meeting path relies on the same `VoiceConsent` row through the route flag.

## 9. Prosody and isochrony (voice-quality controls)

Latency tuning of these is in [08](08-realtime-latency.md); only the contracts are given here.

Prosody (`shared/prosody.py`, `prosody_enabled` true, `prosody_continuity` true):
- Source-speech arousal is measured and reduced to Cartesia's three controls: emotion label, speed (clamped 0.6 to 1.5) and volume. There is no pitch contour transfer.
- Valence from the translator (section 2.3) completes the emotion label, because arousal alone cannot tell anger from joy.
- `open_prosody_context` keeps continuity between consecutive sentences of one speaker.

Isochrony (`shared/isochrony.py`): closed loop that matches dubbed duration to source duration. An EMA of the measured dub/source duration ratio (alpha 0.25, at least 2 samples) shifts the speed for the next utterance within a 0.8 to 1.25 centre range. `_generation_config` (~`worker.py:4450`) merges both.

## 10. Failure handling (TTS)

| Failure | Behaviour | Reference |
|---|---|---|
| Synthesis error | `synthesis_max_retries` 1, backoff 0.5 s, within a 10 s retry window | `TTSSettings` |
| Give up | `_synthesis_give_up_reason` returns a reason for: partially spoken (never replay), not retryable, retries exhausted, retry window exceeded; a dead-letter entry is written | `worker.py:2778` |
| Clone failure | Utterance falls back to the stock voice; profile `clone_failed` for uploads | section 6.3 |
| Consent missing | Stock voice for that utterance (never a clone) | section 5 |
| Ordering | One lock per (speaker, target language) plus `consume_pipelined`, so retried sentences cannot overtake later ones | `worker.py` |

Stock-voice fallback chain: clone, carried clone, chosen dub voice, hashed catalog voice, built-in default (en/vi only), `en` default.

## 11. Revocation and deletion

Triggers and effects:

| Event | Effect |
|---|---|
| `POST voice-consent/revoke` (`VoiceConsentService.RevokeAsync`, ~lines 88-176) | Marks consent `REVOKED`, soft-deletes the user's profiles and samples, clears `DubVoiceId`; after the DB commit calls `RequestDeletionAsync(voiceId, "consent-revoked")` |
| `DELETE voice-profiles/{id}` | Soft-deletes profile and samples, removes sample objects from the bucket, clears `DubVoiceId` if it matches, queues provider deletion |
| Runtime | Consent re-check per utterance (section 5) stops cloned speech immediately |
| In-meeting orphans | Orphan sweep (enabled, interval 6 h, min age 24 h) deletes unpromoted in-meeting clones by name and age; it never touches promoted `profile-` voices |

Provider deletion is asynchronous: backend `RedisVoiceCarryOverQueue.cs:19` writes stream `voice:delete_requests`; the worker consumes it (`_VOICE_DELETE_STREAM`, `worker.py:423`; `_handle_voice_delete_request`, ~`worker.py:4437`). A failed provider delete is logged and left pending for redelivery; there is no user-visible confirmation. Soft-deleted sample retention is governed by `RetentionUntil`; the purge job was not located.

## 12. Configuration summary

| Area | Knob | Default |
|---|---|---|
| Translation | realtime model / effort / pool | `gpt-realtime-2.1-mini` / `minimal` / 4 |
| Translation | fallback model, temperature | `gpt-5.4-nano`, 0.0 |
| TTS | model, sample rate | `sonic-3.5`, 44100 |
| TTS | warm pool, max concurrency | 3, 3 |
| TTS | stream to LiveKit | true |
| TTS | synth cache | enabled, 3600 s |
| Clone | min seconds, ladder, upgrades, margin | 20.0, 3/8/45, 1, 0.15 |
| Clone | far speaker clone | true |
| Platform | `flags.voice_clone`, `meetings.voice_clone.min_sample_seconds`, `meetings.voice_clone.upgrade_margin` | operator-set |
| Orphans | sweep interval, min age | 6 h, 24 h |

Code defaults only; production overrides were not determined.

## 13. Gaps and proposals

| Gap | Note |
|---|---|
| Single provider each for translation and TTS | A provider outage degrades only within OpenAI or Cartesia |
| Delete confirmation | Provider deletion failure is only logged |
| Consent for far speakers | Depends on platform bridge flags; audit trail not located |
| Sample retention purge | `RetentionUntil` exists; the purge job was not found |

Proposals:
- (proposal) Add a secondary TTS provider behind the same `Synthesizer` interface, selected on repeated give-up for a language, with stock voices only (never a clone sent to a second vendor without separate consent).
- (proposal) Add a delete-verification step: after `voice:delete_requests`, list provider voices by the profile name and surface `deletion_pending` on the profile until confirmed.
- (proposal) Record an audit row per clone creation and deletion (voice id hash, consent id, source) to answer data-subject requests.
- (proposal) Gate translation on the same `ai_policy` flag used by suggestions if workspaces must block external LLM use for translation too.
- (proposal) Add a per-room glossary size cap and an eval set of ASR-repair cases to measure the "possible" fuzzy-match clause.

## 14. Not determined

- Whether the translation path checks `ai_policy`.
- Production overrides of all settings above.
- Purge job for expired `VoiceSample` rows.
