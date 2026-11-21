import AVFoundation
import AppKit

/// Canonical format published to LiveKit for the far side: 48 kHz, mono, Float32, non-interleaved.
enum BridgeAudioFormat {
    static let sampleRate: Double = 48_000
    static let channels: AVAudioChannelCount = 1
    // Standard (deinterleaved Float32) format; init only fails for invalid args, which these are not.
    static let format: AVAudioFormat = AVAudioFormat(standardFormatWithSampleRate: sampleRate, channels: channels)!
}

/// A meeting app we can capture (picked via NSWorkspace).
struct CaptureTarget: Identifiable, Hashable {
    let bundleID: String
    let pid: pid_t
    let name: String
    /// Extra bundle-id prefixes whose audio processes belong to this app (e.g. Safari's WebKit GPU process).
    let extraAudioPrefixes: [String]
    var id: String { "\(bundleID):\(pid)" }
}

/// Catalog of supported meeting apps.
struct MeetingAppSpec {
    let name: String
    let bundleIDs: [String]
    let extraAudioPrefixes: [String]

    static let all: [MeetingAppSpec] = [
        .init(name: "Google Chrome", bundleIDs: ["com.google.Chrome"], extraAudioPrefixes: []),
        .init(name: "Zoom", bundleIDs: ["us.zoom.xos"], extraAudioPrefixes: []),
        // New Teams = com.microsoft.teams2, classic = com.microsoft.teams
        .init(name: "Microsoft Teams", bundleIDs: ["com.microsoft.teams2", "com.microsoft.teams"], extraAudioPrefixes: []),
        // Safari renders web audio in a shared WebKit GPU process, not a child of Safari.
        .init(name: "Safari", bundleIDs: ["com.apple.Safari"], extraAudioPrefixes: ["com.apple.WebKit"]),
    ]
}

enum CaptureError: LocalizedError {
    case unsupportedOS
    case noAudioProcess(String)
    case coreAudio(String, OSStatus)
    case permissionDenied(String)
    case noDisplay
    case formatUnavailable

    var errorDescription: String? {
        switch self {
        case .unsupportedOS: return "Cần macOS 14.2 trở lên cho chế độ Process Tap."
        case .noAudioProcess(let n): return "\(n) chưa phát âm thanh (chưa có tiến trình audio). Hãy vào cuộc họp / bật tiếng rồi thử lại."
        case .coreAudio(let what, let st): return "Core Audio lỗi ở \(what) (OSStatus \(st))."
        case .permissionDenied(let w): return "Chưa được cấp quyền: \(w). Mở tab Quyền để cấp."
        case .noDisplay: return "Không tìm thấy màn hình cho ScreenCaptureKit."
        case .formatUnavailable: return "Không đọc được định dạng âm thanh."
        }
    }
}

/// Anything that can deliver the far side's audio as 48 kHz mono Float32 buffers.
/// Callbacks arrive on an arbitrary (non-main) queue.
protocol AudioCaptureSource: AnyObject {
    var displayName: String { get }
    var onBuffer: (@Sendable (AVAudioPCMBuffer) -> Void)? { get set }
    var onError: (@Sendable (Error) -> Void)? { get set }
    func start(target: CaptureTarget) async throws
    func stop()
}

enum RunningMeetingApps {
    /// Running instances of the supported meeting apps, via NSWorkspace.
    static func current() -> [CaptureTarget] {
        var out: [CaptureTarget] = []
        for spec in MeetingAppSpec.all {
            for bid in spec.bundleIDs {
                for app in NSWorkspace.shared.runningApplications where app.bundleIdentifier == bid {
                    out.append(CaptureTarget(bundleID: bid, pid: app.processIdentifier,
                                             name: app.localizedName ?? spec.name,
                                             extraAudioPrefixes: spec.extraAudioPrefixes))
                }
            }
        }
        return out
    }
}
