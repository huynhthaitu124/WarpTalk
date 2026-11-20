# 08 - Real-time Latency Design (speech -> translation -> dub)

Source repo: https://github.com/WarpTalk-CapstoneProject/warptalk-ai, commit `d0db9b9` (shallow clone). No backend code was needed for this document.
Paths are relative to that repo; `config.py` means `shared/config.py`. Line numbers refer to that commit. Normative words follow RFC 2119. Measurements quoted here are those recorded in source comments (dated by the authors); they are not re-measured by this spec. Env names assume the class prefix shown in section 8. No secrets or hostnames are reproduced.

## 1. Pipeline and budget

```
LiveKit audio -> livekit_ingress_worker (VAD, gates, chunking)
   |-- audio:frames (live 96 ms windows, flash mode) -------+
   '-- audio:chunks (closed utterance)                      v
                                              stt_worker (OpenAI Realtime STT)
        stt:speculative (pub/sub hint) -> translation warm-up    |
        stt:results (early sentences, then remainder) <----------'
   -> translation_worker (Realtime first sentence, HTTP chat for the rest)
   -> translate:results -> tts_worker (Cartesia, streamed to LiveKit) -> tts:results / LiveKit track
```

Streams: `audio:chunks`, `audio:frames` (`shared/schemas.py:125`), `stt:results`, `translate:results`, `tts:results`; each is published to both `<stream>:{meetingId}` and the global `<stream>` (`shared/base_worker.py:818-842`). Workers are independent consumer groups on Redis Streams.

Budget and targets, where the source defines them:

| Source | Target or figure |
|---|---|
| `.agents/resources/ai_pipeline_architecture.md:3-10` | Design target about 1.5 s to first output (STT 0.5 s, translate 0.4 s, TTS 0.6 s) on 2 s chunks. Legacy design note; the shipped chunking is 6 s max (section 2). |
| `.agents/resources/implementation_plan.md:15-24` | Older budget (about 1.5 s) written for Whisper/XTTS/NLLB, not the deployed providers. Treat as obsolete. |
| `tools/probe_translate_ttfb.py` docstring | Working goal "< 2.5 s" end to end ("whether translation can be hidden"). |
| Measured, STT (`config.py:432-470`) | commit -> completed p50 774 ms with streaming model and paced audio, versus 1651 ms baseline (human Vietnamese recording, n=3, 2026-08-20). |
| Measured, translation (`config.py:604-612`) | Realtime mini warm 655-919 ms; chat model warm median about 0.8 s. |
| Measured, TTS (`config.py:775-790`) | First audio 0.669 s cold vs 0.180 s with a warm connection (end to end 0.721 s -> 0.251 s). |
| Metric `tts_first_audio` | What the listener hears; `tts_synthesis` includes playout under streaming and MUST NOT be read as wait time (`tts_worker/worker.py:2685-2711`). |

No hard SLO is enforced in code; targets are guidance. Flow control relies on stream lag and stage histograms (section 9).

## 2. Ingress: chunking, VAD, endpointing, gates

All in `livekit_ingress_worker/worker.py` with settings on `WorkerSettings` (`config.py:98-410`).

**Chunk cap.** `chunk_duration_ms` default 6000 is a hard cap for uninterrupted speech; ordinary turns flush on VAD silence (`config.py:168-172`). Platform setting `meetings.chunk_duration_ms` (range 2000-15000) exists (`shared/platform_settings.py:121`).

**VAD hysteresis and padding.** Silero per 32 ms frame. Entry `vad_threshold` 0.35, exit `vad_release_threshold` 0.20 (clamped to entry), `vad_pre_speech_ms` 384 prepended from the ring buffer (no latency cost), `vad_min_speech_ms` 288 (`config.py:198-247`).

**Adaptive hangover ladder** ("the more a speaker has said, the readier to cut", `config.py:283-334`):

| Speech so far | Hangover | Knob |
|---|---|---|
| >= 4000 ms (`vad_seek_boundary_after_ms`) | 250 ms | `vad_seek_hangover_ms` |
| 1500-4000 ms | 576 ms | `vad_silence_hangover_ms` |
| < 1500 ms (`vad_short_turn_speech_ms`) | 864 ms | `vad_short_turn_hangover_ms` |

Trailing silence beyond the base hangover is trimmed before publish, so a longer decision window adds no audio. Boundary seeking has about 1.6 s (4000-5616 ms) to find a pause before the cap cuts blindly, because pre-speech padding counts toward the cap.

**Gates** (applied to the closed chunk, in `_publish_speech_chunk`, `worker.py:2637`; not to live frames):

| Gate | Module | Default | Behaviour |
|---|---|---|---|
| Absolute energy floor | `_ENERGY_FLOOR_RMS` (`worker.py:420`) | RMS 0.02, always on | Drops noise that tripped VAD; scaled by sqrt(speech/total samples) so padding does not tighten it (`worker.py:2688`) |
| `speech_level_floor` | `livekit_ingress_worker/speech_level_floor.py` | `ingress_energy_relative_ratio` 0.4 (0 disables), baseline min 3 chunks, window 8 | Only LOWERS the floor for a speaker whose own median speech level is quiet; rescues quiet mics (`config.py:355-357`) |
| `near_field_gate` | `livekit_ingress_worker/near_field_gate.py` | `near_field_gate_enabled` false; relative floor 0.35, min baseline chunks 2, EMA alpha 0.3 | Rejects chunks whose peak is under 35 % of the track's own near-field peak. Off by default: it dropped valid middle chunks in production (`config.py:336-346`) |
| `far_side_gate` | `livekit_ingress_worker/far_side_gate.py` | `far_side_gate_enabled` false | Meet-bridge only: zeroes 32 ms frames of the stand-in feed explained by a real participant's own track, delayed 150-1200 ms. Unsure means keep. Off pending a real bridged recording; synthetic eval removes only about 20 % of duplicate frames (`config.py:358-405`) |

The STT text dedupe `far_side_dedupe_*` is the second layer for the same duplicate (`config.py:533-549`, `stt_worker/far_side_dedupe.py`).

## 3. STT: streaming, partials, commit, second pass

- **Flash mode (stream while speaking).** `stt_streaming_enabled` default true (`config.py:135`). The ingress publishes 96 ms windows to `audio:frames` (`STT_FRAME_STREAM_MAXLEN` 2000) via `publish_ephemeral`, without energy or near-field gating, so the worst case of failure is the old latency, never a lost sentence (`worker.py:2606-2636`). Per-room override Redis key `translationRoom:{id}:flash_mode`, read at speech onset and held for the whole utterance (`_flash_mode_enabled`, `worker.py:2515`); platform default `meetings.flash_mode_default` (`platform_settings.py:120`).
- **Model pairing.** `STTSettings.model` default `gpt-live-transcribe` MUST be used only with flash mode on: given a closed chunk in one go it measured 4733 ms, 2.9x worse than baseline. Turning flash mode off requires reverting both (`config.py:432-476`).
- **Commit semantics.** `input_audio_buffer.commit()` consumes the buffer, so rolling commits yield disjoint slices, not a growing hypothesis (`tools/probe_commit_semantics.py`); 1 s slices hallucinated across languages (`tools/probe_chunk_context.py`). Hence the design commits at the VAD turn end and does not use rolling partial hypotheses.
- **Early (mid-chunk) sentences.** `on_early_segment=publish_early` publishes each sentence the model finishes before the turn closes. They are final and append-only (`exclude_emitted_from_final=True`; the completed event carries only the remainder) and pass the full `_filter_segments`. Measured first sentence 1649 ms (vi, 15 s clip) and 1176 ms (en, 18.5 s) versus whole chunk about 5.7 s. Trade-off: deltas carry no logprob, so they publish with `STT_UNKNOWN_CONFIDENCE` (-1.0) and the confidence gate cannot fire (`stt_worker/worker.py:1425-1456`, `config.py:445-455`).
- **Speculative hint.** `on_speculative_segment=publish_speculative` publishes a complete delta sentence on pub/sub `stt:speculative` (ephemeral, never shown). The two callbacks are mutually exclusive, and the early path replaced the speculative cache in flash mode (`stt_worker/worker.py:1240-1275, 1450-1456`).
- **Warm sockets.** `realtime_pool_size` default 4 warm WebSockets avoid the 1-2 s handshake on a speaker's first utterance (`config.py:512-514`).
- **Concurrency.** Cross-speaker parallel dispatch with a per-speaker lock (same-speaker commits MUST NOT interleave); `_MAX_IN_FLIGHT` 32; ack tied to handler success (`stt_worker/worker.py:301, 858-905`).
- **Clean prepass.** Deterministic disfluency prepass on every final segment, about 1-5 ms, kill switch `stt_clean_prepass_enabled` (`config.py:167`). Prosody measurement is about 3 ms CPU per second of audio, run as a parallel task (`stt_worker/worker.py:1240`, `config.py:523`).
- **Second pass (post-meeting, not real time).** `retranscribe_worker` re-transcribes archived per-speaker files with a batch endpoint and a stronger model, off the latency path; it merges by sort on the meeting clock and refuses spans flagged `transcriptPaused` (`retranscribe_worker/batch_transcriber.py`, `merge.py`, `stt_worker/second_pass.py`; README "Transcript recording pause"). Requires `audio_archive_enabled` (default false).

## 4. Translation: first-token and ordering

- **Realtime for the first sentence.** The persistent Realtime text path (`realtime_model` default `gpt-realtime-2.1-mini`, effort `minimal`, `realtime_max_output_tokens` 128, pool 4, timeout 2.0 s) carries the first sentence of each utterance; the chat model (`model` default `gpt-5.4-nano`) is the correctness fallback and carries the rest via `translate_batch`, one call for N sentences, falling back to concurrent per-sentence calls (`config.py:603-648`, `translation_worker/translator.py:515, 763`, `worker.py` fallback log `openai_realtime_translation_failed_using_chat_fallback` at `translator.py:695`).
- **Why timeout 2.0 s and 128 tokens.** A premature HTTP fallback turned a rare 1.2 s response into a 5-19 s cliff; 64 tokens intermittently ended `incomplete` (`config.py:640-647`). Effort above minimal broke or slowed the path with no repair gain (`tools/probe_realtime_effort.py`).
- **Warm-up.** `OpenAITranslator.warm_up` sends one tiny request at startup; a cold first request measured above 3 s (`translator.py:375-392`). The Realtime pool is pre-connected the same way (`_ensure_realtime_pool`, `translator.py:448`).
- **Speculative prefetch.** `TranslationWorker` subscribes to `stt:speculative` and translates delta sentences ahead, keyed by normalized text, TTL `_SPECULATIVE_TTL_SECONDS` 15 s, per-call `_SPECULATIVE_TIMEOUT_SECONDS` 1.5 s, one-slot semaphore (`translation_worker/worker.py:193-223, 262-360`). Output is cached only; durable publish happens only for the validated STT segment.
- **Concurrency and order.** `_CONCURRENCY_LIMIT` 8 and `_MAX_IN_FLIGHT` 32 so sentence 2 starts while sentence 1 is in flight; a per-speaker `_PublishTurn` event prevents a later line overtaking an earlier one, waiting at most `_PUBLISH_ORDER_WAIT_SECONDS` 10 s (nothing dropped) (`worker.py:140-176, 185-190, 420-480`).
- **Determinism for cache hits.** `temperature` 0.0 so repeated sentences translate identically and hit the TTS cache (`config.py:649-654`).
- **Same-language polish.** `_POLISH_TIMEOUT_SECONDS` 1.5 s; past it the raw text ships (`worker.py:198-201`).
- **Retries.** `_create_with_retry`: 2 retries, 0.5 s base delay doubling (`translator.py:578`). Provider is OpenAI only, no cross-vendor fallback (`config.py:599`).
- **ASR-repair prompt gating.** The repair clause is sent only when a possible mishearing or meeting context exists, to save tokens (`translator.py:70-110`; `tools/probe_gate_hit_rate.py`).

## 5. TTS: first audio, isochrony, catch-up

- **Warm Cartesia connections.** `tts_warm_pool_size` 3 pre-dialled WebSockets, refilled in background after each claim; dialing costs p50 427.6 ms versus 0.1 ms to build a context on an open connection (`tts_worker/synthesizer.py:125-195`, `config.py:775-790`).
- **Stream to LiveKit.** `stream_to_livekit` default true pushes each Cartesia chunk onto the track as it arrives (WT-397) (`config.py:730-739`). `tts_synthesis` therefore includes playout; use `tts_first_audio`.
- **Concurrency gate.** `cartesia_max_concurrency` 3 (plan limit divided by replicas). An open prosody context holds a vendor slot until `done`, so contexts MUST be ended right after `flush_done`, and a sentence waits only for generation, not playout. Wait time is recorded as `tts_slot_wait` for every sentence (`config.py:791-824`, `tts_worker/worker.py:2695`). Consume dispatch `_MAX_IN_FLIGHT` 32 (`worker.py:705`).
- **Synthesis cache.** `cache_enabled` true, `cache_ttl_seconds` 3600, key `tts:cache:{sha256}`; a hit costs about 2 ms against about 1 s for a call (`config.py:746-747`, `tts_worker/worker.py:2535, 4602-4617`).
- **Voice catalog warm.** `voice_catalog_warm_enabled` true, refresh 3600 s against a 6 h TTL (`config.py:757-769`).
- **Speed.** `speed` default `fast`: a dub must fit the speaker's gap (`config.py:673`).
- **Isochrony.** `shared/isochrony.py` learns dub/source duration ratio per speaker and language and centres Cartesia speed on it, clamped 0.8-1.25 (`MIN_SPEED_CENTER`, `MAX_SPEED_CENTER`; applied at `tts_worker/worker.py:4499`).
- **Catch-up.** When the dub lags the speaker by more than `_CATCH_UP_ON_TIME_MS` 1500, speed is multiplied linearly up to 1.25 at `_CATCH_UP_FULL_LAG_MS` 4000 ms of lag (`tts_worker/worker.py:671-683, 4530-4556`).
- **Prosody continuity.** `prosody_continuity` true keeps one Cartesia context across a speaker's consecutive sentences only when the next is already queued (`config.py:725`).
- **Retry.** `synthesis_max_retries` 1, backoff 0.5 s, window 10 s; only for moment-bound failures (drop, 5xx, timeout), never 4xx; retry runs inside the per-key lock so order is kept. Exhausted or 4xx go to `translate:results:dead-letter` (`config.py:826-846`).

## 6. Queues, backpressure, recovery

- Blocking read `block_ms=2000`; `consume_concurrent` with `count` and `concurrency` for opt-in workers; default `concurrency` 1 (`shared/base_worker.py:98-111, 855-890`). Audio and text workers with ordering assumptions MUST keep 1 unless they use their own pipelined consume (stt, translation, tts use `consume_pipelined` with `max_in_flight`).
- Stale recovery: XAUTOCLAIM after `processing_timeout_seconds` (120) plus `reclaim_margin_seconds` (30); `max_delivery_attempts` 5 then dead letter; heartbeat 10 s with TTL 30 s (`base_worker.py:112-125, 899-1049`).
- Redis: `max_connections` 50, `socket_timeout` 15 s, `socket_connect_timeout` 5 s, `retry_max_attempts` 5, `retry_base_delay` 0.5 s (`config.py:22-50`).
- Memory bound: `stream_maxlen` 200 entries per stream (a `tts:results` entry is about 113 KB of WAV) and `stream_ttl_seconds` 3600 refreshed on publish, because nothing deletes room streams otherwise (`config.py:41-79`).
- A suggestion or assistant worker reads `stt:results` in its own group and MUST NOT delay captions (see 09).

## 7. Provider timeouts and fallbacks

| Stage | Timeout | Fallback | Reference |
|---|---|---|---|
| STT realtime | Pool of warm sockets; discard floor `min_avg_logprob` -0.7 (per-language overrides) | Closed chunk on `audio:chunks` still carries the whole turn if streaming fails | `config.py:486-502`, `worker.py:2606` |
| Translation realtime | 2.0 s | HTTP chat model | `config.py:642` |
| Translation speculative / polish | 1.5 s / 1.5 s | Discard speculation; ship raw text | `translation_worker/worker.py:197-201` |
| TTS | Cartesia SDK retries plus one worker retry within 10 s | Bytes fallback; dead letter on terminal failure | `config.py:844-846` |
| Suggestion LLM | 8 s, 0 retries | Silence | `config.py:1022` |
| Provider call stats | Every OpenAI and Cartesia call timed to response headers | n/a | `shared/provider_calls.py:1-30` |

## 8. Configuration knobs

Env prefixes: `WorkerSettings` none (field name uppercased), `STTSettings` `STT_`, `TranslationSettings` `TRANSLATION_`, `TTSSettings` `TTS_`, `RedisSettings` `REDIS_` (`config.py`). Values are code defaults; deployments may override.

| Knob (class) | Default | Effect |
|---|---|---|
| `stt_streaming_enabled` (Worker) | true | Flash mode: stream audio during speech |
| `chunk_duration_ms` (Worker) | 6000 | Hard cap per chunk; lower is faster but cuts sentences |
| `vad_threshold` (Worker) | 0.35 | VAD entry bar |
| `vad_release_threshold` (Worker) | 0.20 | VAD exit bar |
| `vad_pre_speech_ms` (Worker) | 384 | Onset padding, no latency cost |
| `vad_silence_hangover_ms` (Worker) | 576 | Standard end-of-turn wait |
| `vad_min_speech_ms` (Worker) | 288 | Minimum speech to count |
| `vad_seek_boundary_after_ms` / `vad_seek_hangover_ms` (Worker) | 4000 / 250 | Take earlier pause in long turns |
| `vad_short_turn_speech_ms` / `vad_short_turn_hangover_ms` (Worker) | 1500 / 864 | Hold short turns open to avoid fragments |
| `near_field_gate_enabled` (+ `_relative_floor` 0.35, `_min_baseline_chunks` 2, `_baseline_ema_alpha` 0.3) | false | Peak-relative gate |
| `ingress_energy_relative_ratio` (+ `_baseline_min_chunks` 3, `_baseline_window` 8) | 0.4 | Speaker-relative floor, 0 disables |
| `far_side_gate_enabled` (+ lag 150-1200 ms, history 4000, lock corr 0.5 / hold 8000, frame corr 0.7, second-voice hold 1500) | false | Meet duplicate suppression |
| `stt_clean_prepass_enabled` (Worker) | true | Tier-1 disfluency clean |
| `model` (STT) | `gpt-live-transcribe` | Streaming model, paired with flash mode |
| `realtime_pool_size` (STT) | 4 | Warm STT sockets |
| `min_avg_logprob` (STT) | -0.7 | Discard floor |
| `noise_reduction` (STT) | `off` | Extra denoise hurt close-mic speech |
| `prosody_enabled` (STT, TTS) | true | About 3 ms/s CPU; field optional downstream |
| `far_side_dedupe_enabled` (+ window 15000 ms, lag 150-2000 ms, ratio 0.8) (STT) | true | Text-level duplicate drop |
| `model` / `realtime_model` (Translation) | `gpt-5.4-nano` / `gpt-realtime-2.1-mini` | Fallback and first-sentence models |
| `realtime_reasoning_effort` / `realtime_max_output_tokens` / `realtime_timeout_seconds` / `realtime_pool_size` (Translation) | `minimal` / 128 / 2.0 / 4 | Latency-critical first sentence |
| `max_tokens` / `temperature` (Translation) | 512 / 0.0 | Output cap; determinism for TTS cache |
| `model` / `speed` / `sample_rate` (TTS) | `sonic-3.5` / `fast` / 44100 | Voice model and pace |
| `stream_to_livekit` (TTS) | true | Chunk-by-chunk playout |
| `tts_warm_pool_size` (TTS) | 3 | Warm Cartesia sockets |
| `cartesia_max_concurrency` (TTS) | 3 | In-flight generations |
| `cache_enabled` / `cache_ttl_seconds` (TTS) | true / 3600 | Synthesis cache |
| `prosody_continuity` (TTS) | true | Context across sentences |
| `synthesis_max_retries` / `_backoff_seconds` / `_retry_window_seconds` (TTS) | 1 / 0.5 / 10.0 | Bounded retry |
| `voice_clone_ladder_seconds` / `voice_clone_first_wait_ms` (TTS) | (3, 8, 45) / 2500 | Progressive clone, bounded wait |
| `stream_maxlen` / `stream_ttl_seconds` (Redis) | 200 / 3600 | Memory bound |
| `socket_timeout` / `max_connections` (Redis) | 15.0 / 50 | Connection tolerance |
| `meetings.flash_mode_default`, `meetings.chunk_duration_ms` (platform settings) | true, 6000 (2000-15000) | Live-tunable, per `platform_settings.py:120-121` |

Other settings that are not latency knobs (voice cloning thresholds, far-speaker hints, orphan voice sweep) were not analysed.

## 9. Metrics emitted

From `metrics_exporter/metrics.py` (Prometheus text on `METRICS_PORT`, default 9108):

| Metric | Type | Source |
|---|---|---|
| `redis_stream_group_lag`, `redis_stream_group_messages_pending`, `redis_stream_group_consumers` | gauge | `XINFO GROUPS` per stream and group, every scrape |
| `redis_stream_groups`, `redis_stream_length` (dead-letter streams) | gauge | same |
| `redis_keys_count` | gauge | worker heartbeat keys |
| `warptalk_stage_latency_ms` | histogram, buckets 250, 500, 1000, 2000, 3000, 5000, 8000, 12000, 20000 ms | `warptalk:latency:{stage}` written by `record_latency` (`shared/redis_client.py:36-37, 439-461`) |
| `warptalk_stage_messages_total{stage,outcome}` | counter, outcomes ok, error, timeout, vendor_error, dead_letter | `warptalk:outcome:{stage}` hash (`redis_client.py:46`) |
| `warptalk_billing_unbilled_events_total` | counter | billing (not latency) |

Stages recorded by name include `tts_synthesis`, `tts_slot_wait`, `tts_first_audio` (`tts_worker/worker.py:2690-2711`) and per-worker stage latency via `publish_telemetry` (`redis_client.py:418-437`, also published on `translationRoom:telemetry`). Per-vendor outcome and latency hashes `warptalk:provider_calls:{date}` feed the admin Providers page (`shared/provider_calls.py`). Per-room stream series are off by default (`METRICS_EXPORT_PER_ROOM_STREAMS`). Auto-scaling thresholds in `.agents/resources/ai_pipeline_architecture.md` (STT lag > 100, TTS lag > 50) are design notes; no autoscaler config was found in this repo.

## 10. Probe tools (`tools/`, docstrings only)

| Tool | Measures |
|---|---|
| `probe_streaming_lag.py` | How far behind a live speaker each STT provider runs when audio is paced in real time |
| `probe_commit_semantics.py` | Whether Realtime `commit()` consumes the buffer or grows it (rolling-commit viability) |
| `probe_chunk_context.py` | Whether caching prior sentences as context rescues chunked transcription |
| `probe_energy_floor.py` | What the 0.02 RMS ingress floor actually rejects |
| `probe_translate_ttfb.py` | Warmed translation latency on the production path; whether speculative translation can hide it |
| `probe_realtime_effort.py` | Realtime reasoning-effort sweep against mishearing repair and latency |
| `probe_latency_ab.py` | 2x2 prompt x model latency A/B (repair prompt and full vs mini model) |
| `probe_tts_first_audio.py` | Time to first sample of a dubbed sentence, cold vs warm connection |
| `probe_world_roundtrip.py` | Whether reshaping Cartesia pitch after synthesis costs more than it buys |
| `probe_gate_hit_rate.py` | How often the ASR-repair evidence gate stays shut in real meetings |
| `probe_gate_safety.py` | Whether dropping the repair instruction without evidence is safe |
| `probe_gate_wiring.py` | The real prompt `translate()` sends once the repair clause is gated |
| `probe_guardian_vs_prod.py` | Effect of the repair ("guardian") prompt on production models |
| `probe_hard_case_fluency.py` | Whether the repair prompt turns a visible error into a fluent wrong sentence |
| `probe_which_part_repairs.py` / `probe_context_only_repair.py` | Which half of the repair mechanism does the work, and whether the instruction is redundant |
| `replay_livekit_chunk.py` | Replays a retained chunk through a real LiveKit track for full-path latency |

## 11. Known trade-offs

- Flash mode and the streaming STT model are one decision; either alone regresses (section 3).
- Early sentences gain seconds but give up the logprob gate; foreign-script leaks measured 0 of 6, which is evidence, not a guarantee.
- Longer hangover for short turns costs about 288 ms on genuine one-word replies to reduce fragments.
- 6 s cap and boundary seeking can still cut a speaker who never pauses; raising the cap delays everyone who does not pause.
- Lower `vad_release_threshold` may hold turns open in noisy rooms until the cap; the value is unmeasured on this product's audio (`config.py:222-241`).
- Near-field and far-side gates are off by default for recall; the 0.02 floor is the only always-on gate.
- Realtime first sentence is fast but cannot repair mishearings; the larger model repairs (3 of 3 versus 0 of 3) at similar latency (`config.py:625-637`). Switching model is a deploy decision.
- Realtime timeout of 2.0 s trades tail latency for avoiding the HTTP-fallback cliff.
- Streaming TTS makes `tts_synthesis` include playout; dashboards MUST use `tts_first_audio`.
- Cartesia concurrency 3 means a burst queues at the vendor; only `tts_slot_wait` makes it visible.
- Isochrony and catch-up raise speech rate up to 1.25x, bounded to stay intelligible.
- `stream_maxlen` 200 trades replay depth for Redis memory.

## 12. Not determined

- Production values of env overrides and platform settings.
- Current deployed autoscaling and concurrency (replica counts).
- Any enforced end-to-end SLO; only design targets and probe goals were found.
- `translator.py` details beyond entry points (not read in full), and `tts_worker` internals beyond cited ranges.
