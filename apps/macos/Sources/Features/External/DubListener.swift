import AVFoundation
import Foundation
import LiveKit

/// Voice mode only (D7): a listener LiveKit connection (join token from `/meetings/{id}/join`) that subscribes ONLY to
/// `dub-{listenLanguage}` and hands its PCM to `pcmSink` (-> VoiceOutputPlayer -> BlackHole). Captions are NOT handled
/// here: the web app owns them and pushes overlay lines via `pushOverlayCaption`.
/// WKWebView cannot route audio to a chosen output device (no setSinkId), which is why this lives natively.
///
/// LiveKit Swift SDK 2.x names are from memory (no toolchain here); shaky ones carry `// VERIFY:`.
@MainActor
final class DubListener {
    private let room: Room
    private let renderer = DubRenderer()
    private var task: Task<Void, Never>?
    private var attachedSid: String?
    private var lastSubRequest: [String: Date] = [:]
    let language: String
    private(set) var subscribedTrackName: String?

    init(language: String, pcmSink: @escaping @Sendable (AVAudioPCMBuffer) -> Void) {
        self.language = language
        renderer.sink = pcmSink
        // VERIFY: Room init + ConnectOptions(autoSubscribe:) + RoomOptions in 2.x.
        room = Room(delegate: nil, connectOptions: ConnectOptions(autoSubscribe: false),
                    roomOptions: RoomOptions(adaptiveStream: false, dynacast: false))
    }

    func connect(url: String, token: String) async throws {
        try await room.connect(url: url, token: token)   // VERIFY: connect(url:token:)
        task = Task { [weak self] in
            while !Task.isCancelled { self?.reconcile(); try? await Task.sleep(nanoseconds: 400_000_000) }
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
        for p in room.remoteParticipants.values {      // VERIFY: remoteParticipants: [Participant.Identity: RemoteParticipant]
            for pub in p.audioTracks {                 // VERIFY: Participant.audioTracks: [TrackPublication]
                guard let rpub = pub as? RemoteTrackPublication else { continue }
                let name = rpub.name                   // VERIFY: TrackPublication.name
                let want = (name == wanted)            // originals and other languages are never subscribed
                let sid = rpub.sid.stringValue         // VERIFY: Track.Sid.stringValue
                if rpub.isSubscribed != want,          // VERIFY: RemoteTrackPublication.isSubscribed
                   Date().timeIntervalSince(lastSubRequest[sid] ?? .distantPast) > 3 {
                    lastSubRequest[sid] = Date()
                    Task { try? await rpub.set(subscribed: want) }   // VERIFY: set(subscribed:) async throws
                }
                if want {
                    active = name
                    if attachedSid != sid, let track = rpub.track as? RemoteAudioTrack {   // VERIFY: RemoteAudioTrack
                        track.add(audioRenderer: renderer)   // VERIFY: AudioTrack.add(audioRenderer:)
                        track.volume = 0                      // VERIFY: RemoteAudioTrack.volume; silence SDK playback, we render ourselves
                        attachedSid = sid
                    }
                }
            }
        }
        subscribedTrackName = active
    }
}

/// Receives decoded PCM of the subscribed dub track.
final class DubRenderer: NSObject, AudioRenderer, @unchecked Sendable {
    var sink: (@Sendable (AVAudioPCMBuffer) -> Void)?
    // VERIFY: protocol requirement name/signature in 2.x (`render(pcmBuffer:)`).
    func render(pcmBuffer: AVAudioPCMBuffer) { sink?(pcmBuffer) }
}
