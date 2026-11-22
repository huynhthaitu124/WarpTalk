import AVFoundation
import Foundation
import LiveKit

/// Voice mode only (D7): a listener LiveKit connection (join token from `/meetings/{id}/join` with `clientTag: "bridge-listener"`,
/// identity `{userId}:bridge-listener`, subscribe-only) that subscribes ONLY to the track named
/// `dub-{listenLanguage}` and hands its PCM to `pcmSink` (-> VoiceOutputPlayer -> BlackHole).
/// Captions are normally handled by the web app (it pushes overlay lines); `onCaptionData` exposes the raw `captions`
/// packets of this connection for the opt-in `nativeOverlayCaptions` mode.
/// WKWebView cannot route audio to a chosen output device (no setSinkId), which is why this lives natively.
///
/// LiveKit Swift SDK API verified against client-sdk-swift 2.19.0 (all paths under Sources/LiveKit/):
///   Room(delegate:connectOptions:roomOptions:) Core/Room.swift:277 · connect(url:token:connectOptions:roomOptions:) :385
///   remoteParticipants Room.swift:48 · Participant.audioTracks Participant/Participant.swift:82
///   TrackPublication.name/sid/track TrackPublications/TrackPublication.swift:27-35 (Track.Sid.stringValue Types/Track+Types.swift:23)
///   RemoteTrackPublication.isSubscribed/set(subscribed:) TrackPublications/RemoteTrackPublication.swift:48,63
///   RemoteAudioTrack.add(audioRenderer:) Track/Remote/RemoteAudioTrack.swift (AudioRenderer.render(pcmBuffer:) Protocols/AudioRenderer.swift:27)
///   RoomDelegate (all methods `@objc optional`) Protocols/RoomDelegate.swift:35-190
/// The SDK does not play remote audio in manual rendering mode (`LiveKitAudio`), so the track volume is left alone.
/// VERIFY (first run): that `AudioRenderer.render(pcmBuffer:)` still fires for remote tracks in manual rendering mode. The SDK doc
/// (AudioManager.swift:408-410) says individual tracks are readable via `RemoteAudioTrack.add(audioRenderer:)` in that mode, but no test covers it.
@MainActor
final class DubListener {
    private let room: Room
    private let renderer = DubRenderer()
    private let delegate: DubRoomDelegate
    private var task: Task<Void, Never>?
    private var attachedSid: String?
    private var lastSubRequest: [String: Date] = [:]
    let language: String
    private(set) var subscribedTrackName: String?

    /// Raw `captions` data packets (JSON), delivered on the main actor.
    var onCaptionData: (@MainActor (Data) -> Void)?
    /// The connection dropped for good (after the SDK's own reconnect attempts).
    var onDisconnect: (@MainActor (String) -> Void)?

    init(language: String, pcmSink: @escaping @Sendable (AVAudioPCMBuffer) -> Void) {
        self.language = language
        renderer.sink = pcmSink
        let delegate = DubRoomDelegate()
        self.delegate = delegate
        room = Room(delegate: delegate, connectOptions: ConnectOptions(autoSubscribe: false), roomOptions: RoomOptions())
        delegate.onTrackEvent = { [weak self] in Task { @MainActor in self?.reconcile() } }
        delegate.onData = { [weak self] data in Task { @MainActor in self?.onCaptionData?(data) } }
        delegate.onDisconnect = { [weak self] message in Task { @MainActor in self?.onDisconnect?(message) } }
    }

    func connect(url: String, token: String) async throws {
        try LiveKitAudio.ensureManualRendering()
        try await room.connect(url: url, token: token)
        reconcile()
        // The delegate triggers reconcile on publish/subscribe; this slow poll only covers missed events.
        task = Task { [weak self] in
            while !Task.isCancelled { try? await Task.sleep(nanoseconds: 1_000_000_000); self?.reconcile() }
        }
    }

    func disconnect() async {
        task?.cancel(); task = nil
        await room.disconnect()
        attachedSid = nil; subscribedTrackName = nil
    }

    private func reconcile() {
        let wanted = "dub-\(language)"
        var active: String?
        for p in room.remoteParticipants.values {
            for pub in p.audioTracks {
                guard let rpub = pub as? RemoteTrackPublication else { continue }
                let name = rpub.name
                let want = (name == wanted)            // originals and other languages are never subscribed
                let sid = rpub.sid.stringValue
                if rpub.isSubscribed != want,
                   Date().timeIntervalSince(lastSubRequest[sid] ?? .distantPast) > 3 {
                    lastSubRequest[sid] = Date()
                    Task { try? await rpub.set(subscribed: want) }
                }
                if want {
                    active = name
                    if attachedSid != sid, let track = rpub.track as? RemoteAudioTrack {
                        track.add(audioRenderer: renderer)
                        attachedSid = sid
                    }
                }
            }
        }
        subscribedTrackName = active
    }
}

/// Receives decoded PCM of the subscribed dub track (called on a WebRTC audio thread).
final class DubRenderer: NSObject, AudioRenderer, @unchecked Sendable {
    var sink: (@Sendable (AVAudioPCMBuffer) -> Void)?
    func render(pcmBuffer: AVAudioPCMBuffer) { sink?(pcmBuffer) }
}

/// `RoomDelegate` is an `@objc` protocol with optional methods, so a small NSObject adapter forwards what we need.
final class DubRoomDelegate: NSObject, RoomDelegate, @unchecked Sendable {
    var onTrackEvent: (@Sendable () -> Void)?
    var onData: (@Sendable (Data) -> Void)?
    var onDisconnect: (@Sendable (String) -> Void)?

    func room(_ room: Room, participant: RemoteParticipant, didPublishTrack publication: RemoteTrackPublication) { onTrackEvent?() }

    func room(_ room: Room, participant: RemoteParticipant, didSubscribeTrack publication: RemoteTrackPublication) { onTrackEvent?() }

    func room(_ room: Room, participant: RemoteParticipant?, didReceiveData data: Data, forTopic topic: String, encryptionType: EncryptionType) {
        if topic == "captions" { onData?(data) }      // warptalk_contracts::media::CAPTIONS_TOPIC
    }

    func room(_ room: Room, didDisconnectWithError error: LiveKitError?) {
        onDisconnect?(error.map { "Mất kết nối nghe dub: \($0.localizedDescription)" } ?? "Mất kết nối nghe dub.")
    }
}
