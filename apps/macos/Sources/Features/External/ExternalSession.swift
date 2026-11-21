import AVFoundation
import Foundation
import SwiftUI

enum ExternalAudioMode: String, CaseIterable, Identifiable {
    case text, voice
    var id: String { rawValue }
    var label: String { self == .text ? "Chỉ phụ đề (text-only)" : "Giọng dịch (voice) → thiết bị ảo" }
}

enum CaptureBackend: String, CaseIterable, Identifiable {
    case processTap, screenCaptureKit
    var id: String { rawValue }
    var label: String { self == .processTap ? "Core Audio Process Tap (14.2+)" : "ScreenCaptureKit (dự phòng)" }
}

/// Thread-safe audio activity meter (written from the capture queue, read from the main actor).
final class AudioActivity: @unchecked Sendable {
    private let lock = NSLock()
    private var lastBufferAt: Date?
    private var lastSignalAt: Date?
    private var lastRms: Double = -120
    var signalThresholdDb: Double = -60

    func record(rmsDbfs: Double, now: Date = Date()) {
        lock.lock(); defer { lock.unlock() }
        lastBufferAt = now
        lastRms = rmsDbfs
        if rmsDbfs > signalThresholdDb { lastSignalAt = now }
    }

    struct Snapshot { var rmsDbfs: Double; var lastAudioAt: Date?; var flowing: Bool }

    /// `flowing` = the capture pipeline is delivering buffers (liveness), NOT "somebody is talking":
    /// a quiet meeting must not look like a dead capturer. `lastAudioAt` = last time the level exceeded the threshold.
    func snapshot(now: Date = Date(), liveWindow: TimeInterval = 3) -> Snapshot {
        lock.lock(); defer { lock.unlock() }
        let flowing = lastBufferAt.map { now.timeIntervalSince($0) <= liveWindow } ?? false
        return Snapshot(rmsDbfs: flowing ? lastRms : -120, lastAudioAt: lastSignalAt, flowing: flowing)
    }
}

/// Orchestrates External Meeting mode end to end.
@MainActor
final class ExternalSession: ObservableObject {
    enum Phase: Equatable { case idle, starting, running, needsTakeover, failed(String) }

    // Inputs
    @Published var targets: [CaptureTarget] = []
    @Published var selectedTargetID: String?
    @Published var linkText = ""
    @Published var provider: BridgeProviderKind = .googleMeet
    @Published var farLanguage: Language = .en        // language spoken on the other side
    @Published var audioMode: ExternalAudioMode = .text
    @Published var backend: CaptureBackend = .processTap
    @Published var autoFallbackToSCK = true

    // Outputs
    @Published private(set) var phase: Phase = .idle
    @Published private(set) var statusLines: [String] = []
    @Published private(set) var levelDb: Double = -120
    @Published private(set) var room: TranslationRoom?
    @Published private(set) var lastHeartbeat: Date?
    @Published private(set) var captureName = "-"

    let live: LiveKitSession
    private let app: AppState
    private let publisher = BridgePublisher()
    private var source: AudioCaptureSource?
    private var voice: VoiceOutputPlayer?
    private let activity = AudioActivity()
    private var heartbeatTask: Task<Void, Never>?
    private var meterTask: Task<Void, Never>?
    private var claim: BridgeClaim?

    init(app: AppState) {
        self.app = app
        self.live = LiveKitSession(listenLanguage: app.settings.listenLanguage.rawValue, hearOriginals: false)
        refreshTargets()
    }

    var selectedTarget: CaptureTarget? { targets.first { $0.id == selectedTargetID } }
    var isActive: Bool { phase == .running || phase == .starting }

    func refreshTargets() {
        targets = RunningMeetingApps.current()
        if selectedTarget == nil { selectedTargetID = targets.first?.id }
    }

    func linkChanged() {
        if let p = MeetingLink.parse(linkText)?.provider { provider = p }
    }

    private func log(_ s: String) {
        statusLines.append("\(Self.timeFmt.string(from: Date()))  \(s)")
        if statusLines.count > 80 { statusLines.removeFirst() }
    }
    private static let timeFmt: DateFormatter = { let f = DateFormatter(); f.dateFormat = "HH:mm:ss"; return f }()

    // MARK: Start / stop

    func start(takeOver: Bool = false) async {
        guard let ws = app.selectedWorkspace else { phase = .failed("Chưa chọn workspace."); return }
        guard let target = selectedTarget else { phase = .failed("Hãy chọn ứng dụng họp đang chạy."); return }
        guard let link = MeetingLink.parse(linkText) else { phase = .failed("Hãy dán link hoặc mã cuộc họp."); return }
        phase = .starting
        statusLines = []
        let s = app.settings

        do {
            // 1. Claim the bridge room
            log("Claim bridge room (\(provider.label), \(link.code))…")
            let claim = try await app.api.claimBridge(BridgeClaimRequest(
                workspaceId: ws.id, provider: provider.rawValue, meetCode: link.code,
                sourceLanguage: farLanguage.rawValue, targetLanguages: [s.listenLanguage.rawValue],
                externalMeetingLanguage: farLanguage.rawValue, displayName: nil, audioMode: audioMode.rawValue))
            self.claim = claim
            room = claim.room
            if !claim.isCapturer {
                if takeOver {
                    log("Takeover capturer…")
                    _ = try await app.api.bridgeTakeover(roomId: claim.room.id)
                } else {
                    log("Máy khác đang là capturer của phòng này.")
                    phase = .needsTakeover
                    return
                }
            }
            let roomId = claim.room.id
            log("Phòng \(claim.room.roomCode) (\(claim.created ? "mới" : "đã có")), role capturer")

            // Make sure the server-side mode matches what the user picked (409 BRIDGE_AUDIO_MODE_LOCKED possible).
            if claim.audioMode != audioMode.rawValue {
                do { _ = try await app.api.setBridgeAudioMode(roomId: roomId, mode: audioMode.rawValue) }
                catch let e as APIError where e.code == "BRIDGE_AUDIO_MODE_LOCKED" {
                    log("Không đổi được sang voice khi đang dịch (BRIDGE_AUDIO_MODE_LOCKED) — giữ text-only.")
                    audioMode = .text
                }
            }

            // 2. Start translation if not running (capturer may start it). Errors here are non-fatal.
            if !claim.room.translationActive {
                do { room = try await app.api.startRoom(roomId); log("Đã bắt đầu dịch") }
                catch { log("Start room: \((error as? LocalizedError)?.errorDescription ?? "\(error)") (bỏ qua)") }
            }

            // 3. Listener connection (captions + dub). Bridge token cannot subscribe, so this is a separate room connection.
            app.captions.clear()
            app.captions.listenLanguage = s.listenLanguage.rawValue
            live.listenLanguage = s.listenLanguage.rawValue
            live.hearOriginals = false
            live.onCaptionData = { [weak self] data in self?.app.captions.ingest(data: data) }
            if audioMode == .voice {
                let player = VoiceOutputPlayer(outputDeviceUID: s.dubOutputDeviceUID)
                voice = player
                live.silenceSDKPlayback = true
                live.dubPCMSink = { [player] buf in player.enqueue(buf) }
                if s.dubOutputDeviceUID.isEmpty { log("CẢNH BÁO: chưa chọn thiết bị đầu ra cho dub (Cài đặt) — sẽ phát ra loa mặc định.") }
            } else {
                live.dubPCMSink = nil
                live.silenceSDKPlayback = false
            }
            let listen = try await app.api.meetingJoin(roomId: roomId, displayName: nil)
            await live.connect(url: listen.serverUrl, token: listen.token, publishMic: false)
            log("Listener LiveKit: \(live.state)")

            // 4. Bridge publish connection
            let bt = try await app.api.bridgeToken(roomId: roomId)
            try await publisher.connect(url: bt.serverUrl, token: bt.token)
            log("Bridge LiveKit: đã kết nối (\(bt.participantIdentity))")

            // 5. Capture
            try await startCapture(target: target)

            // 6. Heartbeat + meter + overlay
            startHeartbeat(roomId: roomId, interval: max(5, claim.heartbeatIntervalSeconds))
            startMeter()
            app.overlay.show()
            phase = .running
        } catch {
            let msg = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
            log("LỖI: \(msg)")
            await teardown()
            phase = .failed(msg)
        }
    }

    private func startCapture(target: CaptureTarget) async throws {
        let primary: AudioCaptureSource = backend == .processTap ? ProcessTapCapture() : SCKAudioCapture()
        do {
            try await begin(primary, target: target)
        } catch {
            app.permissions.recordAudioCaptureResult(success: false)
            guard backend == .processTap, autoFallbackToSCK else { throw error }
            log("Process Tap lỗi (\((error as? LocalizedError)?.errorDescription ?? "\(error)")) → thử ScreenCaptureKit")
            let fallback = SCKAudioCapture()
            try await begin(fallback, target: target)
        }
    }

    private func begin(_ src: AudioCaptureSource, target: CaptureTarget) async throws {
        let activity = self.activity
        let publisher = self.publisher
        src.onBuffer = { buf in
            activity.record(rmsDbfs: Resampler.rmsDbfs(buf))
            publisher.push(buf)
        }
        src.onError = { [weak self] err in
            Task { @MainActor in self?.log("Capture lỗi: \(err.localizedDescription)") }
        }
        try await src.start(target: target)
        source = src
        captureName = src.displayName
        if src is ProcessTapCapture { app.permissions.recordAudioCaptureResult(success: true) }
        log("Đang capture \(target.name) bằng \(src.displayName)")
    }

    func stop() async {
        await teardown()
        phase = .idle
        log("Đã dừng")
    }

    /// Ends the bridge room on the server (host/capturer).
    func endRoom() async {
        if let r = room { _ = try? await app.api.endRoom(r.id) }
        await stop()
    }

    private func teardown() async {
        heartbeatTask?.cancel(); heartbeatTask = nil
        meterTask?.cancel(); meterTask = nil
        source?.stop(); source = nil
        voice?.stop(); voice = nil
        live.dubPCMSink = nil
        await publisher.disconnect()
        await live.disconnect()
        app.overlay.hide()
        levelDb = -120
    }

    // MARK: Heartbeat (every 15 s) + meter

    private func startHeartbeat(roomId: String, interval: Double) {
        heartbeatTask?.cancel()
        heartbeatTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                await self.beat(roomId: roomId)
                try? await Task.sleep(nanoseconds: UInt64(interval * 1_000_000_000))
            }
        }
    }

    private func beat(roomId: String) async {
        let snap = activity.snapshot()
        do {
            _ = try await app.api.bridgeHeartbeat(roomId: roomId, BridgeHeartbeatRequest(
                audioFlowing: snap.flowing, lastAudioAt: snap.lastAudioAt, rmsDbfs: snap.rmsDbfs))
            lastHeartbeat = Date()
        } catch let e as APIError where e.status == 409 {
            log("Mất quyền capturer (409). Dừng.")
            await teardown()
            phase = .needsTakeover
        } catch {
            log("Heartbeat lỗi: \((error as? LocalizedError)?.errorDescription ?? "\(error)")")
        }
    }

    private func startMeter() {
        meterTask?.cancel()
        meterTask = Task { [weak self] in
            while !Task.isCancelled {
                if let self { self.levelDb = self.activity.snapshot().rmsDbfs }
                try? await Task.sleep(nanoseconds: 200_000_000)
            }
        }
    }
}
