import AudioToolbox
import AVFoundation
import CoreAudio
import Foundation

/// Raw float samples (mono) from a capture, with their sample rate.
enum AudioSamples {
    /// Copies channel 0 of a Float32 buffer (our captures are already 48 kHz mono, see `Resampler`).
    static func floats(_ b: AVAudioPCMBuffer) -> [Float] {
        guard let ch = b.floatChannelData?[0], b.frameLength > 0 else { return [] }
        return Array(UnsafeBufferPointer(start: ch, count: Int(b.frameLength)))
    }
}

/// Microphone source for the `.me` channel. Callbacks arrive on the audio thread.
protocol MicCapturing: AnyObject {
    var onSamples: (@Sendable (_ samples: [Float], _ sampleRate: UInt32) -> Void)? { get set }
    var onError: (@Sendable (Error) -> Void)? { get set }
    func start(deviceUid: String?) throws
    func stop()
}

/// AVAudioEngine input tap -> `Resampler` (48 kHz, mono, Float32) -> `onSamples`. No main-thread hop.
/// Needs the microphone TCC permission (checked by the controller before `start`).
final class MicCapture: MicCapturing, @unchecked Sendable {
    var onSamples: (@Sendable ([Float], UInt32) -> Void)?
    var onError: (@Sendable (Error) -> Void)?

    private let engine = AVAudioEngine()
    private let resampler = Resampler()
    private var observer: NSObjectProtocol?
    private var running = false

    func start(deviceUid: String?) throws {
        guard !running else { return }
        let input = engine.inputNode
        // Route the engine to a specific input device before reading its format. Same AUHAL mechanism as VoiceOutputPlayer
        // (kAudioOutputUnitProperty_CurrentDevice on `AVAudioIONode.audioUnit`).
        // VERIFY: on a real Mac that setting the device on inputNode.audioUnit takes effect for the tap (and that an
        // aggregate/Bluetooth device reports a usable `outputFormat(forBus: 0)`).
        if let uid = deviceUid, !uid.isEmpty {
            guard let dev = DeviceManager.device(uid: uid), dev.hasInput else {
                throw BridgeError(code: "CAPTURE_FAILED", message: "Không tìm thấy micro \(uid).")
            }
            if let unit = input.audioUnit {
                var id = dev.objectID
                let st = AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                                              &id, UInt32(MemoryLayout<AudioDeviceID>.size))
                if st != noErr { throw CaptureError.coreAudio("chọn micro", st) }
            }
        }
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else { throw CaptureError.formatUnavailable }

        let resampler = self.resampler
        input.installTap(onBus: 0, bufferSize: 2048, format: format) { [weak self] buffer, _ in
            // Audio thread. Resampler is only used from here; convert() copies, so the tap buffer is not retained.
            guard let cb = self?.onSamples, let out = resampler.convert(buffer) else { return }
            let samples = AudioSamples.floats(out)
            if !samples.isEmpty { cb(samples, UInt32(out.format.sampleRate)) }
        }
        observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange, object: engine, queue: nil) { [weak self] _ in
            // Device unplugged / sample rate changed: the engine stops itself. Report; the web app can restart the session.
            self?.onError?(CaptureError.coreAudio("cấu hình micro thay đổi", 0))
        }
        engine.prepare()
        do { try engine.start() } catch {
            input.removeTap(onBus: 0)
            throw error
        }
        running = true
    }

    func stop() {
        guard running else { return }
        running = false
        if let o = observer { NotificationCenter.default.removeObserver(o); observer = nil }
        engine.inputNode.removeTap(onBus: 0)
        engine.stop()
    }
}
