# LiveKit transport (feature `livekit-rtc`)

`src/livekit_transport.rs` is the LiveKit implementation of `MediaTransport`. It is behind the
cargo feature `livekit-rtc` (default off) and has **not been compiled**: the `livekit` crate pulls
in `webrtc-sys`, whose build needs **clang >= 21** (the dev container has an older one). It was
written against the sources of `livekit 0.12.0` and `libwebrtc 0.4.2`; the few calls that could not
be checked are marked `// UNVERIFIED` in the file.

Without the feature, `MEDIA_TRANSPORT=livekit` exits with an error pointing here;
`MEDIA_TRANSPORT=simulated` works everywhere.

## Enabling it

Two manifests need a change (neither is done yet; the root one is owned by the workspace owner).

Already applied in the repo (root `Cargo.toml`, `[workspace.dependencies]`):

```toml
livekit = { version = "0.12", default-features = false, features = ["tokio", "rustls-tls-webpki-roots"] }
```

(`default-features = false` drops `native-tls`; the `tokio` feature selects the tokio runtime.
If your toolchain prefers the platform TLS, use `features = ["tokio", "native-tls"]` instead.)

`apps/pipeline/Cargo.toml` (already applied):

```toml
[features]
livekit-rtc = ["dep:livekit"]        # replaces the current `livekit-rtc = []`

[dependencies]
livekit = { workspace = true, optional = true }
```

Then build and run:

```sh
cargo build -p warptalk-pipeline --features livekit-rtc --release
MEDIA_TRANSPORT=livekit LIVEKIT_URL=wss://... LIVEKIT_API_KEY=... LIVEKIT_API_SECRET=... \
OPENAI_API_KEY=... REDIS_URL=redis://localhost:6379 \
  target/release/warptalk-pipeline
```

## Building

`webrtc-sys` downloads a prebuilt libwebrtc (needs network at build time, through the agent proxy
in this sandbox) and compiles a C++ shim with clang.

- **macOS**: `brew install llvm` (Homebrew's current llvm is >= 21), then
  `export CC=$(brew --prefix llvm)/bin/clang CXX=$(brew --prefix llvm)/bin/clang++` before
  `cargo build`. Xcode's Apple clang is not a substitute for the version check.
- **Linux**: install clang 21 (for example from apt.llvm.org: `llvm.sh 21`), then
  `export CC=clang-21 CXX=clang++-21`. Also needs `pkg-config`, `libssl-dev`, `libx11-dev`,
  `libgl1-mesa-dev`, `libxext-dev` (webrtc-sys links X11/GL even for audio-only use).
- Check with `clang --version` before building; the failure mode of an old clang is a C++
  compile error in `webrtc-sys`, not a Rust error.

## Behaviour and assumptions

- Joins as a hidden agent (`warptalk_livekit::pipeline_agent_token`, `hidden`, publish + subscribe +
  data). LiveKit room name = translation room id; participant identity = participant id (the
  audio routes reference participants by id). Both are assumptions about the meeting service:
  adjust `LiveKitTransport::join` if the room name differs.
- Subscribes to every remote microphone track and reads it with `NativeAudioStream` at 16 kHz
  mono. Tracks of identities starting with `pipeline-agent` or `ai-interpreter-` are skipped.
- Dub audio: one `NativeAudioSource` (48 kHz mono, 1000 ms queue) and `LocalAudioTrack` per
  target language, track name `ai-interpreter-{lang}`; `publish_dub` resamples to 48 kHz and
  captures 10 ms frames. A per-voice or per-speaker track naming (spec 08 hop 6) would key
  `dub_tracks` by `(lang, voice)` instead.
- Captions: reliable data packet, topic `captions`, JSON payload (see `downstream.rs`).

## Verification checklist once it builds

1. `cargo clippy -p warptalk-pipeline --features livekit-rtc --all-targets -- -D warnings`
2. Join a LiveKit dev server room with two browser participants; check that
   `XLEN stt:results` grows and captions arrive on the data channel (topic `captions`).
3. Confirm `NativeAudioStream::new(.., None)` (queue size) and the `Room::remote_participants`
   / `TrackSubscribed` ordering for participants that were already in the room.
4. Run `pipeline-bench` against the same STT numbers to compare simulated and real transport CPU.
