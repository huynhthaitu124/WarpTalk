import AVFoundation
import XCTest
@testable import WarpTalk

final class ResamplerTests: XCTestCase {
    func testCapacityMath() {
        XCTAssertEqual(Resampler.outputFrameCapacity(inputFrames: 4800, inputRate: 48000, outputRate: 48000), 4800 + 32)
        XCTAssertEqual(Resampler.outputFrameCapacity(inputFrames: 441, inputRate: 44100, outputRate: 48000), 480 + 32)
        XCTAssertEqual(Resampler.outputFrameCapacity(inputFrames: 160, inputRate: 16000, outputRate: 48000), 480 + 32)
        XCTAssertEqual(Resampler.outputFrameCapacity(inputFrames: 10, inputRate: 0, outputRate: 48000), 0)
    }

    private func sine(rate: Double, channels: AVAudioChannelCount, frames: AVAudioFrameCount) -> AVAudioPCMBuffer {
        let f = AVAudioFormat(standardFormatWithSampleRate: rate, channels: channels)!
        let b = AVAudioPCMBuffer(pcmFormat: f, frameCapacity: frames)!
        b.frameLength = frames
        for ch in 0..<Int(channels) { for i in 0..<Int(frames) { b.floatChannelData![ch][i] = 0.5 * sinf(Float(i) * 0.05) } }
        return b
    }

    func testStereo44kTo48kMono() throws {
        let r = Resampler()
        var total = 0
        for _ in 0..<10 { total += Int(try XCTUnwrap(r.convert(sine(rate: 44100, channels: 2, frames: 4410))).frameLength) }
        // 10 chunks of 100 ms -> ~1 s at 48 kHz (converter latency allows a small shortfall)
        XCTAssertEqual(Double(total), 48000, accuracy: 600)
        XCTAssertEqual(r.convert(sine(rate: 44100, channels: 2, frames: 441))?.format.channelCount ?? 0, 1)
    }

    func test16kTo48k() throws {
        let out = try XCTUnwrap(Resampler().convert(sine(rate: 16000, channels: 1, frames: 1600)))
        XCTAssertEqual(Double(out.frameLength), 4800, accuracy: 400)
        XCTAssertEqual(out.format.sampleRate, 48000)
    }

    func test48kMonoPassthroughKeepsLength() throws {
        let out = try XCTUnwrap(Resampler().convert(sine(rate: 48000, channels: 1, frames: 960)))
        XCTAssertEqual(out.frameLength, 960)
    }

    func testRms() {
        XCTAssertEqual(Resampler.rmsDbfs(sine(rate: 48000, channels: 1, frames: 4800)), -9, accuracy: 1)
    }
}
