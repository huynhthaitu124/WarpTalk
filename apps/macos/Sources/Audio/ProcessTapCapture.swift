import AVFoundation
import CoreAudio

/// Captures the audio of a meeting app with a Core Audio process tap (macOS 14.2+):
///   CATapDescription -> AudioHardwareCreateProcessTap -> private aggregate device (tap in its tap list)
///   -> IOProc -> AVAudioPCMBuffer -> Resampler (48 kHz mono f32) -> onBuffer.
///
/// - Taps only the target app's audio processes (Chrome renders audio in helper processes, so we tap all
///   matching process objects). WarpTalk's own process is never included, so dubbed audio cannot loop back.
/// - `muteBehavior = .unmuted`: the user still hears the meeting normally.
/// - The tap's process list is fixed at creation, so a watchdog rebuilds the tap when new helper processes
///   appear (e.g. Chrome spawns an audio process only once a tab starts playing).
/// - Needs TCC "Audio Recording"/system audio permission (NSAudioCaptureUsageDescription); the prompt shows on first start.
final class ProcessTapCapture: AudioCaptureSource, @unchecked Sendable {
    let displayName = "Core Audio process tap"
    var onBuffer: (@Sendable (AVAudioPCMBuffer) -> Void)?
    var onError: (@Sendable (Error) -> Void)?

    private let queue = DispatchQueue(label: "warptalk.processtap", qos: .userInteractive)
    private let lock = NSLock()
    private var target: CaptureTarget?
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var procID: AudioDeviceIOProcID?
    private var resampler = Resampler()
    private var tapFormat: AVAudioFormat?
    private var activeObjects: Set<AudioObjectID> = []
    private var watchdog: Task<Void, Never>?

    func start(target: CaptureTarget) async throws {
        guard #available(macOS 14.2, *) else { throw CaptureError.unsupportedOS }
        self.target = target
        let objects = DeviceManager.processObjects(for: target)
        guard !objects.isEmpty else { throw CaptureError.noAudioProcess(target.name) }
        try build(objects: objects)
        watchdog = Task.detached { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 4_000_000_000)
                self?.refreshIfNeeded()
            }
        }
    }

    func stop() {
        watchdog?.cancel(); watchdog = nil
        lock.lock(); defer { lock.unlock() }
        teardownLocked()
        target = nil
    }

    // MARK: Build / teardown

    @available(macOS 14.2, *)
    private func build(objects: [AudioObjectID]) throws {
        lock.lock(); defer { lock.unlock() }
        teardownLocked()

        // VERIFY: initializer parameter type. Header is NSArray<NSNumber*>; if the compiler wants [AudioObjectID], drop the map.
        let desc = CATapDescription(stereoMixdownOfProcesses: objects.map { NSNumber(value: $0) })
        desc.uuid = UUID()
        desc.name = "WarpTalk tap"
        desc.isPrivate = true
        desc.muteBehavior = .unmuted

        var tap = AudioObjectID(kAudioObjectUnknown)
        var st = AudioHardwareCreateProcessTap(desc, &tap)
        guard st == noErr else {
            // -> 'what' ≈ permission not granted (system shows prompt on first call) or no such process.
            throw CaptureError.coreAudio("AudioHardwareCreateProcessTap", st)
        }
        tapID = tap

        // Tap stream format
        var fmtAddr = AudioObjectPropertyAddress(mSelector: kAudioTapPropertyFormat, mScope: kAudioObjectPropertyScopeGlobal,
                                                 mElement: kAudioObjectPropertyElementMain)
        var asbd = AudioStreamBasicDescription()
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        st = AudioObjectGetPropertyData(tap, &fmtAddr, 0, nil, &size, &asbd)
        guard st == noErr, let fmt = AVAudioFormat(streamDescription: &asbd) else {
            teardownLocked(); throw CaptureError.coreAudio("kAudioTapPropertyFormat", st)
        }
        tapFormat = fmt
        resampler = Resampler()

        // Private aggregate device holding the tap. Clock = default output device.
        guard let out = DeviceManager.defaultOutputDevice() else { teardownLocked(); throw CaptureError.formatUnavailable }
        let dict: [String: Any] = [
            kAudioAggregateDeviceNameKey: "WarpTalk Tap",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceMainSubDeviceKey: out.uid,   // VERIFY: key name on 14.2 SDK (older: ...MasterSubDeviceKey)
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapAutoStartKey: true,
            kAudioAggregateDeviceSubDeviceListKey: [[kAudioSubDeviceUIDKey: out.uid]],
            kAudioAggregateDeviceTapListKey: [[
                kAudioSubTapDriftCompensationKey: true,
                kAudioSubTapUIDKey: desc.uuid.uuidString,
            ]],
        ]
        var agg = AudioObjectID(kAudioObjectUnknown)
        st = AudioHardwareCreateAggregateDevice(dict as CFDictionary, &agg)
        guard st == noErr else { teardownLocked(); throw CaptureError.coreAudio("AudioHardwareCreateAggregateDevice", st) }
        aggregateID = agg

        var pid: AudioDeviceIOProcID?
        st = AudioDeviceCreateIOProcIDWithBlock(&pid, agg, queue) { [weak self] _, inInputData, _, _, _ in
            self?.handle(inInputData)
        }
        guard st == noErr, let pid else { teardownLocked(); throw CaptureError.coreAudio("AudioDeviceCreateIOProcIDWithBlock", st) }
        procID = pid

        st = AudioDeviceStart(agg, pid)
        guard st == noErr else { teardownLocked(); throw CaptureError.coreAudio("AudioDeviceStart", st) }
        activeObjects = Set(objects)
    }

    private func teardownLocked() {
        if aggregateID != kAudioObjectUnknown {
            if let procID { AudioDeviceStop(aggregateID, procID); AudioDeviceDestroyIOProcID(aggregateID, procID) }
            AudioHardwareDestroyAggregateDevice(aggregateID)
        }
        procID = nil
        aggregateID = AudioObjectID(kAudioObjectUnknown)
        if tapID != kAudioObjectUnknown {
            if #available(macOS 14.2, *) { AudioHardwareDestroyProcessTap(tapID) }
        }
        tapID = AudioObjectID(kAudioObjectUnknown)
        activeObjects = []
    }

    private func refreshIfNeeded() {
        guard let target else { return }
        let now = Set(DeviceManager.processObjects(for: target))
        lock.lock(); let same = (now == activeObjects); lock.unlock()
        guard !same, !now.isEmpty else { return }
        if #available(macOS 14.2, *) {
            do { try build(objects: Array(now)) } catch { onError?(error) }
        }
    }

    // MARK: IO

    private func handle(_ list: UnsafePointer<AudioBufferList>) {
        guard let fmt = tapFormat,
              let buf = AVAudioPCMBuffer(pcmFormat: fmt, bufferListNoCopy: list, deallocator: nil),
              buf.frameLength > 0,
              let out = resampler.convert(buf) else { return }   // convert copies, so the no-copy buffer is not retained
        onBuffer?(out)
    }
}
