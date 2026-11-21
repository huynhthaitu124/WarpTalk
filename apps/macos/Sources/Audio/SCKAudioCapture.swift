import AVFoundation
import CoreMedia
import ScreenCaptureKit

/// Fallback capture: ScreenCaptureKit audio of the target application (needs Screen Recording permission).
/// SCStream always wants a video config; we shrink it to 2x2 @ 1 fps and ignore video frames.
final class SCKAudioCapture: NSObject, AudioCaptureSource, SCStreamOutput, SCStreamDelegate, @unchecked Sendable {
    let displayName = "ScreenCaptureKit (audio)"
    var onBuffer: (@Sendable (AVAudioPCMBuffer) -> Void)?
    var onError: (@Sendable (Error) -> Void)?

    private var stream: SCStream?
    private let queue = DispatchQueue(label: "warptalk.sck.audio", qos: .userInteractive)
    private let resampler = Resampler()

    func start(target: CaptureTarget) async throws {
        guard CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess() else {
            throw CaptureError.permissionDenied("Ghi màn hình (Screen Recording)")
        }
        let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: false)
        guard let display = content.displays.first else { throw CaptureError.noDisplay }
        let apps = content.applications.filter {
            $0.bundleIdentifier == target.bundleID || $0.processID == target.pid
                || target.extraAudioPrefixes.contains(where: { p in $0.bundleIdentifier.hasPrefix(p) })
        }
        guard !apps.isEmpty else { throw CaptureError.noAudioProcess(target.name) }

        let filter = SCContentFilter(display: display, including: apps, exceptingWindows: [])
        let cfg = SCStreamConfiguration()
        cfg.capturesAudio = true
        cfg.excludesCurrentProcessAudio = true
        cfg.sampleRate = 48_000
        cfg.channelCount = 1
        cfg.width = 2; cfg.height = 2
        cfg.minimumFrameInterval = CMTime(value: 1, timescale: 1)
        cfg.queueDepth = 3

        let s = SCStream(filter: filter, configuration: cfg, delegate: self)
        try s.addStreamOutput(self, type: .audio, sampleHandlerQueue: queue)
        try await s.startCapture()
        stream = s
    }

    func stop() {
        let s = stream; stream = nil
        Task { try? await s?.stopCapture() }
    }

    // MARK: SCStreamOutput
    func stream(_ stream: SCStream, didOutputSampleBuffer sb: CMSampleBuffer, of type: SCStreamOutputType) {
        guard type == .audio, sb.isValid, CMSampleBufferGetNumSamples(sb) > 0,
              let fd = CMSampleBufferGetFormatDescription(sb),
              let asbdPtr = CMAudioFormatDescriptionGetStreamBasicDescription(fd) else { return }
        var asbd = asbdPtr.pointee
        guard let fmt = AVAudioFormat(streamDescription: &asbd) else { return }
        let frames = AVAudioFrameCount(CMSampleBufferGetNumSamples(sb))
        guard let pcm = AVAudioPCMBuffer(pcmFormat: fmt, frameCapacity: frames) else { return }
        pcm.frameLength = frames
        let st = CMSampleBufferCopyPCMDataIntoAudioBufferList(sb, at: 0, frameCount: Int32(frames), into: pcm.mutableAudioBufferList)
        guard st == noErr, let out = resampler.convert(pcm) else { return }
        onBuffer?(out)
    }

    // MARK: SCStreamDelegate
    func stream(_ stream: SCStream, didStopWithError error: Error) { onError?(error) }
}
