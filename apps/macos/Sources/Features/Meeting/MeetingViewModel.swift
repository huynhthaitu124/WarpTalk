import Foundation
import SwiftUI

/// Native WarpTalk meeting: join token -> LiveKit -> mic + dub playback + captions.
@MainActor
final class MeetingViewModel: ObservableObject {
    let room: TranslationRoom
    let live: LiveKitSession
    private let app: AppState
    @Published var busy = false
    @Published var error: String?
    @Published var roomState: TranslationRoom?

    init(room: TranslationRoom, app: AppState) {
        self.room = room
        self.app = app
        self.roomState = room
        self.live = LiveKitSession(listenLanguage: app.settings.listenLanguage.rawValue,
                                   hearOriginals: app.settings.hearOriginals)
    }

    func join(displayName: String?) async {
        busy = true; defer { busy = false }
        let s = app.settings
        do {
            // 1. Be a participant of the room (idempotent on the server for repeated joins is not guaranteed:
            //    a Conflict here just means "already joined").
            do {
                _ = try await app.api.joinRoom(id: room.id, JoinByIdRequest(displayName: displayName,
                                                                           speakLanguage: s.speakLanguage.rawValue,
                                                                           listenLanguage: s.listenLanguage.rawValue))
            } catch let e as APIError where e.status == 409 {
                // already joined
            }
            // 2. LiveKit grant
            let grant = try await app.api.meetingJoin(roomId: room.id, displayName: displayName)
            app.captions.clear()
            app.captions.listenLanguage = s.listenLanguage.rawValue
            live.listenLanguage = s.listenLanguage.rawValue
            live.hearOriginals = s.hearOriginals
            live.onCaptionData = { [weak self] data in self?.app.captions.ingest(data: data) }
            LiveKitAudioDevices.apply(inputUID: s.micDeviceUID, outputUID: s.playbackDeviceUID)
            await live.connect(url: grant.serverUrl, token: grant.token, publishMic: true)
            app.overlay.show()
        } catch {
            self.error = (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
        }
    }

    func start() async {
        do { roomState = try await app.api.startRoom(room.id) } catch { report(error) }
    }

    func endMeeting() async {
        do { roomState = try await app.api.endRoom(room.id) } catch { report(error) }
        await leave()
    }

    func leave() async {
        await live.disconnect()
        app.overlay.hide()
        try? await app.api.leaveRoom(room.id)
    }

    func refreshRoom() async {
        if let r = try? await app.api.getRoom(room.id) { roomState = r }
    }

    var isHost: Bool { app.session?.userId == (roomState ?? room).effectiveHostId }

    private func report(_ e: Error) { error = (e as? LocalizedError)?.errorDescription ?? e.localizedDescription }
}
