import AVFoundation

/// Converts any PCM buffer to 48 kHz mono Float32 (AVAudioConverter). Stateful: keep one instance per stream
/// so the converter's filter state carries across chunks.
final class Resampler {
    let outputFormat: AVAudioFormat
    private var converter: AVAudioConverter?
    private var converterInput: AVAudioFormat?

    init(outputFormat: AVAudioFormat = BridgeAudioFormat.format) { self.outputFormat = outputFormat }

    /// Upper bound of output frames for `inputFrames` at `inputRate` -> `outputRate` (ceil + small slack
    /// for the converter's internal buffering). Pure; unit-tested.
    static func outputFrameCapacity(inputFrames: AVAudioFrameCount, inputRate: Double, outputRate: Double) -> AVAudioFrameCount {
        guard inputRate > 0 else { return 0 }
        let exact = Double(inputFrames) * outputRate / inputRate
        return AVAudioFrameCount(exact.rounded(.up)) + 32
    }

    func convert(_ input: AVAudioPCMBuffer) -> AVAudioPCMBuffer? {
        guard input.frameLength > 0 else { return nil }
        if input.format == outputFormat { return Self.copy(input) }

        if converter == nil || converterInput != input.format {
            converter = AVAudioConverter(from: input.format, to: outputFormat)
            converterInput = input.format
            // VERIFY: default stereo->mono mixing of AVAudioConverter is an average (no clipping); otherwise set channelMap/mixer.
        }
        guard let converter else { return nil }

        let cap = Self.outputFrameCapacity(inputFrames: input.frameLength, inputRate: input.format.sampleRate,
                                           outputRate: outputFormat.sampleRate)
        guard let out = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: cap) else { return nil }

        var fed = false
        var err: NSError?
        let status = converter.convert(to: out, error: &err) { _, outStatus in
            if fed { outStatus.pointee = .noDataNow; return nil }
            fed = true
            outStatus.pointee = .haveData
            return input
        }
        if status == .error || err != nil { return nil }
        return out.frameLength > 0 ? out : nil
    }

    static func copy(_ b: AVAudioPCMBuffer) -> AVAudioPCMBuffer? {
        guard let c = AVAudioPCMBuffer(pcmFormat: b.format, frameCapacity: b.frameLength) else { return nil }
        c.frameLength = b.frameLength
        let n = Int(b.frameLength)
        if let s = b.floatChannelData, let d = c.floatChannelData {
            for ch in 0..<Int(b.format.channelCount) { d[ch].update(from: s[ch], count: n) }
        }
        return c
    }

    /// RMS level in dBFS of channel 0 (-120 for silence).
    static func rmsDbfs(_ b: AVAudioPCMBuffer) -> Double {
        guard let ch = b.floatChannelData?[0], b.frameLength > 0 else { return -120 }
        var sum: Float = 0
        for i in 0..<Int(b.frameLength) { sum += ch[i] * ch[i] }
        let rms = (sum / Float(b.frameLength)).squareRoot()
        return rms > 0 ? max(-120, Double(20 * log10(rms))) : -120
    }
}
