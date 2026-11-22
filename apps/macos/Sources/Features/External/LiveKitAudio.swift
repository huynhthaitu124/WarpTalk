import Foundation
import LiveKit

/// Process-wide LiveKit audio setup shared by `BridgePublisher` and `DubListener`.
///
/// Both use the SDK's MANUAL RENDERING mode (`AudioManager.setManualRenderingMode(true)`):
/// the SDK's AVAudioEngine then touches no audio device, which means
///   - publishing needs no microphone and no TCC Microphone prompt: captured PCM goes in with
///     `AudioManager.shared.mixer.capture(appAudio:)` and leaves through the normal microphone track;
///   - remote audio is NOT played by the SDK, so the dub track can only be heard through our own `VoiceOutputPlayer`
///     (no double playback, no feedback into the meeting).
/// Source: client-sdk-swift 2.19.0, Sources/LiveKit/Audio/Manager/AudioManager.swift:406-416 (doc comment + `setManualRenderingMode`),
/// Docs/audio.md "Capturing Audio Buffers Without Device Access (Manual Mode)",
/// Tests/LiveKitAudioTests/AudioEngineTests.swift `manualRenderingModePublishAudio`.
/// The mode is global to the process (`AudioManager.shared`); the web view's own WebRTC stack is unaffected.
enum LiveKitAudio {
    @MainActor
    static func ensureManualRendering() throws {
        if AudioManager.shared.isManualRenderingMode { return }
        do { try AudioManager.shared.setManualRenderingMode(true) }
        catch { throw BridgeError(code: "CAPTURE_FAILED", message: "Không bật được chế độ âm thanh thủ công của LiveKit: \(error.localizedDescription)") }
    }
}
