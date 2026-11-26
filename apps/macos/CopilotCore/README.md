# CopilotCore (Rust engine, UniFFI)

This directory is a local Swift package (`Package.swift`, module `CopilotCore`, binary target `CopilotCoreFFI`). `CopilotCore.xcframework` and `Sources/CopilotCore/CopilotCore.swift` are build outputs (gitignored). Generate them first:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin   # once
cd apps/macos && make core                                    # then: make gen / make build
```

`make core` runs `crates/pipeline/copilot-core/scripts/build-xcframework.sh` (release, universal arm64 + x86_64,
deployment target 14.2). `project.yml` depends on the package (`import CopilotCore`, so `#if canImport(CopilotCore)` is true), so
`make gen`/`make build` fail until `make core` has run. The Rust-side modulemap module is `warptalk_copilot_coreFFI`;
the generated Swift imports it itself; app code does `import CopilotCore`.

Swift API (reference snapshot: `crates/pipeline/copilot-core/bindings/swift/warptalk_copilot_core.swift`):
`CopilotHandle(configJson:)`, `start(params:)`, `pushAudio(channel:samples:sampleRate:)` (O(1), audio-thread safe),
`updateEndpointer`, `updateBearer`, `stats()`, `stop()`, `events()` (`next(timeoutMs:)` / `drain(max:)`), and
`setListener(listener:)` with `CopilotEventListener.onEvent(event:)`, which is called on a Rust thread: hop to the
main actor before touching UI. Do not mix `setListener` with polling `events()` (one shared queue).
