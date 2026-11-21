import AVFoundation
import CoreAudio
import AudioToolbox

/// D7 voice mode: plays the `dub-{lang}` PCM into a chosen output device (e.g. "BlackHole 2ch") with AVAudioEngine,
/// so Meet/Zoom/Teams (whose microphone is set to that device) transmit the dub.
/// Buffers arrive from the LiveKit audio thread; we copy and schedule them, dropping if the backlog exceeds ~1 s.
final class VoiceOutputPlayer: @unchecked Sendable {
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private var format: AVAudioFormat?
    private let lock = NSLock()
    private var pending = 0
    private var deviceID: AudioDeviceID?
    private(set) var droppedBuffers = 0

    init(outputDeviceUID: String?) {
        if let uid = outputDeviceUID, !uid.isEmpty { deviceID = DeviceManager.device(uid: uid)?.objectID }
    }

    var isRunning: Bool { engine.isRunning }

    /// Entry point for `LiveKitSession.dubPCMSink`.
    func enqueue(_ buffer: AVAudioPCMBuffer) {
        lock.lock(); defer { lock.unlock() }
        if format != buffer.format { rebuildLocked(for: buffer.format) }
        guard engine.isRunning, let copy = Resampler.copy(buffer) else { return }
        if pending > 60 { droppedBuffers += 1; return }
        pending += 1
        player.scheduleBuffer(copy) { [weak self] in
            self?.lock.lock(); self?.pending -= 1; self?.lock.unlock()
        }
        if !player.isPlaying { player.play() }
    }

    func stop() {
        lock.lock(); defer { lock.unlock() }
        player.stop()
        if engine.isRunning { engine.stop() }
        format = nil; pending = 0
    }

    private func rebuildLocked(for fmt: AVAudioFormat) {
        player.stop()
        if engine.isRunning { engine.stop() }
        engine.reset()
        if player.engine == nil { engine.attach(player) }

        // Route this engine to the chosen device (must be done before start).
        if let id = deviceID, let unit = engine.outputNode.audioUnit {
            var dev = id
            let st = AudioUnitSetProperty(unit, kAudioOutputUnitProperty_CurrentDevice, kAudioUnitScope_Global, 0,
                                          &dev, UInt32(MemoryLayout<AudioDeviceID>.size))
            if st != noErr { NSLog("[voice] set output device failed: \(st)") }   // VERIFY: works on AVAudioEngine.outputNode.audioUnit
        }
        engine.connect(player, to: engine.mainMixerNode, format: fmt)
        do { try engine.start(); format = fmt } catch { NSLog("[voice] engine start failed: \(error)") }
    }
}
