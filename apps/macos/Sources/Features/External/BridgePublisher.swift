import AVFoundation
import Foundation
import LiveKit

/// Publishes externally captured PCM (48 kHz mono Float32) into the bridge LiveKit room using the
/// publish-only token from `POST /meetings/{id}/bridge-token` (identity = the stand-in seat).
///
/// How external PCM gets into LiveKit Swift (verified against client-sdk-swift 2.19.0): there is no custom audio source
/// object like `LocalAudioTrack(source:)`. The SDK's documented path is
///   1. `AudioManager.shared.setManualRenderingMode(true)`         (AudioManager.swift:411; no device access, see `LiveKitAudio`)
///   2. `room.localParticipant.setMicrophone(enabled: true)`        (publishes the microphone track; fed by the engine, not by a mic)
///   3. `AudioManager.shared.mixer.capture(appAudio: buffer)` repeatedly   (MixerEngineObserver.swift:337; converts any
///      `AVAudioPCMBuffer` format to the engine format itself and schedules it on the app-audio player node)
/// Docs/audio.md "Capturing Audio Buffers Without Device Access (Manual Mode)" describes exactly this sequence.
/// `capture(appAudio:)` drops buffers (log "Engine is not running") until step 2 has started the engine: harmless.
@MainActor
final class BridgePublisher {
    private let room: Room
    private(set) var isConnected = false

    init() {
        // autoSubscribe false: the bridge token cannot subscribe anyway. ConnectOptions.swift:99 / RoomOptions.swift:72.
        room = Room(delegate: nil, connectOptions: ConnectOptions(autoSubscribe: false), roomOptions: RoomOptions())
    }

    func connect(url: String, token: String) async throws {
        try LiveKitAudio.ensureManualRendering()
        AudioManager.shared.mixer.appVolume = 1          // MixerEngineObserver.swift:~33 (Float 0...1)
        try await room.connect(url: url, token: token)   // Room.swift:385
        _ = try await room.localParticipant.setMicrophone(enabled: true)
        isConnected = true
    }

    /// Thread-safe: called from the capture queue (`MixerEngineObserver` state is lock-protected).
    nonisolated func push(_ buffer: AVAudioPCMBuffer) {
        AudioManager.shared.mixer.capture(appAudio: buffer)
    }

    func disconnect() async {
        isConnected = false
        _ = try? await room.localParticipant.setMicrophone(enabled: false)
        await room.disconnect()                          // Room.swift:530
    }
}
