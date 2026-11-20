# 09 - AI Suggestion Agent (suggestion_worker)

Source repo: https://github.com/WarpTalk-CapstoneProject/warptalk-ai, commit `d0db9b9` (shallow clone). Gateway-side delivery read from https://github.com/WarpTalk-CapstoneProject/warptalk-backend, commit `c51b39b` (grep only).
Paths below are relative to the warptalk-ai repo unless prefixed `backend:`. Line numbers refer to that commit. Normative words follow RFC 2119. No secrets or hostnames are reproduced.

## 1. Purpose and placement

The suggestion agent decides, unprompted, whether a just-transcribed segment deserves a one-line inline hint (a badge pinned to a transcript line) and publishes it. (`suggestion_worker/worker.py:1-15`)

- It MUST run beside the real-time STT -> translation -> TTS path, never inside it. It reads `stt:results` under its own consumer group `suggestion-workers`, so a slow or failing suggestion never delays a caption. (`worker.py:181-183`)
- Suggestions MUST be ephemeral: nothing is persisted by this worker. (`worker.py:14`)
- The feature SHOULD be silent by default; every default is tuned to stay quiet. (`shared/config.py` `SuggestionSettings` docstring, line 984)

## 2. Pipeline

```
stt:results (group suggestion-workers)
  -> SuggestionWorker.process (worker.py:253)
     gates: deploy switch, room active, platform flag, transcript-paused
  -> window append (rolling context)
  -> stage 0 local heuristics (no tokens)           _passes_local_heuristics, worker.py:411
  -> cap==0 check, consent gate, cooldown probe     worker.py:326-340
  -> stage 1 decide (cheap model)                   OpenAISuggester.decide, suggester.py:438
  -> confidence / category / "fact needs docs" gates worker.py:346-385
  -> atomic slot claim (cooldown + budget, Redis)   _claim_slot, worker.py:515
  -> stage 2 generate (full model + documents)      OpenAISuggester.generate, suggester.py:481
  -> publish to ai_assistant:results (type=suggestion) _publish, worker.py:582
```

## 3. Trigger conditions

A segment reaches the decide model only if ALL hold, in this order (`worker.process`, `worker.py:253-340`):

| # | Gate | Behaviour |
|---|---|---|
| 1 | `SUGGESTION_ENABLED` | Deploy ceiling. `false` means off regardless of the platform flag (`worker.py:256`). Default `false`. |
| 2 | Room active | Not in `_paused_rooms` and route state not in `PAUSED, ENDED, FAILED, CANCELLED, TIMEOUT` (`_is_room_active`, `worker.py:54,406`). |
| 3 | `flags.ai_suggest` | Platform feature flag for the room's workspace; can only narrow gate 1. Workspace is resolved lazily only when the flag could depend on it (`_flag_allows`, `worker.py:239`). |
| 4 | Transcript not paused | When the host paused transcript recording, the segment is skipped BEFORE it enters the window, so off-the-record speech never reaches a prompt (`worker.py:276-301`, `BaseWorker.is_transcript_paused`). This is distinct from the room-level translation pause. |
| 5 | Stage 0 heuristics | Non-empty text; word floor; STT confidence floor (section 5). |
| 6 | Cap not zero | `max_per_meeting <= 0` means "no suggestions" (`worker.py:326`). |
| 7 | External-LLM consent | See section 8. Fail closed. |
| 8 | Cooldown not active | Read-only probe (`worker.py:338`). |

Every segment, including ones rejected at gate 5, is appended to the window first (after gate 4), because a short line may make the next one meaningful (`worker.py:303-309`).

`is_final_chunk` MUST NOT be used as a gate: on this stream it marks the empty end-of-chunk marker (`worker.py:419-423`).

## 4. Inputs

| Input | Source | Used by |
|---|---|---|
| Segment | `STTResultMessage`: `meeting_id`, `segment_id`, `speaker_id`, `text` (whitespace-normalized), `language`, `confidence` (`worker.py:259-265`) | both stages |
| Transcript window | In-memory `deque(maxlen=window_size)` per room, default 8 turns; current segment excluded from its own context (`worker.py:197,305,343`) | both stages |
| Workspace knowledge | Redis `meeting:{room}:context_snapshot`: document text with `--- Document: <name> ---` headers, written at meeting start; same key ai_assistant_worker reads for summaries. Fetched only after the decide gates pass (`_context_snapshot`, `worker.py:551`) | generate only |
| Language | Segment language; hint MUST be written in it. Reader languages from Redis hash `translationRoom:{room}:languages` (speaker included), minus the segment language, max 4 (`_reader_languages`, `worker.py:563`, `_MAX_READER_LANGUAGES` line 64) | generate only |

The window MUST be bounded and dropped in `_cleanup_room` to avoid a per-room leak (`worker.py:206-214`).

## 5. Stage 0: local heuristics (no tokens)

(`_passes_local_heuristics`, `worker.py:411-462`)

- Empty text MUST be rejected.
- Word floor: `min_question_words` (default 2) if `_looks_like_question`, else `min_words` (default 4). Question detection: trailing `?` or `？`, else first or last word in a small Vietnamese and English interrogative list (`worker.py:71-131`). It only selects the floor; it never decides to suggest.
- STT confidence: rejected if `confidence != STT_UNKNOWN_CONFIDENCE` and `confidence < min_stt_confidence`. The sentinel `-1.0` ("provider exposed no logprobs") MUST pass, otherwise the feature silently stops when logprobs are absent (WT-543, `worker.py:443-461`). Confidence is an average token logprob (always <= 0), not a 0-1 score.
- Rejections log at DEBUG only (`suggestion_stage0_rejected`, `worker.py:316`).

## 6. Prompt construction and model choice

Two stages, both OpenAI chat completions with `response_format={"type":"json_object"}` (`suggester.py:444-457, 504-520`).

| Stage | Model default | Max tokens | Purpose |
|---|---|---|---|
| decide | `gpt-4o-mini` | 64 | Cheap judgement on every segment that survives stage 0; returns `{should_suggest, category, confidence, reason}` |
| generate | `gpt-4.1` | 200 x (1 + number of reader languages) | Writes the hint; sees window plus documents |

Models, temperature (0.2) and `reasoning_effort` (default `"none"`, applies to gpt-5 family only) pass through `shared.openai_options.completion_options` (`suggester.py:450,510`). The deployed model may differ from these code defaults via env.

Decide prompt (`_DECIDE_SYSTEM_PROMPT`, `suggester.py:162`): the model judges, it is not told to abstain. Categories (`SUGGESTION_CATEGORIES`, `suggester.py:33`):

| Category | Fires when |
|---|---|
| `clarification` | A question was asked and not answered, with or without `?` |
| `term` | Undefined jargon, acronym or product used |
| `action` | A commitment lacks owner and/or date |
| `correction` | Contradicts earlier transcript, or misstates a well-established public fact (high bar) |
| `fact` | A figure or reference the meeting's own documents cover |

Generate prompt (`_generate_system_prompt`, `suggester.py:270`): per-category output contract from `_CATEGORY_CONTRACTS` (`suggester.py:211`); same language as the latest segment; meeting-specific facts (names, owners, dates, figures) MUST come from transcript or documents, never memory; public knowledge only for `term` and `correction` with `source` = `"general knowledge"`; empty `content` is a valid answer. The user turn carries hint type, decide reason, segment language and the fenced transcript (`suggester.py:492-497`).

Prompt-injection defences:
- Transcript is fenced in `<transcript>` markers and both prompts include `_UNTRUSTED_INPUT_RULE` (`suggester.py:137`, `_render_transcript` line 366).
- Reference documents are appended to the user message, not the system prompt, inside `<reference_documents>` (`suggester.py:498-501`).
- Model output is never used raw: a boolean, a whitelisted category, a clamped float, a length-capped string, and a source name matched against known document headers (`suggester.py:130-136, 332-344, 536-540`).

## 7. Sensitivity dials, dedup, rate limiting

| Control | Mechanism | Reference |
|---|---|---|
| Confidence gate | `decide.confidence < min_confidence` is dropped (default 0.55; below a coin flip is noise) | `worker.py:357` |
| Category whitelist | Unknown category dropped and logged at WARNING | `worker.py:364` |
| `fact` needs documents | `fact` with empty snapshot is declined, to avoid unsourced figures that look sourced (WT-371) | `worker.py:380` |
| Cooldown | Redis `suggest:cd:{room}`, `SET NX` with TTL = `cooldown_seconds` (min 1). Cross-replica. Probed read-only before decide; claimed atomically after decide passes and before generate | `worker.py:505,512,530` |
| Per-meeting budget | Redis `suggest:n:{room}` incremented with TTL `state_ttl_seconds`; over `max_per_meeting` is dropped | `worker.py:509,538-544` |
| Cleanup | Cooldown and budget keys are left to expire; not reset on room end | `worker.py:206-212` |

Limits MUST live in Redis, not worker memory, because replicas >= 2 share one consumer group (`worker.py:518-522`). Note that a claimed slot is burnt even if generate then returns nothing; the cooldown default was lowered to 20 s for that reason (`config.py`, comment above `cooldown_seconds`).

There is no text-similarity dedup of hints. Duplicate suppression is the cooldown, the budget, the "already explained" rule in the decide prompt, and "never restate" in the generate prompt (`suggester.py:183-185, 295`).

## 8. Privacy and DLP

| Control | Behaviour | Reference |
|---|---|---|
| External-LLM consent | Reads Redis `translationRoom:{room}:ai_policy` (JSON `allow_external_llm`), projected by the gateway. MUST fail closed: missing or unreadable key means no suggestion and no model call. Cached 60 s per room so a mid-meeting revoke takes effect | `_external_llm_allowed`, `worker.py:468-498`, `_POLICY_REFRESH_SECONDS` line 59 |
| Gate position | Consent is checked BEFORE any transcript text leaves the process | `worker.py:330` |
| Transcript pause | Paused speech MUST NOT enter the window or prompts | `worker.py:276-301` |
| Profanity | Gateway masks `content` and `detail` when the workspace profanity filter is on, before the push | `backend: gateway/src/WarpTalk.Gateway/Services/AiResultConsumerService.cs:1256-1268` |
| Hint length | `content` truncated to `max_suggestion_chars` (default 140) | `_publish`, `worker.py:588-591` |
| Source citation | Only document names present in the snapshot headers are accepted | `suggester.py:332-344` |

No other DLP or PII scrubbing was found in this worker.

## 9. Output schema and delivery

Message `SuggestionResultMessage` (`shared/schemas.py:1113-1149`):

| Field | Meaning |
|---|---|
| `meeting_id`, `segment_id` | Room and STT segment the badge pins to |
| `category` | One of the five categories |
| `content` | Badge text, at most `max_suggestion_chars` |
| `type` | Constant `"suggestion"` |
| `detail` | Expanded evidence, may be empty |
| `confidence` | The decide stage's confidence |
| `language` | Language of this copy |
| `token_count` | Decide plus generate tokens (billed as one AI_ASSISTANT usage record by billing_worker) |
| `sources_json` | `[{marker:"S<n>", kind:"document", title}]`, empty when from transcript |
| `timestamp_ms` | Epoch ms |

Delivery:
1. `BaseWorker.publish("ai_assistant:results", meeting_id, ...)` writes to both `ai_assistant:results:{meeting_id}` and the global `ai_assistant:results` (`shared/base_worker.py:818-842`; `_publish`, `worker.py:609`).
2. For each reader language, one additional copy with translated `content`/`detail`, `language` set, `token_count=0` (WT-922, `worker.py:611-629`). The client keeps the copy matching its listen language.
3. The gateway consumes the stream, routes `type="suggestion"` before the legacy summary branch, drops entries lacking `segment_id` or `content`, and pushes SignalR event `AiSuggestionReceived` to group `translationRoom:{roomId}` (`backend: AiResultConsumerService.cs:1252-1273, 1334-1345`; DTO in `backend: gateway/src/WarpTalk.Gateway/Hubs/HubModels.cs:133`).

## 10. Failure handling

- Both model calls MUST degrade to silence, never raise: any exception returns decline (decide) or `None` (generate), so the message is not redelivered and the model is not billed twice (`suggester.py:384-391, 459-461, 522-524`).
- The OpenAI client uses `max_retries=0` and `request_timeout_seconds` (default 8 s): a late hint is worthless, and SDK retries held the consumer about 27 s (`suggester.py:420-430`).
- `NullSuggester` is the default when `SUGGESTION_ENABLED` is false: the worker starts, consumes, declines everything, and needs no API key (`suggester.py:106`, `suggestion_worker/__main__.py:1-26`). Enabling without an API key is a hard startup failure (`suggester.py:416-418`).
- Consent read failure: fail closed (section 8). Reader-language read failure: hint still published in the speaker's language (`worker.py:572-574`). Malformed or missing translation entry: that reader only loses the copy (`suggester.py:347-363`).
- Empty or whitespace `content` after generate is not published (`worker.py:397`).
- Output truncation risk: generate budget scales with reader languages so JSON is not cut off (`suggester.py:510-518`).

## 11. Configuration knobs

Env prefix `SUGGESTION_` (`shared/config.py:984-1075`). Five thresholds are also operator-tunable live via platform settings, read per segment, falling back to env (`worker.py:225-237`, `shared/platform_settings.py:101-129`).

| Env | Default | Platform setting key (range) | Effect |
|---|---|---|---|
| `SUGGESTION_ENABLED` | `false` | n/a | Deploy ceiling |
| `SUGGESTION_API_KEY` | empty | n/a | Falls back to the shared OpenAI key resolver |
| `SUGGESTION_DECIDE_MODEL` | `gpt-4o-mini` | n/a | Stage 1 model |
| `SUGGESTION_GENERATE_MODEL` | `gpt-4.1` | n/a | Stage 2 model |
| `SUGGESTION_DECIDE_MAX_TOKENS` | 64 | n/a | Decide output cap |
| `SUGGESTION_GENERATE_MAX_TOKENS` | 200 | n/a | Per-language generate cap |
| `SUGGESTION_TEMPERATURE` | 0.2 | n/a | Sampling |
| `SUGGESTION_REASONING_EFFORT` | `none` | n/a | gpt-5 family only |
| `SUGGESTION_REQUEST_TIMEOUT_SECONDS` | 8.0 | n/a | Per-call timeout, no retries |
| `SUGGESTION_MIN_CONFIDENCE` | 0.55 | `meetings.ai_suggest.min_confidence` (0-1) | Decide confidence floor |
| `SUGGESTION_COOLDOWN_SECONDS` | 20 | `meetings.ai_suggest.cooldown_seconds` (0-600) | Min gap between hints per room |
| `SUGGESTION_MAX_PER_MEETING` | 30 | `meetings.ai_suggest.max_per_meeting` (0-200) | Budget; 0 disables |
| `SUGGESTION_STATE_TTL_SECONDS` | 14400 | n/a | Lifetime of budget key |
| `SUGGESTION_MIN_WORDS` | 4 | `meetings.ai_suggest.min_words` (1-20) | Word floor, statements |
| `SUGGESTION_MIN_QUESTION_WORDS` | 2 | n/a | Word floor, questions |
| `SUGGESTION_MIN_STT_CONFIDENCE` | -0.5 | `meetings.ai_suggest.min_stt_confidence` (-5 to 0) | Avg-logprob floor |
| `SUGGESTION_WINDOW_SIZE` | 8 | n/a | Context turns |
| `SUGGESTION_MAX_SUGGESTION_CHARS` | 140 | n/a | Badge length cap |
| (feature flag) | on | `flags.ai_suggest` | Per-workspace rollout, allow or deny list |

## 12. Relation to ai_assistant_worker

`ai_assistant_worker` is a different surface that shares infrastructure with this worker. (Only entry points were inspected.)

| Aspect | suggestion_worker | ai_assistant_worker summary path | ChatAssistantWorker (same package) |
|---|---|---|---|
| Class and group | `SuggestionWorker`, `suggestion-workers` | `AIAssistantWorker`, `assistant-workers` (`ai_assistant_worker/worker.py:80-85`) | `ChatAssistantWorker`, `assistant-chat-workers` (`chat_worker.py:669-674`) |
| Input | `stt:results` | `stt:results` | `assistant:chat_requests` |
| Trigger | Every segment, gated | `__MEETING_END__` marker or on demand (`worker.py:56,146`) | Explicit user question |
| Output | `ai_assistant:results`, `type=suggestion` | `ai_assistant:results` (summary, action items; `worker.py:479,502`) | `assistant:chat_results` (`chat_worker.py:1615`) |

Shared contracts: the `meeting:{room}:context_snapshot` key, the `sources_json` chip shape (so the client renders one provenance component), the `ai_assistant:results` stream (the gateway splits suggestions from legacy results by `type`), and the transcript-pause flag. Unlike the summary path, the suggestion worker needs no control-marker exception for pause because `__MEETING_END__` does not trigger it (`worker.py:291-293`).

## 13. Not determined

- Production values of env overrides and platform settings (code defaults only).
- Whether any frontend-side dedup exists for repeated badges (client code not read).
