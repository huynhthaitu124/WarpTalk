# 08 - Real-time Translation Pipeline and Sub-2s Latency

| Repo | Commit | Scope read |
|---|---|---|
| `warptalk-ai` | `d0db9b9` (2026-10-04), full history, blobless clone, 420 commits | whole latency path; `git log` searched for latency work |
| `warptalk-backend` | `c51b39b` (depth 1, sparse) | `gateway/.../Services/AiResultConsumerService.cs`, `MeetingCaptionMetrics.cs` only |
| `warptalk-web` | `e1dd35d` (depth 1, sparse) | `src/lib/{meeting,audio,realtime}`, `src/components/rooms/live`, `src/hooks` only |

Paths are relative to the named repo; `config.py` is `shared/config.py`; line numbers refer to the commits above. Normative words follow RFC 2119. Figures come from source comments, docstrings and commit messages (the authors' own measurements, not re-measured here). A figure marked "(inferred)" is this spec's estimate, not found in code. No secrets, hostnames or URLs are reproduced; env names assume the prefixes in section 8.

**Headline.** A two-second deadline is stated in comments for the live transcript (`livekit_ingress_worker/audio_archive.py:4`, `retranscribe_worker/batch_transcriber.py:5`), but no code constant, alert or SLO enforces it, and the measured dub lag in the authors' own probe is about 2.7 s (section 3). This spec states the budget the architecture can meet, which depends on early-sentence publishing, speculative translation and warm connections, and which the single-short-sentence path does not meet.

## 1. End-to-end flow

```mermaid
sequenceDiagram
  autonumber
  participant Mic as Speaker client (web)
  participant LK as LiveKit SFU
  participant ING as livekit_ingress_worker
  participant STT as stt_worker (Realtime STT)
  participant TR as translation_worker
  participant TTS as tts_worker (Cartesia)
  participant GW as Gateway (SignalR)
  participant L as Listener client (web)

  Mic->>LK: mic track (WebRTC)
  LK->>ING: AudioStream 16 kHz mono (native resample)
  ING->>ING: VAD, hangover ladder, pre-speech pad
  ING-)STT: audio:frames (96 ms windows, ungated) [flash mode]
  STT-)GW: pub/sub stt:interim (live words, 150 ms throttle)
  GW-)L: TranscriptInterimReceived (caption preview)
  STT-)TR: pub/sub stt:speculative (finished delta sentence)
  TR->>TR: speculative translate, cache 15 s
  ING-)STT: audio:chunks (closed turn, gated)
  STT-)TR: stt:results (early sentences, then remainder)
  STT-)GW: stt:results (same stream, own group)
  GW-)L: TranscriptSegmentReceived (original caption)
  TR->>TR: sentence 0 single call (Realtime), rest one batch call
  TR-)TTS: translate:results (one entry per sentence per target language)
  TR-)GW: translate:results (own group)
  GW-)L: TranslationTextReceived (translated caption)
  TTS->>TTS: cache, voice select or clone, Cartesia slot
  TTS->>LK: bot publishes PCM 20 ms frames as chunks arrive
  TTS-)GW: tts:results (WAV bytes, also cached; web has no handler found)
  LK->>L: ai-interpreter-{lang}-{speaker} track
  L->>L: raw mic unsubscribed while dub present, play dub
```

Hop table (each stream is published to both `<stream>:{meetingId}` and the global `<stream>`, `shared/base_worker.py:818-842`; workers are independent consumer groups):

| Hop | Transport | Producer to consumer | Source |
|---|---|---|---|
| 1 | LiveKit track | web mic to ingress bot (`AudioStream(sample_rate=16000, num_channels=1)`) | commit `510abb0`, `livekit_ingress_worker/worker.py` |
| 2a | Redis stream `audio:frames` (maxlen 2000, ephemeral) | ingress to STT, 96 ms windows while speaking | `worker.py:2606-2636` |
| 2b | Redis stream `audio:chunks` | ingress to STT, closed utterance (also feeds voice clone sampling in tts_worker) | `worker.py:2823` |
| 3a | pub/sub `stt:interim` | STT to gateway, live words, never stored or billed | `stt_worker/worker.py:359-381`, `stt_worker/model.py:97` |
| 3b | pub/sub `stt:speculative` | STT to translation, prefetch hint | `translation_worker/worker.py:256-278` |
| 4 | stream `stt:results` | STT to translation, gateway, transcript service, assistant (separate groups) | `stt_worker/worker.py:1425-1456` |
| 5 | stream `translate:results` | translation to tts_worker, gateway, billing, transcript | `translation_worker/worker.py:1003-1016` |
| 6 | LiveKit track `tts-audio`, identity `ai-interpreter-{lang}-{speakerId}` (`-voice-{id8}-` variant for a listener-picked voice) | tts_worker bot to listeners | `tts_worker/livekit_publisher.py:616-670`, `warptalk-web src/lib/meeting/interpreter-track.ts:28-33` |
| 7 | stream `tts:results` | tts_worker to gateway (`TranslatedAudioReceived`, base64 WAV) | `AiResultConsumerService.cs:1069-1114` |
| T | pub/sub `translationRoom:telemetry`, hashes `warptalk:latency:{stage}` | every worker, stage ages | section 7 |

Two parallel lanes leave `translate:results`: the text-caption lane (gateway to `TranslationTextReceived`, about the polling interval of 25 ms) and the dub lane (tts_worker to a LiveKit track). A caption never waits for TTS. In `warptalk-web` no handler for `TranslatedAudioReceived` was found in the directories read; the dub is heard only through the LiveKit track (not verified for the rest of the web repo).

## 2. Stage behaviour that sets the budget

### 2.1 Ingress (`livekit_ingress_worker/worker.py`, settings `config.py:98-410`)

- `chunk_duration_ms` 6000 is a safety cap; turns normally close on VAD silence (`config.py:168-172`). Silero per 32 ms frame; entry 0.35, exit 0.20; `vad_pre_speech_ms` 384 prepended at no latency cost; `vad_min_speech_ms` 288.
- Adaptive hangover (`config.py:283-334`): speech so far under 1500 ms waits 864 ms; 1500-4000 ms waits 576 ms; 4000 ms or more waits 250 ms. Trailing silence beyond the base is trimmed before publish.
- Gates on the closed chunk (`_publish_speech_chunk`, `worker.py:2637`): absolute RMS floor 0.02 (always on), `speech_level_floor` (relative 0.4), `near_field_gate` (off), `far_side_gate` (off). Live frames in flash mode skip the gates, so a gate bug cannot lose a sentence (`worker.py:2606-2636`).
- Chunk `timestamp_ms` is stamped at the end of the chunk and is carried unchanged through STT, translation and TTS messages (`translation_worker/worker.py:957`). Every stage age in section 7 therefore counts from the end of the turn, after the hangover.

### 2.2 STT (`stt_worker/`)

- Flash mode streams audio while the person speaks to `gpt-live-transcribe`; the model is only valid with flash mode on (`config.py:432-476`, measured table in section 4).
- Three outputs per utterance: live words on `stt:interim` (preview, replaced by the final line); early sentences on `stt:results` as the model completes them (append-only, `exclude_emitted_from_final`, confidence `-1.0` unknown); the remainder on commit. `input_audio_buffer.commit()` consumes the buffer, so rolling commits yield disjoint slices and 1 s slices hallucinated, hence commit only at turn end (`tools/probe_commit_semantics.py`, `tools/probe_chunk_context.py`).
- Second pass is not real time: `retranscribe_worker` re-runs archived per-speaker audio with a batch endpoint (`stt_worker/second_pass.py`, requires `audio_archive_enabled`, default false). It exists precisely because the live path is tuned for the clock and is allowed to be less accurate (`audio_archive.py:3-12`; the live model repaired 0 of 3 hard cases, the full model 3 of 3).

### 2.3 Translation (`translation_worker/translator.py`, `worker.py`)

- **Not token-streamed downstream.** `_translate_realtime` collects `response.output_text.delta` into a list and returns only at `response.done` (`translator.py:515-577`). Streaming exists on the wire to the model, but the unit that moves downstream is one sentence. Sentence granularity, not token granularity, is what lets TTS start before the turn ends.
- **Split of work per STT message** (`worker.py:755-830`): sentence 0 gets its own `translate_with_valence` call so it publishes alone as soon as it returns; sentences 1..N-1 go as one `translate_batch` call started concurrently (numbered-line output, parse mismatch falls back to concurrent per-sentence calls, `translator.py:763-874`). A speculative hit replaces the sentence-0 call with an already-running task (`_take_speculative_translation`, `worker.py:385`).
- **Realtime first, chat fallback** (`translator.py:626-724`): Realtime pool (4 sockets, effort `minimal`, 128 output tokens, 2.0 s timeout) is tried first; any exception or empty result logs `openai_realtime_translation_failed_using_chat_fallback` and falls through to chat completions with 2 retries, 0.5 s doubling (`translator.py:578-600`).
- **Context** (`worker.py:570-577`, `translator.py:650-690`): last 3 accepted source utterances (window of 4, `_CONTEXT_SEGMENTS`, `worker.py:192`) plus the durable room title/description (600 characters, Redis key `translationRoom:{id}:meeting_context`, cached per worker, `worker.py:1177`). Translator keeps the last 4 lines. Context is attached only if `_select_relevance_context` finds lexical overlap, otherwise it is dropped to save tokens (`worker.py:110-125`, `_CONTEXT_FREE_SHORT_TURNS`). The prompt also returns a valence marker in the same call (no extra round trip).
- **What is translated** is `display_text`, the clean line after the deterministic disfluency prepass, not the raw recogniser text (`worker.py:586-600`).
- **Pool hygiene**: a Realtime socket older than the vendor session cap is retired before hand-out, not after a failed request (production 2026-09-08: four 60-minute expiries within twenty seconds of start, each costing a fallback) (`translator.py:473-501`).
- **Concurrency**: `_CONCURRENCY_LIMIT` 8, `_MAX_IN_FLIGHT` 32 via `consume_pipelined` (commit `b4c1a97`: a 7.1 s STT commit used to hold every other speaker's message). Per-speaker `_PublishTurn` keeps order (wait at most `_PUBLISH_ORDER_WAIT_SECONDS` 10 s).
- **Skips before spending** (`worker.py:496-566`): control markers, paused room, translation not started, credits suspended, same-language target (passthrough, optional polish capped at 1.5 s).
- **Result fields that drive downstream timing**: `timestamp_ms`, `start_ms`, `chunk_index`, `latency_ms` (this sentence's translation time from the start of `_translate_and_publish`, including waiting for batch and for the earlier sentence's publish turn), `pipeline_latency_ms` (now minus chunk end), `is_final_chunk`, `speculative_hit` (`worker.py:1005-1016`).

### 2.4 TTS (`tts_worker/worker.py`, `synthesizer.py`, `prosody_context.py`, `livekit_publisher.py`)

Per message `process` (`worker.py:1788-1940`) does, in order: skip if room `PAUSED`; skip if source language equals target (never dub a listener into the speaker's own language); skip if route is `TEXT_ONLY_MODE` or a text-only Meet-bridge route (still emits the `final_chunk_processed` event); resolve voice variants (shared default or clone track plus one per listener-picked voice); compute catch-up lag; synthesize each variant.

- **Voice**: default catalog voice per speaker persona; clone via a ladder of growing buffers (3, 8, 45 s, config default; commit `743edc3` mentions 3, 8, 20, 45) instead of waiting for 20 s; a voice-less speaker's first dub waits at most `voice_clone_first_wait_ms` 2500 ms counted from clone start, then goes out in the catalog voice (`worker.py:813-838`, `config.py:692-695`). Far-speaker (Meet-side) clone is consent-gated (`far_speaker_clone.py`, not analysed).
- **Cache first** (`worker.py:2530-2577`): key includes text, voice id and mode, generation config (speed, prosody) and sample rate; hit publishes cached WAV (about 2 ms); an empty or header-only WAV is never cached or replayed (WT-874).
- **Synthesis** (`_synthesize_sentence`, `worker.py:1563`): with `prosody_continuity` a Cartesia context is reused for the speaker's next sentence only when it is already queued; each push uses `flush=True` and reads until the matching `flush_done` (`prosody_context.py:197-250`). Per-sentence timeout 6.0 s (`SENTENCE_TIMEOUT_SECONDS`), idle context expiry 4.0 s (server retires idle contexts at about 4.7-5.2 s). An open context holds a vendor slot even when idle: leaving 3 open moved TTFB from about 0.7 s to about 4.0 s (`prosody_context.py:42-52`).
- **First audio**: each Cartesia chunk is fed to a `TrackStream` the instant it arrives (`on_pcm`, WT-397); the stream holds back one 20 ms frame plus an 8 ms fade (`livekit_publisher.py:203`, `_FADE_MS` 76), so the added first-audio cost is about 20 ms. Frames are 20 ms; `AudioSource` queue 1000 ms (look-ahead buffer, not added delay).
- **Bot lifecycle**: one LiveKit participant per (meeting, speaker, language, voice). A cold bot costs `room.connect` plus 0.2 s settle (`_PUBLISH_SETTLE_S`); under CPU throttle it measured 4.5-17.6 s. Fixes: the join starts when the stream starts, beside Cartesia generation (`TrackStream.start`, commit `1c7ba09`); idle timeout 600 s (`LiveKitSettings.tts_bot_idle_timeout_s`, the 60 s constant is only the fallback, `livekit_publisher.py:56-59`); bots released at once when the room pauses or ends (commit `743edc3`).
- **Ordering and lag**: per (meeting, speaker, language) lock keeps order. `_catch_up_lag_ms` (`worker.py:4502`) is the larger of out-of-order gap (a sentence that belongs before one already spoken) and translation overrun beyond `_CATCH_UP_ON_TIME_MS` 1500 ms; speed multiplier ramps linearly to 1.25x at 4000 ms (`_with_catch_up`, `worker.py:4536`). A late line is spoken in full, only faster. Catch-up is part of the cache key.
- **Isochrony** (`shared/isochrony.py`): learned dub/source duration ratio per speaker and language centres the speed within 0.8-1.25, applied at `worker.py:4499`.

### 2.5 Playback (client, `warptalk-web`)

- **Track selection** (`components/rooms/live/filtered-room-audio.tsx`, replaces the default room audio renderer): the listener is subscribed only to the interpreter track for their listen language (and preferred voice) and unsubscribed with `setSubscribed(false)` from the others, so the SFU stops sending them. `onlySubscribed: false` lets the component see tracks it is not receiving.
- **"Ducking" of the original is mute, not gain**: a speaker whose language differs from the listener's has their raw mic unsubscribed only once a dub track for that speaker exists (`filtered-room-audio.tsx:42-56`). The bot is created by the first synthesized chunk, so the first sentence of a turn from a new speaker plays the original; after the bot exists the original stays muted across idle reconnects via `dubbedHistory` (WT-874, `:164-212`). No cross-fade, no gain duck.
- **Gain ducking exists only for the Meet bridge echo path**: the microphone copy sent to Meet is ducked to `MEET_AUDIO_DUCK_GAIN` 0.15 while the far side is speaking (`src/lib/meeting/meet-audio-track.ts:86-144`). Unrelated to dub playout.
- **Jitter buffer, playout delay, target latency**: no setting found in `lib/meeting`, `lib/audio`, `components/rooms/live` or `hooks`; the client uses LiveKit/WebRTC defaults (the buffer size is therefore inferred, see 3.1).
- **Captions**: `TranscriptInterimReceived` sets a live line that expires after `LIVE_LINE_MAX_AGE_MS` (`lib/transcript/live-text`, handler `persistent-meeting-session.tsx:4002-4022`); `TranscriptSegmentReceived` (`:3978`) replaces it; `TranslationTextReceived` (`:4044`) carries the translated text. `use-audio-playback.ts` and `exclusive-playback.ts` are voice-preview clip players, not part of the live dub path.

## 3. Latency budget

### 3.1 Target allocation, speaker to first dub audio, under 2000 ms

Reference point: the moment the first complete sentence of a multi-sentence turn has been spoken (flash mode, early sentence path). Hangover is not on this path because the early sentence publishes before the turn closes. Values marked "(inferred)" are this spec's allocation; the unmarked figures are the authors' measurements.

| # | Stage | Target ms | How achieved | Knobs (default) | Source |
|---|---|---|---|---|---|
| 1 | Capture, WebRTC, ingress decode, 96 ms frame window | 150 (inferred) | native 16 kHz delivery, no Python resampling; ungated live frames | `stt_streaming_enabled` true | 96 ms window `worker.py:2606`; `510abb0` |
| 2 | STT: words to sentence-complete text | 450 (inferred) | streaming model transcribes while audio arrives; early sentence published without waiting for commit; live words trail the speaker by 0-0.6 s; paced commit to completed p50 774 ms is the turn-end path, not this one | `model` `gpt-live-transcribe`; `realtime_pool_size` 4; `on_early_segment` | `bd5d7c2`, `b06b183`, `config.py:432-470` |
| 3 | Redis hops stt to translation to tts (3 reads, pipelined) | 60 (inferred) | `consume_pipelined`, `block_ms` 2000 returns on data; ack on handler success | `_MAX_IN_FLIGHT` 32 | `base_worker.py:855-890`, `b4c1a97` |
| 4 | Translation of sentence 0 | 700 | warm Realtime mini, effort `minimal`, 128 tokens; **0 on a speculative hit** | `realtime_model` `gpt-realtime-2.1-mini`; `realtime_timeout_seconds` 2.0; `_SPECULATIVE_TIMEOUT_SECONDS` 1.5 | p50 685 ms `tools/probe_streaming_lag.py:76`; warm range 655-919 ms `config.py:604-612` |
| 5 | TTS first audio, warm connection | 200 | pre-dialled sockets, streaming to track, cache hit about 2 ms | `tts_warm_pool_size` 3; `stream_to_livekit` true; `cartesia_max_concurrency` 3 | warm p50 180 ms, cold 669 ms `config.py:775-790` |
| 6 | Bot publish, SFU, client jitter buffer and decode | 150 (inferred) | bot pre-joined; 1 s AudioSource queue is look-ahead; 20 ms frames; client defaults | `tts_bot_idle_timeout_s` 600 | `livekit_publisher.py:31-44` |
| | **Total** | **1710** | leaves about 290 ms slack, about 0.7 s more with a speculative hit | | |

### 3.2 Paths that exceed the budget

| Path | Added cost | Result |
|---|---|---|
| Turn-end path, single short sentence, no early publish (unverified, see section 9) | hangover 864 ms (short turn) or 576 ms, then commit to completed 774 ms p50, then translate 685, TTS 180, plus rows 1, 3, 6 (about 360) | about 2.9 s (inferred sum); consistent with the authors' 2.7 s |
| Authors' full dub budget | STT lag about 1797 ms (word-rate median, approximate) + translation 685 ms + TTS first audio 180 ms | about 2.7 s dub lag, "a 1000ms dub target is NOT reachable with this cascade" (`tools/probe_streaming_lag.py:74-90`) |
| Cold TTS connection | 669 ms against 180 ms | row 5 grows by about 490 ms |
| Cold interpreter bot after idle (before `1c7ba09`) | join 4.5-17.6 s under throttle | dub arrived 17-29 s late in production, 2026-10-03 |
| Cartesia slot held through playout (before `aa5fb31`) | queue behind another speaker's playout | dubs p50 about 2 s, p90 about 5 s late (worst room p50 8.2 s) |

### 3.3 Targets and evidence found in code, docs and history

| Where | Figure | Status |
|---|---|---|
| `livekit_ingress_worker/audio_archive.py:4`, `retranscribe_worker/batch_transcriber.py:5` | "two-second deadline" / "two-second budget" for the live transcript | comment only, not enforced |
| `tools/probe_streaming_lag.py` `--target-ms` default 1000; `tools/probe_chunk_context.py:150` `CABIN_LAG_TARGET_MS` 1500 | cabin lag goals used to rank probe cells | probe parameters |
| `tools/probe_translate_ttfb.py:1-9` | "the <2.5s question" | probe goal |
| `tts_worker/worker.py` `_CATCH_UP_ON_TIME_MS` 1500 | translation overrun budget that triggers faster speech | the only runtime budget constant; applies to translation, not end to end |
| `.agents/resources/ai_pipeline_architecture.md:3-10` | about 1.5 s: STT 0.5, translate 0.4, TTS 0.6 on 2 s chunks | obsolete design note (Whisper era); shipped chunking differs |
| Commit `bd5d7c2` (2026-08-20) | time to first dubbed audio about 3292 ms to about 1443 ms on a 15 s human Vietnamese recording; time to first usable transcript 1467 ms to 2 ms after commit | measured once; reference point not stated, not reproduced here |
| Commit `b06b183` (2026-10-04) | a short sentence reached the caption about 2.5 s after the speaker stopped; with live text words appear at 1.2-1.5 s into speech then every about 200 ms, trailing the speaker by 0-0.6 s | measured |
| Commit `aa5fb31` (2026-10-01) | production dub lag p50 about 2 s, p90 about 5 s before the slot fix | measured |
| `config.py:432-470` (`bd5d7c2` table) | `gpt-transcribe` dumped 1651 ms, paced 1003 ms; `gpt-live-transcribe` dumped 4733 ms, paced 774 ms | measured, n=3 |

No metric measures speaker to listener (section 7), so the sub-2 s claim cannot be confirmed from production telemetry.

## 4. Optimization catalog

Format: what; why; where; knobs; trade-off.

1. **Flash mode (stream audio during speech).** Removes the whole-chunk wait before STT starts. Problem: a closed chunk took 1651 ms to complete, and 3292 ms to first dub. `bd5d7c2`; `worker.py:2606-2636`, `config.py:135`. Knobs: `stt_streaming_enabled`, room key `translationRoom:{id}:flash_mode`, platform `meetings.flash_mode_default`. Trade-off: coupled to the model; streaming model on a closed chunk is 2.9x worse (4733 ms), so rollback needs both settings.
2. **Early sentences.** Publish each sentence the model completes before the turn closes: 1649 ms for a first sentence (vi, 15 s clip), 1176 ms (en, 18.5 s) against about 5.7 s for the whole chunk. `stt_worker/worker.py:1425-1456`. Trade-off: no logprob, confidence `-1.0`, so the confidence gate cannot fire; billing carries `is_early` so an early line is not double charged.
3. **Boundary seeking.** Past 4000 ms of speech the hangover drops to 250 ms so long turns split at a breath rather than the 6 s cap. `config.py:283-334`. Knobs: `vad_seek_boundary_after_ms` 4000, `vad_seek_hangover_ms` 250. Trade-off: pause-based cuts lose BLEU against fixed windows in the literature cited by `bd5d7c2`; short turns keep a long hangover (about 288 ms extra on one-word replies) to avoid fragments.
4. **Live text lane.** Words are published as the model emits them, at most every 150 ms per item, sentence ends always sent; one reader per socket drains events to a queue. `stt_worker/model.py:97, 3091-3111`. Why: captions arrived about 2.5 s after the speaker stopped because nothing read the socket until commit. Trade-off: preview only, never stored, billed or translated; masked under the same profanity switch.
5. **Speculative translation.** Finished delta sentences on `stt:speculative` are translated ahead into a cache keyed by normalised text, 15 s TTL, one-slot semaphore, 1.5 s per call. `translation_worker/worker.py:193-223, 289-400`. Trade-off: if one translation costs more than 1.5 s speculation is decoration (`tools/probe_translate_ttfb.py:3-8`); output is cached, never published unvalidated. In flash mode the early path replaced the speculative cache for STT (`stt_worker/worker.py:1450-1456`).
6. **Realtime socket for sentence 0, one batch for the rest, concurrently.** Avoids a full round trip per extra sentence. `worker.py:755-830`, `translator.py:763`. Knobs: `realtime_model`, `realtime_reasoning_effort` `minimal`, `realtime_max_output_tokens` 128, `realtime_timeout_seconds` 2.0. Why these values: a premature HTTP fallback turned a rare 1.2 s answer into a 5-19 s cliff; 64 tokens ended `incomplete`; higher effort slowed or broke it (`tools/probe_realtime_effort.py`, `config.py:640-647`). Trade-off: Realtime mini repaired 0 of 3 mishearings, the larger model 3 of 3 at similar latency (`config.py:625-637`).
7. **Model and socket warm-up.** Startup request to prime DNS, TLS and routing (cold first request above 3 s); Realtime pool primed per slot (`translator.py:375-428`). STT warm pool of 4 sockets, rotated before vendor expiry, refilled by a 60 s task, prepared on `meeting.participant_joined` rather than first unmute (`87a63ff`, `743edc3`). TTS warm pool of 3 Cartesia sockets (dial p50 427.6 ms against 0.1 ms to open a context, `synthesizer.py:125-195`). Trade-off: idle vendor sessions and participant minutes.
8. **Connection reuse.** Realtime translation sockets are exclusive per request and returned healthy (`translator.py:503`); Cartesia contexts reused only when the next sentence is queued; LiveKit bots kept 600 s so a pause costs no join. Trade-off: a held bot keeps the original muted on web and costs participant minutes.
9. **Bot join in parallel with generation.** `TrackStream.start` begins the join before the first Cartesia chunk (`1c7ba09`, `livekit_publisher.py:316-325`). Removes 4.5-17.6 s handshake from the first audio under throttle.
10. **Release the vendor slot at `flush_done`, not at playout.** Since WT-397 a streamed sentence returns only after playout; the slot used to wrap that whole span. `aa5fb31`, `worker.py:1514-1538`. Knob: `cartesia_max_concurrency` 3. Trade-off: the plan limit is divided by replicas; burst waits are visible only in `tts_slot_wait`.
11. **Pipelined consume.** A finished message frees its slot and the next read happens at once (`b4c1a97` for STT and translation; TTS earlier). Per-speaker order kept by speaker lock, publish ticket and key lock.
12. **Do not wait on the next turn.** The next turn keeps streaming while the previous one is transcribed (`84f0ed7`); deltas are tracked per item so words of one turn never lead another (`3a63e4d`).
13. **Cache and determinism.** `temperature` 0.0 so a repeated sentence translates identically and hits `tts:cache:{sha256}` (TTL 3600 s, about 2 ms against about 1 s). Cache key includes speed and prosody, so catch-up or a shouted "okay" never replays a calm render. `worker.py:2530-2577`.
14. **Native resampling.** `AudioStream(16 kHz mono)` instead of a per-frame Python resampler (`510abb0`): less CPU on the ingress, which had an OOM and throttling history (`743edc3`).
15. **ASR-repair clause gating.** The repair instruction and glossary are sent only when evidence of a mishearing or meeting context exists, saving prompt tokens on the hot path (`translator.py:70-124`, `tools/probe_gate_hit_rate.py`).
16. **Catch-up speech.** A dub behind the speaker reads up to 1.25x faster, ramping from 1500 ms to 4000 ms of lag; nothing is dropped. `worker.py:660-683, 4502-4562`, commit `d2e6311`. Trade-off: comprehension ceiling, so the 1.25 cap sits under the 1.5 prosody maximum.
17. **Isochrony.** Speed centred on each speaker's measured dub-to-source duration ratio, clamped 0.8-1.25, so a dub fits the speaker's gap and does not accumulate backlog. `shared/isochrony.py`, `worker.py:4499`; `speed` default `fast`.
18. **Far-side gating to avoid wasted STT and TTS.** `far_side_gate` (frames, off by default; removes about 20 % of duplicate frames on a synthetic eval) and `far_side_dedupe_*` (text, on) stop Meet-bridge duplicates from reaching translation and TTS; `text_only_dub_for` skips dub where no cable exists. Off by default for recall. `config.py:358-405, 533-549`.
19. **Caption lane polling.** The gateway's caption loops poll every 25 ms (was 200 ms, half of which every caption paid on average); `RedisStreamService` cannot block. `AiResultConsumerService.cs:46-55, 1003-1060`.
20. **Skip before spend.** Control marker, pause, translation-not-started, credits-suspended and same-language checks run before any LLM call or synthesis (`translation_worker/worker.py:496-566`, `tts_worker/worker.py:1788-1860`).

Not found: dedicated first-chunk sizing for TTS (chunking is whatever Cartesia emits, passed through at 20 ms frames), time-to-first-token streaming of translation into TTS, cross-vendor provider fallback, and queue-age shedding (section 5).

## 5. Failure and degradation

| Failure or overrun | Behaviour | Source |
|---|---|---|
| STT flash socket fails or streaming empty | closed chunk on `audio:chunks` still carries the whole turn; empty streamed commit on speech of 0.8 s or more is retried with the chunk's audio | `worker.py:2606`, commit `743edc3` |
| STT low confidence (non-early) | discard under `min_avg_logprob` -0.7 | `config.py:486-502` |
| Realtime translation error, empty or over 2.0 s | chat completions model, 2 retries (0.5 s doubling) | `translator.py:626-724, 578` |
| Batch parse mismatch | concurrent per-sentence calls | `translator.py:763-874` |
| Speculative translation over 1.5 s or unclaimed 15 s | abandoned, translated on demand | `translation_worker/worker.py:193-223` |
| Same-language polish over 1.5 s | raw text ships | `worker.py:198-201` |
| Earlier sentence's publish late | later sentence waits up to 10 s, then proceeds, nothing dropped | `worker.py:151` |
| Translation slow beyond 1500 ms or out of order | dub spoken faster, up to 1.25x; never dropped | `tts_worker/worker.py:4502-4562` |
| Cartesia context stalls | 6.0 s per-sentence timeout, context retired; fall back to one-shot synthesis, ending the context first so it never holds two slots | `prosody_context.py:86`, `worker.py:1563` |
| Cartesia transient failure (drop, 5xx, timeout) | one retry, 0.5 s backoff, within 10 s, inside the key lock so order holds; 4xx or exhausted goes to `translate:results:dead-letter` | `config.py:826-846` |
| Voice clone not ready | first dub waits at most 2500 ms from clone start, then catalog voice | `worker.py:813-838` |
| Room `PAUSED` | no translation, no TTS | `tts_worker/worker.py:1795` |
| `TEXT_ONLY_MODE`, text-only Meet-bridge route | captions only, TTS skipped, `final_chunk_processed` still emitted | `worker.py:1858-1880` |
| Credits suspended | translation stops before the LLM call, so no TTS | `translation_worker/worker.py:546` |
| LiveKit bot cannot publish | one evict-and-resume retry per slice, then the sentence is marked broken (no per-chunk reconnects); fallback to non-streaming publish | `livekit_publisher.py:331-385` |
| Dub not yet on the wire | web plays the original rather than silence | `filtered-room-audio.tsx:50-56` |
| Redelivery after a crash (reclaimed message at least `processing_timeout_seconds` + margin old, 150 s) | if a later sentence of the same speaker is already dubbed, the stale one is dead-lettered as `superseded` instead of spoken; restart clears the map so it is spoken | `worker.py:1023-1046` |
| Stuck pending | gateway acks entries pending over 5 minutes undelivered | `AiResultConsumerService.cs:110, 309` |
| Poison message | `max_delivery_attempts` 5 then dead letter; heartbeat 10 s, TTL 30 s | `base_worker.py:112-125, 899-1049` |

Budget exceeded: there is no hard deadline path that skips TTS or drops a late dub. The only adaptive responses are faster speech (catch-up), supersession of a reclaimed stale line, and the captions lane, which is independent of TTS and so continues if dubbing is late or fails. **Backpressure and queue-age shedding:** per-stage `_MAX_IN_FLIGHT` 32 bounds concurrency; `block_ms` 2000; `stream_maxlen` 200 per stream (a `tts:results` entry is about 113 KB) and `stream_ttl_seconds` 3600 bound memory (entries can be trimmed under load, which is lossy); `audio:frames` is capped at 2000 and ephemeral. No code drops a message because it is too old. A deep TTS backlog is therefore caught up with speed (max 1.25x), not shed.

## 6. Capacity and configuration knobs

Env prefixes: `WorkerSettings` none (field name uppercased), `STTSettings` `STT_`, `TranslationSettings` `TRANSLATION_`, `TTSSettings` `TTS_`, `RedisSettings` `REDIS_`, LiveKit settings under their own prefix. Values are code defaults at `d0db9b9`; deployments override.

| Knob (class) | Default | Effect on latency |
|---|---|---|
| `stt_streaming_enabled` (Worker) | true | Flash mode |
| `chunk_duration_ms` (Worker; platform `meetings.chunk_duration_ms` 2000-15000) | 6000 | Hard cap; lower cuts sentences |
| `vad_threshold` / `vad_release_threshold` / `vad_pre_speech_ms` / `vad_min_speech_ms` | 0.35 / 0.20 / 384 / 288 | Detection, no hangover cost |
| `vad_silence_hangover_ms` / `vad_seek_hangover_ms` / `vad_short_turn_hangover_ms` | 576 / 250 / 864 | End-of-turn wait ladder |
| `vad_seek_boundary_after_ms` / `vad_short_turn_speech_ms` | 4000 / 1500 | Ladder thresholds |
| `near_field_gate_enabled` / `far_side_gate_enabled` | false / false | Recall over precision |
| `ingress_energy_relative_ratio` | 0.4 | Quiet-mic rescue |
| `model` (STT) / `realtime_pool_size` | `gpt-live-transcribe` / 4 | Streaming model, warm sockets |
| `min_avg_logprob` (STT) | -0.7 | Discard floor |
| `far_side_dedupe_enabled` (STT) | true | Drops duplicates |
| `model` / `realtime_model` (Translation) | `gpt-5.4-nano` / `gpt-realtime-2.1-mini` | Fallback / first-sentence |
| `realtime_reasoning_effort` / `realtime_max_output_tokens` / `realtime_timeout_seconds` / `realtime_pool_size` | `minimal` / 128 / 2.0 / 4 | First-sentence latency |
| `max_tokens` / `temperature` (Translation) | 512 / 0.0 | Output cap, cache determinism |
| `model` / `speed` / `sample_rate` (TTS) | `sonic-3.5` / `fast` / 44100 | Voice and pace |
| `stream_to_livekit` / `prosody_continuity` / `cache_enabled` / `cache_ttl_seconds` (TTS) | true / true / true / 3600 | Streaming, context reuse, cache |
| `tts_warm_pool_size` / `cartesia_max_concurrency` | 3 / 3 | Warm sockets, vendor slots |
| `synthesis_max_retries` / `_backoff_seconds` / `_retry_window_seconds` | 1 / 0.5 / 10.0 | Bounded retry |
| `voice_clone_ladder_seconds` / `voice_clone_first_wait_ms` | (3, 8, 45) / 2500 | Clone ladder, bounded wait |
| `tts_bot_idle_timeout_s` (LiveKit settings) | 600 | Bot reuse |
| `stream_maxlen` / `stream_ttl_seconds` (Redis) | 200 / 3600 | Memory bound |
| Constants (not env) | `_CATCH_UP_ON_TIME_MS` 1500, `_CATCH_UP_FULL_LAG_MS` 4000, `_CATCH_UP_MAX_SPEED` 1.25, `_SPECULATIVE_TIMEOUT_SECONDS` 1.5, `_SPECULATIVE_TTL_SECONDS` 15, `_POLISH_TIMEOUT_SECONDS` 1.5, `_PUBLISH_ORDER_WAIT_SECONDS` 10, `SENTENCE_TIMEOUT_SECONDS` 6.0, `CONTEXT_IDLE_EXPIRY_SECONDS` 4.0, `_LIVE_TEXT_MIN_INTERVAL_S` 0.15, `AUDIO_SOURCE_QUEUE_MS` 1000, `CaptionPollDelay` 25 ms | Fixed in code |

## 7. Observability

**Worker histograms** (Redis hash `warptalk:latency:{stage}` written by `record_latency`, scraped by `metrics_exporter` as Prometheus text on `METRICS_PORT` 9108, buckets 250, 500, 1000, 2000, 3000, 5000, 8000, 12000, 20000 ms; `shared/redis_client.py:36-37, 418-461`):

| Metric | Measures | Source |
|---|---|---|
| `warptalk_stage_latency_ms{stage="stt"}` | now minus chunk end, observed when STT starts the chunk (`stt_worker/worker.py:1201-1202`) | `publish_telemetry` |
| `{stage="translation"}` | now minus chunk end, observed at `process` entry (`translation_worker/worker.py:562-564`) | `publish_telemetry` |
| `{stage="tts"}` | now minus chunk end, observed at `process` entry (`tts_worker/worker.py:1801-1803`) | `publish_telemetry` |
| `{stage="tts_first_audio"}` | first Cartesia attempt start to first sample on the track; what a listener waits on after TTS starts (`worker.py:2583-2711`) | `record_latency` |
| `{stage="tts_synthesis"}` | worker busy time, includes playout under streaming; MUST NOT be read as wait | `worker.py:2685-2690` |
| `{stage="tts_slot_wait"}` | time waiting for a Cartesia slot, zeros included so count equals sentence count | `worker.py:2695` |
| `warptalk_stage_messages_total{stage,outcome}` | ok, error, timeout, vendor_error, dead_letter | `redis_client.py:46` |
| `redis_stream_group_lag`, `_messages_pending`, `_consumers`, `redis_stream_length` (dead-letter) | per stream and group from `XINFO GROUPS` | `metrics_exporter/metrics.py` |
| `warptalk:provider_calls:{date}` hashes | every OpenAI and Cartesia call timed to headers (TTFA for Cartesia) | `shared/provider_calls.py` |
| `translationRoom:telemetry` pub/sub | live payload `{roomId, workerType, latencyMs}` | `redis_client.py:418-437` |

Because the chunk timestamp is carried unchanged, the three stage values are cumulative ages since the end of the turn, not per-stage durations: the per-stage cost is the difference between consecutive stages. `tts` is the age at which TTS starts; adding `tts_first_audio` gives about the end-of-turn to first-sample figure on the worker side, excluding the VAD hangover, bot publish and client playout. Translation also writes `latency_ms` and `pipeline_latency_ms` into each `translate:results` entry (persisted to `translation_contents.latency_ms`).

**Gateway** (`MeetingCaptionMetrics.cs`, meter `warptalk-gateway`): `meeting.captions.delivered{kind=transcript|translation|audio}`, `meeting.captions_started`, `meeting.time_to_first_caption` (first join to first caption, seconds, per meeting). Best effort, so a metric cannot delay a caption.

**Not instrumented**: speaker to listener latency, client playout delay, VAD hangover, and the web's own caption render time. No alert rules or dashboards for these were found in the three repos; `.agents/resources/ai_pipeline_architecture.md` autoscaling thresholds (STT lag over 100, TTS lag over 50) are a design note with no config behind them.

**Probe tools** (`warptalk-ai/tools/`, run by hand against live vendors, docstring summaries):

| Tool | Measures |
|---|---|
| `probe_streaming_lag.py` | STT lag behind a paced live speaker per provider; prints the full dub budget against `--target-ms` |
| `probe_commit_semantics.py` | whether Realtime `commit()` consumes or grows the buffer |
| `probe_chunk_context.py` | whether prior sentences as context rescue chunked STT; ranks cells against a 1500 ms cabin budget |
| `probe_translate_ttfb.py` | warmed translation p50/p95/max on the production path; count over the 1.5 s speculative budget |
| `probe_realtime_effort.py` | Realtime reasoning-effort sweep against repair and latency |
| `probe_latency_ab.py` | paired, interleaved 2x2 prompt by model latency A/B |
| `probe_tts_first_audio.py` | cold versus warm time to first sample, taken at the same `on_pcm` tee that feeds LiveKit |
| `probe_energy_floor.py` | what the 0.02 RMS floor rejects |
| `probe_world_roundtrip.py` | cost of reshaping pitch after synthesis |
| `probe_gate_hit_rate.py`, `probe_gate_safety.py`, `probe_gate_wiring.py` | repair-clause gate: hit rate, safety, real prompt sent |
| `probe_guardian_vs_prod.py`, `probe_hard_case_fluency.py`, `probe_which_part_repairs.py`, `probe_context_only_repair.py` | repair prompt quality and which half does the work |
| `replay_livekit_chunk.py` | replays a retained chunk through a real LiveKit track for full-path latency |
| `tests/test_e2e_latency.py` | translation plus TTS first-byte only, no Redis, no LiveKit |

## 8. Known trade-offs

- Flash mode and the streaming STT model are one decision (section 4, item 1).
- Early sentences gain seconds and give up the logprob gate; foreign-script leaks measured 0 of 6 is evidence, not a guarantee.
- The 6 s cap with boundary seeking can still cut a speaker who never pauses.
- Realtime first sentence is fast and cannot repair mishearings; switching model is a deploy decision.
- Realtime timeout 2.0 s trades tail latency for avoiding the HTTP-fallback cliff.
- Catch-up and isochrony raise speech rate up to 1.25x; bounded for intelligibility.
- Idle-held bots, warm sockets and the 4-socket pools spend vendor sessions and participant minutes to buy first-sentence latency.
- `stream_maxlen` 200 trades replay depth for Redis memory.

## 9. Open questions

1. **Single short sentence.** Does `on_early_segment` fire for a one-sentence turn before commit, or does that sentence wait for hangover (576-864 ms) plus commit to completed? The answer decides whether short turns meet 2 s (section 3.2). Needs a trace of one short utterance through `stt:results` entry ids.
2. **Reference point of the 1443 ms figure** in commit `bd5d7c2` (speech start, turn end or chunk end) and whether it included flash mode early sentences.
3. **Client playout delay.** No jitter buffer or playout delay setting was found in the web directories read; actual WebRTC buffering on the listener (row 6) is inferred. The rest of `warptalk-web` and any LiveKit server-side config were not read.
4. **Backend side of dub routing.** Room route states (`AUDIO_ROUTING_ACTIVE`, `PAUSED`, `TEXT_ONLY_MODE`) are consumed in tts_worker but their producer (translation-room service) was not read; how fast a route change propagates is unknown.
5. **Is the `tts:results` audio payload still used?** The gateway forwards it as `TranslatedAudioReceived`, but no web handler was found; if unused, base64 WAV of about 113 KB per sentence is pure overhead on the gateway and Redis.
6. **Production overrides** of env and platform settings (`chunk_duration_ms`, `flash_mode_default`, pool sizes, replica counts, Cartesia concurrency per replica) and any deployed alerting or autoscaling.
7. **Current p50/p95 in production** after the October fixes (`aa5fb31`, `1c7ba09`, `743edc3`); the last recorded production figure is p50 about 2 s, p90 about 5 s, before them.
8. **Cartesia concurrency**: `aa5fb31` describes 2 slots per process, config default is 3; which value the plan allows is not determined.
9. **Out of scope and unread**: voice clone quality thresholds, far-speaker consent flow, orphan voice sweep, the Meet bridge audio path in the backend.
