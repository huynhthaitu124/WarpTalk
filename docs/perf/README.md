# Pipeline latency: measurement and optimizations

Scope: speech end (last speech frame of a turn) to what the room hears and reads, through the
real room worker, VAD/endpointer, `RoomPipeline`, MT stage, TTS stage and dub publisher of
`apps/pipeline`, with provider doubles that follow spec 08 section 1. Harness: `pipeline-bench`
(`apps/pipeline/src/bench.rs`). Stage numbers are the `warptalk_stage_latency_ms` histograms the
production code records, read back from an in-memory recorder.

Files: `baseline.json` (both baselines), `optimized.json` (both runs with the shipped defaults),
`runs.json` (every other run of the tables below).

## What is simulated

| Stage | Model | Source |
|---|---|---|
| Capture | real-time paced synthetic speech, 48 kHz / 20 ms blocks, per-turn speech length +-30% | spec 08 3.1 row 1 |
| VAD / endpoint | the real `Endpointer` (energy VAD), ladder 864 / 576 / 250 ms | spec 08 2.1 |
| STT (`JitterStt`) | streaming: a sentence is published as an early segment after its audio ended, log-normal p50 450 / p95 800 ms (live text trails the speaker 0-0.6 s); remainder / done at commit + log-normal p50 774 / p95 1300 ms | spec 08 3.1 row 2, section 4 |
| MT (`FakeMt`) | Realtime first sentence log-normal p50 685 / p95 900 ms (first token 80 ms earlier), batch p50 800 ms, drawn per call | spec 08 3.1 row 4 |
| TTS (`FakeTts`) | first audio log-normal p50 180 / p95 260 ms warm; a context open finds no warm socket 20% of the time and then costs +490 ms (669 ms cold); audio delivered at 2x real time in 100 ms chunks | spec 08 3.1 row 5, 3.2 |
| Publish | `SimulatedTransport` records the instant of every `publish_dub` / `publish_data` | |

Load: 4 rooms x 4 speakers, one target language, 90 s of audio per speaker (about 290 turns per
run), seed 42, a 4-core box (4 runs at a time in parallel; CPU use is 3-6% of one core
per run, so they do not disturb each other). Two turn shapes:

- **Scenario A, single-sentence turns** (speech 2.0 s +-30%, pause 3.0 s): the case the target is
  about ("speaker stops, how long until the dub is audible").
- **Scenario B, two-sentence turns** (speech 3.0 s +-30%, sentence 2.0 s, pause 2.0 s): the first
  sentence is dubbed while the speaker is still talking, so `e2e_first_dub_audio` saturates at 0
  for many turns. `e2e_last_dub_audio` (first audio of the dub of the turn's last sentence) is the
  honest end-of-turn figure there.

Metrics added by the chain, all relative to the last speech frame of the turn (0 when earlier):
`e2e_first_caption`, `e2e_final_caption`, `e2e_first_translated_caption`, `e2e_first_dub_audio`
(first dub chunk of the turn published, mouth-to-ear proxy at the publish point, excludes SFU and
jitter buffer, spec 08 row 6: about 150 ms), `e2e_last_dub_audio`. The speech end of a turn is
only known at commit, so they are computed when the end-of-turn marker arrives.

## Baseline (all optimizations off)

Command: `pipeline-bench --rooms 4 --speakers 4 --seconds 90 --seed 42 --baseline [--speech-ms 2000 --pause-ms 3000 --sentence-ms 4000]`

Baseline behaviour: the early sentences of a turn are held and translated together when the STT
reports the turn complete (`early_mt` off); the VAD hangover is the plain ladder; TTS contexts are
opened when the translation reaches TTS.

| stage (ms), p50 / p95 / p99 | baseline | default |
|---|---|---|
| `endpoint_decision` | 580 / 860 / 880 | 580 / 860 / 880 |
| `stt_first_text` | 0 / 290 / 635 | 0 / 255 / 567 |
| `stt_final` | 782 / 1328 / 1577 | 792 / 1303 / 1584 |
| `mt_first_token` | 610 / 863 / 931 | 610 / 863 / 931 |
| `mt_done` | 691 / 944 / 1013 | 691 / 945 / 1012 |
| `tts_first_audio` | 196 / 699 / 746 | 182 / 259 / 301 |
| `tts_slot_wait` | 0 / 0 / 0 | 0 / 0 / 0 |
| `e2e_first_caption` | 468 / 815 / 1079 | 461 / 813 / 986 |
| `e2e_final_caption` | 468 / 815 / 1079 | 461 / 813 / 986 |
| `e2e_first_translated_caption` | 2111 / 2722 / 2911 | 1172 / 1570 / 1864 |
| `e2e_first_dub_audio` | 2361 / 3080 / 3388 | 1354 / 1768 / 2035 |
| `e2e_last_dub_audio` | 2361 / 3080 / 3388 | 1354 / 1768 / 2035 |

(scenario A; the right column is the shipped default, see below)

Scenario B:

| stage (ms), p50 / p95 / p99 | baseline | default |
|---|---|---|
| `endpoint_decision` | 580 / 581 / 581 | 580 / 581 / 582 |
| `stt_first_text` | 0 / 0 / 430 | 0 / 0 / 512 |
| `stt_final` | 769 / 1275 / 1469 | 757 / 1356 / 1592 |
| `mt_first_token` | 600 / 833 / 914 | 602 / 831 / 930 |
| `mt_done` | 740 / 1038 / 1277 | 682 / 913 / 1012 |
| `tts_first_audio` | 184 / 639 / 729 | 180 / 262 / 324 |
| `tts_slot_wait` | 0 / 0 / 0 | 0 / 0 / 0 |
| `e2e_first_caption` | 0 / 155 / 609 | 0 / 192 / 565 |
| `e2e_final_caption` | 455 / 803 / 1111 | 446 / 837 / 1110 |
| `e2e_first_translated_caption` | 1979 / 2537 / 2766 | 0 / 924 / 1311 |
| `e2e_first_dub_audio` | 2203 / 2920 / 3225 | 65 / 1101 / 1462 |
| `e2e_last_dub_audio` | 3715 / 4664 / 5243 | 1606 / 2413 / 2789 |

Other baseline figures (scenario A / B): frames dropped 0 / 0, media events dropped 0 / 0, CPU
3.3% / 4.6% of one core for 16 simultaneous speakers, wall 93 s / 96 s (90 s of audio plus the drain
of the last dub).

## The three flags (config keys under `latency` in the `pipeline` settings namespace)

| Flag | Default | Effect |
|---|---|---|
| `early_endpoint_hangover_ms` (a) | 0 (off) | once the streaming STT produced a sentence terminator for the turn in progress and the VAD is in its hangover, the hangover of that turn is cut to this value (250 ms = the seek rung); needs the STT to publish sentences before the commit |
| `early_mt` (b) | true | every early STT sentence goes to MT the moment it arrives; off = held and translated together at turn end |
| `tts_prewarm` (c) | true | the TTS contexts of the target languages are opened when a segment enters MT, so a cold dial overlaps the translation |
| `stage_channel_capacity` (d) | 64 | bounded channels between STT, MT, TTS and the publishers |

The same flags exist as bench options (`--early-endpoint-ms`, `--no-early-mt`, `--no-tts-prewarm`,
`--stage-capacity`, `--baseline`).

## Steps, one change at a time (cumulative, then each alone)

Scenario A, single-sentence turns:

| stage (ms), p50 / p95 / p99 | baseline | +a | +a+b | +a+b+c | b only | c only | default (b+c) | default seed 7 |
|---|---|---|---|---|---|---|---|---|
| `endpoint_decision` | 580 / 860 / 880 | 480 / 581 / 800 | 480 / 581 / 800 | 480 / 581 / 800 | 580 / 860 / 880 | 580 / 860 / 880 | 580 / 860 / 880 | 580 / 860 / 881 |
| `stt_first_text` | 0 / 290 / 635 | 0 / 289 / 634 | 0 / 289 / 634 | 0 / 346 / 568 | 0 / 291 / 561 | 0 / 287 / 716 | 0 / 255 / 567 | 0 / 252 / 646 |
| `stt_final` | 782 / 1328 / 1577 | 781 / 1327 / 1577 | 782 / 1328 / 1576 | 790 / 1246 / 1465 | 749 / 1279 / 1513 | 760 / 1291 / 1434 | 792 / 1303 / 1584 | 777 / 1284 / 1534 |
| `mt_first_token` | 610 / 863 / 931 | 610 / 864 / 931 | 610 / 863 / 931 | 610 / 863 / 932 | 610 / 863 / 931 | 610 / 863 / 931 | 610 / 863 / 931 | 589 / 825 / 915 |
| `mt_done` | 691 / 944 / 1013 | 691 / 946 / 1013 | 692 / 944 / 1012 | 690 / 944 / 1012 | 691 / 944 / 1012 | 691 / 944 / 1012 | 691 / 945 / 1012 | 670 / 906 / 995 |
| `tts_first_audio` | 196 / 699 / 746 | 196 / 694 / 745 | 196 / 700 / 785 | 181 / 260 / 316 | 194 / 688 / 742 | 181 / 266 / 304 | 182 / 259 / 301 | 182 / 264 / 297 |
| `tts_slot_wait` | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| `e2e_first_caption` | 468 / 815 / 1079 | 467 / 816 / 1079 | 467 / 815 / 1079 | 472 / 847 / 1080 | 435 / 834 / 1017 | 439 / 834 / 1140 | 461 / 813 / 986 | 452 / 782 / 967 |
| `e2e_final_caption` | 468 / 815 / 1079 | 467 / 816 / 1079 | 467 / 815 / 1079 | 472 / 847 / 1080 | 435 / 834 / 1017 | 439 / 834 / 1140 | 461 / 813 / 986 | 452 / 782 / 967 |
| `e2e_first_translated_caption` | 2111 / 2722 / 2911 | 1953 / 2569 / 2752 | 1161 / 1627 / 1959 | 1178 / 1597 / 1867 | 1150 / 1614 / 1819 | 2067 / 2702 / 2970 | 1172 / 1570 / 1864 | 1141 / 1624 / 1796 |
| `e2e_first_dub_audio` | 2361 / 3080 / 3388 | 2213 / 2911 / 3216 | 1407 / 2007 / 2444 | 1362 / 1795 / 2018 | 1406 / 2001 / 2374 | 2249 / 2904 / 3135 | 1354 / 1768 / 2035 | 1330 / 1787 / 2002 |
| `e2e_last_dub_audio` | 2361 / 3080 / 3388 | 2213 / 2911 / 3216 | 1407 / 2007 / 2444 | 1362 / 1795 / 2018 | 1406 / 2001 / 2374 | 2249 / 2904 / 3135 | 1354 / 1768 / 2035 | 1330 / 1787 / 2002 |

Scenario B, two-sentence turns:

| stage (ms), p50 / p95 / p99 | baseline | +a | +a+b | +a+b+c | b only | c only | default (b+c) | default seed 7 |
|---|---|---|---|---|---|---|---|---|
| `endpoint_decision` | 580 / 581 / 581 | 440 / 580 / 581 | 421 / 581 / 581 | 440 / 581 / 582 | 580 / 581 / 582 | 580 / 581 / 582 | 580 / 581 / 582 | 580 / 581 / 581 |
| `stt_first_text` | 0 / 0 / 430 | 0 / 0 / 378 | 0 / 0 / 566 | 0 / 0 / 592 | 0 / 0 / 429 | 0 / 0 / 578 | 0 / 0 / 512 | 0 / 0 / 212 |
| `stt_final` | 769 / 1275 / 1469 | 776 / 1280 / 1591 | 768 / 1282 / 1532 | 770 / 1211 / 1551 | 788 / 1336 / 1591 | 765 / 1183 / 1477 | 757 / 1356 / 1592 | 811 / 1375 / 1597 |
| `mt_first_token` | 600 / 833 / 914 | 600 / 831 / 972 | 601 / 830 / 924 | 601 / 831 / 930 | 601 / 831 / 924 | 602 / 833 / 973 | 602 / 831 / 930 | 609 / 830 / 922 |
| `mt_done` | 740 / 1038 / 1277 | 730 / 1063 / 1260 | 683 / 911 / 1005 | 683 / 912 / 1011 | 683 / 912 / 1005 | 729 / 1060 / 1206 | 682 / 913 / 1012 | 690 / 911 / 1002 |
| `tts_first_audio` | 184 / 639 / 729 | 183 / 664 / 718 | 185 / 684 / 739 | 179 / 263 / 304 | 192 / 673 / 735 | 179 / 257 / 293 | 180 / 262 / 324 | 180 / 262 / 296 |
| `tts_slot_wait` | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| `e2e_first_caption` | 0 / 155 / 609 | 0 / 188 / 513 | 0 / 98 / 614 | 0 / 124 / 613 | 0 / 132 / 507 | 0 / 154 / 580 | 0 / 192 / 565 | 0 / 338 / 556 |
| `e2e_final_caption` | 455 / 803 / 1111 | 448 / 804 / 1152 | 441 / 795 / 1058 | 456 / 789 / 888 | 450 / 792 / 912 | 449 / 820 / 1036 | 446 / 837 / 1110 | 441 / 777 / 879 |
| `e2e_first_translated_caption` | 1979 / 2537 / 2766 | 1870 / 2431 / 2730 | 0 / 877 / 1341 | 0 / 932 / 1339 | 0 / 828 / 1179 | 1975 / 2402 / 2594 | 0 / 924 / 1311 | 0 / 1034 / 1241 |
| `e2e_first_dub_audio` | 2203 / 2920 / 3225 | 2117 / 2811 / 3208 | 75 / 1141 / 1570 | 0 / 1106 / 1539 | 74 / 1223 / 1648 | 2157 / 2640 / 2876 | 65 / 1101 / 1462 | 159 / 1207 / 1475 |
| `e2e_last_dub_audio` | 3715 / 4664 / 5243 | 3566 / 4744 / 5091 | 1716 / 2554 / 2971 | 1585 / 2340 / 2748 | 1723 / 2616 / 2934 | 3603 / 4658 / 5316 | 1606 / 2413 / 2789 | 1639 / 2528 / 2687 |

Reading the tables (first dub = last dub in A):

| Change | A: first dub p50 / p95 | Verdict |
|---|---|---|
| baseline | 2361 / 3080 | reference |
| (a) early endpoint 250 ms | 2213 / 2911 (-148 / -169) | real gain when MT waits for the turn end, because the commit is then on the critical path |
| (b) early MT, on top of (a) | 1407 / 2007 (-806 / -904) | the big win: translation starts at the early sentence (about 450 ms after speech end) instead of after hangover + STT completion (about 1350 ms) |
| (c) TTS pre-warm, on top of (a)+(b) | 1362 / 1795 (-45 / -212) | p50 small, p95 and p99 improve: TTS first audio p95 700 -> 260 ms |
| (a) on top of (b) | 1406 -> 1407 p50, 2001 -> 2007 p95 (b only vs a+b) | no gain: with early MT the dub no longer waits for the commit, so (a) only moves `endpoint_decision` (p50 580 -> 480 ms) and `stt_final`, not the dub |

Decision by the measurement: `early_mt` = true, `tts_prewarm` = true, `early_endpoint_hangover_ms` =
0. (a) is kept as a flag for deployments that run with `early_mt` off (-148 ms p50 there); with
`early_mt` on it measured no benefit and still carries the risk of splitting a turn at a
mid-sentence pause, so it is not on by default. The shipped default is therefore b + c (rows
"default (b+c)" and "default seed 7" above; the second seed is the repeatability check).

## (d) Other candidates, measured on the default configuration (scenario A)

| stage (ms), p50 / p95 / p99 | default | 1 worker thread | capacity 4 | tts concurrency 3 | 2 languages |
|---|---|---|---|---|---|
| `endpoint_decision` | 580 / 860 / 880 | 580 / 860 / 880 | 580 / 860 / 880 | 580 / 860 / 880 | 580 / 860 / 880 |
| `stt_first_text` | 0 / 255 / 567 | 0 / 384 / 716 | 0 / 278 / 634 | 0 / 329 / 545 | 0 / 247 / 562 |
| `stt_final` | 792 / 1303 / 1584 | 775 / 1242 / 1514 | 762 / 1253 / 1528 | 781 / 1259 / 1467 | 766 / 1276 / 1594 |
| `mt_first_token` | 610 / 863 / 931 | 610 / 864 / 932 | 610 / 863 / 931 | 610 / 863 / 931 | 601 / 829 / 930 |
| `mt_done` | 691 / 945 / 1012 | 690 / 945 / 1013 | 691 / 944 / 1013 | 691 / 944 / 1012 | 683 / 910 / 1012 |
| `tts_first_audio` | 182 / 259 / 301 | 179 / 258 / 321 | 177 / 252 / 321 | 180 / 258 / 331 | 179 / 260 / 301 |
| `tts_slot_wait` | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 | 0 / 0 / 0 |
| `e2e_first_caption` | 461 / 813 / 986 | 461 / 840 / 1139 | 472 / 813 / 1133 | 443 / 833 / 1080 | 421 / 798 / 1048 |
| `e2e_final_caption` | 461 / 813 / 986 | 461 / 840 / 1139 | 472 / 813 / 1133 | 443 / 833 / 1080 | 421 / 798 / 1048 |
| `e2e_first_translated_caption` | 1172 / 1570 / 1864 | 1174 / 1618 / 1911 | 1177 / 1617 / 1831 | 1148 / 1576 / 1791 | 1062 / 1444 / 1672 |
| `e2e_first_dub_audio` | 1354 / 1768 / 2035 | 1369 / 1799 / 2195 | 1354 / 1795 / 2006 | 1340 / 1782 / 1984 | 1239 / 1639 / 1877 |
| `e2e_last_dub_audio` | 1354 / 1768 / 2035 | 1369 / 1799 / 2195 | 1354 / 1795 / 2006 | 1340 / 1782 / 1984 | 1318 / 1717 / 2002 |

None of them moves the dub: the harness uses 3-6% of one core, so worker count, channel capacity
and the slot gate (`tts.max_concurrency` 3 never queued with 16 speakers whose dub lasts about
1.2 s per 5 s turn: `tts_slot_wait` p50/p95 0) are not on the critical path. Two target languages
do not slow the dub of either language (the languages run concurrently: last-sentence dub p50
1318 vs 1354 ms; the first-dub figure is lower only because it takes the earlier of two draws);
the number of TTS contexts and cold opens doubles, as expected. The audio never-blocking guarantees were not
touched: frames dropped and media events dropped are 0 in every run, and the audio path from the
transport event to `SpeakerHandle::push` is unchanged.

## Target: speech end to first dub audio p50 < 1.5 s and p95 < 2.5 s (spec 08 provider numbers)

| Run | p50 | p95 | p99 | Met |
|---|---|---|---|---|
| A baseline (single-sentence turns) | 2361 | 3080 | 3388 | no |
| A default (b+c) | 1354 | 1768 | 2035 | yes (second seed: 1330 / 1787 / 2002) |
| B default, first dub of the turn | 65 | 1101 | 1462 | yes (the first sentence is dubbed during the speech) |
| B default, last sentence of the turn (`e2e_last_dub_audio`) | 1606 | 2413 | 2789 | p95 yes; p50 misses 1.5 s by 106 ms; second seed 1639 / 2528 (p95 over 2.5 s by 28 ms) |

The target is met for the case it describes (a turn that ends and is dubbed after the speaker
stops). For multi-sentence turns the first sentence is dubbed while the speaker is still talking,
but the dub of the last sentence is at the edge of the budget: it pays the sentence-completion
lag, one Realtime translation and one TTS first audio in series after the speaker stopped.

## What is not done, and why

- **MT text deltas streamed into the TTS continuation** (flag, fake supports it): not implemented.
  It needs a streaming `Translator` port (deltas) in `warptalk-mt` and a push-text call on the
  `SynthContext` / TTS stage in `warptalk-tts`; this task only edits `apps/pipeline`,
  `crates/pipeline/runtime` and `crates/pipeline/ingress`. Proposed shape: `Translator::translate_stream`
  returning a channel of text pieces and `SynthContext::push_text(piece, is_last)`; with the 685 ms
  Realtime sentence and a 180 ms TTS first audio the ceiling is about 80-100 ms (first token ahead
  of done, the 80 ms lead modelled in `FakeMt`).
- **Pre-warm on the MT first token**: the MT crate exposes no first-token hook (the stamp is only
  read when the sentence is done), so the pre-warm triggers when the segment enters MT, which is
  strictly earlier (600-900 ms ahead of the TTS open) and costs one pre-opened context per target
  language per segment (local bookkeeping with Cartesia until the first push; unclaimed contexts
  are cancelled after `tts_prewarm_max_age_ms`).
- **Frame coalescing for STT append**: not measured as a win. The feed path costs a few microseconds
  per 32 ms frame and the harness shows no CPU pressure, so there is nothing to coalesce for.
- The doubles encode assumptions that real providers must confirm: the STT delivers sentence
  terminators before the commit (spec 08 section 9 question 1 is still open for single short
  sentences: if it does not, the dub falls back to the baseline path of about 2.4 s), the 20%
  cold-open share is a guess for "pool drained or socket expired", and the TTS delivers at 2x real
  time.

## Reproduce

```sh
cargo build -p warptalk-pipeline --release --bin pipeline-bench
B=./target/release/pipeline-bench; C="--rooms 4 --speakers 4 --seconds 90 --seed 42 --json"
A="--speech-ms 2000 --pause-ms 3000 --sentence-ms 4000"
$B $C $A --baseline                         # A baseline
$B $C $A --baseline --early-endpoint-ms 250 # + a
$B $C $A --baseline --early-endpoint-ms 250 --early-mt                # + a + b
$B $C $A --baseline --early-endpoint-ms 250 --early-mt --tts-prewarm  # + a + b + c
$B $C $A                                     # shipped default (b + c)
$B $C --baseline                             # scenario B baseline; drop $A for B
```

The default `cargo test -p warptalk-pipeline` also runs a short chain bench
(`tests/bench_chain.rs`) and the round-trip tests (`tests/chain.rs`).
