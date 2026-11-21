import AVFoundation
import Foundation
import LiveKit

/// One LiveKit room connection used to LISTEN (and optionally speak):
///  - connects with the join token from `POST /meetings/{id}/join`
///  - `autoSubscribe = false`: we subscribe manually and ONLY to the `dub-{listenLanguage}` audio track
///    (plus original tracks when `hearOriginals` is on)
///  - receives captions on data topic "captions"
///  - exposes the participant list with speaking state
///
/// LiveKit Swift SDK 2.x API names below are from memory (no toolchain here); every shaky one has `// VERIFY:`.
@MainActor
final class LiveKitSession: ObservableObject, RoomDelegate {
    struct ParticipantRow: Identifiable, Equatable {
        let id: String
        var name: String
        var isSpeaking: Bool
        var isLocal: Bool
        var micOn: Bool
    }

    @Published private(set) var state: String = "Chưa kết nối"
    @Published private(set) var isConnected = false
    @Published private(set) var participants: [ParticipantRow] = []
    @Published private(set) var availableDubTracks: [String] = []
    @Published private(set) var subscribedDub: String?
    @Published var micEnabled = true
    @Published var lastError: String?

    var listenLanguage: String
    var hearOriginals: Bool
    /// Called with every caption data packet (main actor).
    var onCaptionData: ((Data) -> Void)?
    /// If set, dub audio is delivered here as PCM instead of (only) being played by the SDK (External voice mode).
    var dubPCMSink: (@Sendable (AVAudioPCMBuffer) -> Void)? { didSet { dubRenderer.sink = dubPCMSink } }
    /// When true the SDK's own playback of the dub track is silenced (we render it ourselves).
    var silenceSDKPlayback = false

    private let room: Room
    private let dubRenderer = DubRenderer()
    private var reconcileTask: Task<Void, Never>?
    private var rendererAttachedTo: String?   // track sid string
    private var loggedFirstCaption = false

    init(listenLanguage: String, hearOriginals: Bool) {
        self.listenLanguage = listenLanguage
        self.hearOriginals = hearOriginals
        // VERIFY: Room init signature + ConnectOptions(autoSubscribe:) + RoomOptions defaults in 2.x.
        self.room = Room(delegate: nil,
                         connectOptions: ConnectOptions(autoSubscribe: false),
                         roomOptions: RoomOptions(adaptiveStream: false, dynacast: false))
        room.add(delegate: self)   // VERIFY: Room.add(delegate:)
    }

    // MARK: Lifecycle

    func connect(url: String, token: String, publishMic: Bool) async {
        state = "Đang kết nối…"
        do {
            try await room.connect(url: url, token: token)   // VERIFY: connect(url:token:) with options set in init
            isConnected = true
            state = "Đã kết nối"
            if publishMic {
                try await room.localParticipant.setMicrophone(enabled: true)
                micEnabled = true
            }
            startReconcile()
        } catch {
            isConnected = false
            state = "Kết nối lỗi"
            lastError = error.localizedDescription
        }
    }

    func disconnect() async {
        reconcileTask?.cancel(); reconcileTask = nil
        await room.disconnect()
        isConnected = false
        state = "Đã ngắt"
        participants = []
        availableDubTracks = []
        subscribedDub = nil
        rendererAttachedTo = nil
    }

    func setMic(_ on: Bool) async {
        do { try await room.localParticipant.setMicrophone(enabled: on); micEnabled = on }
        catch { lastError = error.localizedDescription }
    }

    // MARK: Reconcile loop (participants, speaking, subscriptions)

    private func startReconcile() {
        reconcileTask?.cancel()
        reconcileTask = Task { [weak self] in
            while !Task.isCancelled {
                self?.reconcile()
                try? await Task.sleep(nanoseconds: 400_000_000)
            }
        }
    }

    private static func identity(_ p: Participant) -> String {
        // VERIFY: `identity` is `Participant.Identity?` (struct with .stringValue) in 2.x; String? in older 2.0.x.
        p.identity?.stringValue ?? ""
    }

    private func reconcile() {
        var rows: [ParticipantRow] = []
        let local = room.localParticipant
        rows.append(.init(id: Self.identity(local), name: local.name ?? "Tôi", isSpeaking: local.isSpeaking,
                          isLocal: true, micOn: local.isMicrophoneEnabled()))   // VERIFY: isMicrophoneEnabled() method

        var dubNames: [String] = []
        let wanted = "dub-\(listenLanguage)"
        var activeDub: String?

        for p in room.remoteParticipants.values {   // VERIFY: remoteParticipants: [Participant.Identity: RemoteParticipant]
            var micOn = false
            for pub in p.audioTracks {              // VERIFY: Participant.audioTracks: [TrackPublication]
                guard let rpub = pub as? RemoteTrackPublication else { continue }
                let name = rpub.name                // VERIFY: TrackPublication.name
                let isDub = name.hasPrefix("dub-")
                if isDub { dubNames.append(name) } else if !rpub.isMuted { micOn = true }

                let want = isDub ? (name == wanted) : hearOriginals
                if isDub && want { activeDub = name }
                if rpub.isSubscribed != want {      // VERIFY: RemoteTrackPublication.isSubscribed
                    Task { try? await rpub.set(subscribed: want) }   // VERIFY: set(subscribed:) async throws
                }
                if isDub && want { attachRendererIfNeeded(rpub) }
            }
            rows.append(.init(id: Self.identity(p), name: p.name ?? Self.identity(p), isSpeaking: p.isSpeaking, isLocal: false, micOn: micOn))
        }
        participants = rows
        let sortedDubs = Array(Set(dubNames)).sorted()
        if sortedDubs != availableDubTracks { availableDubTracks = sortedDubs }
        if subscribedDub != activeDub { subscribedDub = activeDub }
    }

    private func attachRendererIfNeeded(_ pub: RemoteTrackPublication) {
        guard let track = pub.track as? RemoteAudioTrack else { return }   // VERIFY: RemoteAudioTrack type name
        let key = pub.sid.stringValue                                       // VERIFY: Track.Sid.stringValue
        guard rendererAttachedTo != key else { return }
        if dubPCMSink != nil {
            track.add(audioRenderer: dubRenderer)                            // VERIFY: AudioTrack.add(audioRenderer:) / AudioRenderer.render(pcmBuffer:)
            if silenceSDKPlayback { track.volume = 0 }                       // VERIFY: RemoteAudioTrack.volume (0...1) and that renderers still get PCM at 0
        }
        rendererAttachedTo = key
    }

    // MARK: RoomDelegate

    // VERIFY: exact delegate signature. 2.x: room(_:participant:didReceiveData:forTopic:); newer 2.x adds `encryptionType:`.
    // If the signature does not match, the default (no-op) implementation is used and captions silently never arrive:
    // check the console line "[captions] first packet" after connecting.
    nonisolated func room(_ room: Room, participant: RemoteParticipant?, didReceiveData data: Data, forTopic topic: String) {
        guard topic == Caption.topic else { return }
        Task { @MainActor in
            if !self.loggedFirstCaption { self.loggedFirstCaption = true; print("[captions] first packet (\(data.count) bytes)") }
            self.onCaptionData?(data)
        }
    }

    nonisolated func room(_ room: Room, didUpdateConnectionState connectionState: ConnectionState, from oldValue: ConnectionState) {
        // VERIFY: signature; used only for the status label.
        Task { @MainActor in
            self.state = "\(connectionState)"
            self.isConnected = (connectionState == .connected)
        }
    }
}

/// Receives decoded PCM of the subscribed dub track.
final class DubRenderer: NSObject, AudioRenderer, @unchecked Sendable {
    var sink: (@Sendable (AVAudioPCMBuffer) -> Void)?
    // VERIFY: protocol requirement name/signature in 2.x (`render(pcmBuffer:)`).
    func render(pcmBuffer: AVAudioPCMBuffer) { sink?(pcmBuffer) }
}

/// Selects mic / speaker for the LiveKit audio engine on macOS.
enum LiveKitAudioDevices {
    @MainActor
    static func apply(inputUID: String, outputUID: String) {
        // VERIFY: AudioManager.shared.inputDevices/outputDevices ([AudioDevice], `deviceId` = Core Audio UID) and the
        // settable `inputDevice` / `outputDevice` properties exist on macOS in 2.x.
        if !inputUID.isEmpty, let d = AudioManager.shared.inputDevices.first(where: { $0.deviceId == inputUID }) {
            AudioManager.shared.inputDevice = d
        }
        if !outputUID.isEmpty, let d = AudioManager.shared.outputDevices.first(where: { $0.deviceId == outputUID }) {
            AudioManager.shared.outputDevice = d
        }
    }
}
