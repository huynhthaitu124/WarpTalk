import AVFoundation
import Foundation
import LiveKit

/// Publishes externally captured PCM (48 kHz mono Float32) into the bridge LiveKit room using the
/// publish-only token from `POST /meetings/{id}/bridge-token` (identity = the stand-in seat).
///
/// !!! The LiveKit Swift SDK has no public "custom audio source" track for PCM like the JS/Android SDKs.
/// !!! Strategy A (implemented): publish the microphone track but SILENCE the real mic in the SDK mixer and feed our
/// !!! captured buffers through the mixer's app-audio input (the API the SDK offers for ReplayKit/app audio).
/// !!! Strategy B (documented fallback, README §"Plan B"): loop captured audio into BlackHole 16ch with AVAudioEngine and select
/// !!! that device as the SDK input (`AudioManager.shared.inputDevice`).
/// Everything marked VERIFY must be checked against the pinned SDK version on first compile.
@MainActor
final class BridgePublisher {
    private let room: Room
    private(set) var isConnected = false

    init() {
        // autoSubscribe false: the bridge token cannot subscribe anyway.
        room = Room(delegate: nil,
                    connectOptions: ConnectOptions(autoSubscribe: false),
                    roomOptions: RoomOptions(adaptiveStream: false, dynacast: false))   // VERIFY: initializer labels
    }

    func connect(url: String, token: String) async throws {
        // VERIFY: AudioManager.shared.mixer exposes micVolume / appVolume (0...1) in 2.x.
        AudioManager.shared.mixer.micVolume = 0
        AudioManager.shared.mixer.appVolume = 1
        try await room.connect(url: url, token: token)
        // Enabling the mic starts the SDK audio engine + creates the published audio track (needs mic TCC permission).
        try await room.localParticipant.setMicrophone(enabled: true)
        isConnected = true
    }

    /// Thread-safe: called from the capture queue.
    nonisolated func push(_ buffer: AVAudioPCMBuffer) {
        // VERIFY: AudioManager.shared.mixer.capture(appAudio: AVAudioPCMBuffer) — method name/availability (SDK >= 2.0.x?).
        AudioManager.shared.mixer.capture(appAudio: buffer)
    }

    func disconnect() async {
        isConnected = false
        try? await room.localParticipant.setMicrophone(enabled: false)
        await room.disconnect()
        AudioManager.shared.mixer.micVolume = 1
    }
}
