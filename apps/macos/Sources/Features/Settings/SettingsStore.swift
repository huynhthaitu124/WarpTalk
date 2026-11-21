import Foundation
import SwiftUI

/// User preferences (UserDefaults). Backend URL default points at the local dev API.
@MainActor
final class SettingsStore: ObservableObject {
    static let defaultBackendURL = "http://localhost:8080"

    @Published var backendURL: String { didSet { save("backendURL", backendURL) } }
    @Published var speakLanguage: Language { didSet { save("speakLanguage", speakLanguage.rawValue) } }
    @Published var listenLanguage: Language { didSet { save("listenLanguage", listenLanguage.rawValue) } }
    /// Output device UID for dub playback in External voice mode (e.g. "BlackHole2ch_UID"). "" = system default.
    @Published var dubOutputDeviceUID: String { didSet { save("dubOutputDeviceUID", dubOutputDeviceUID) } }
    /// Output device UID for native-meeting playback. "" = system default.
    @Published var playbackDeviceUID: String { didSet { save("playbackDeviceUID", playbackDeviceUID) } }
    /// Input device UID for the native-meeting microphone. "" = system default.
    @Published var micDeviceUID: String { didSet { save("micDeviceUID", micDeviceUID) } }
    @Published var hearOriginals: Bool { didSet { save("hearOriginals", hearOriginals) } }
    @Published var overlayClickThrough: Bool { didSet { save("overlayClickThrough", overlayClickThrough) } }
    @Published var overlayHiddenFromScreenShare: Bool { didSet { save("overlayHiddenFromScreenShare", overlayHiddenFromScreenShare) } }
    @Published var selectedWorkspaceId: String { didSet { save("selectedWorkspaceId", selectedWorkspaceId) } }

    private let d = UserDefaults.standard

    init() {
        let d = UserDefaults.standard
        backendURL = d.string(forKey: "backendURL") ?? Self.defaultBackendURL
        speakLanguage = Language(rawValue: d.string(forKey: "speakLanguage") ?? "") ?? .vi
        listenLanguage = Language(rawValue: d.string(forKey: "listenLanguage") ?? "") ?? .en
        dubOutputDeviceUID = d.string(forKey: "dubOutputDeviceUID") ?? ""
        playbackDeviceUID = d.string(forKey: "playbackDeviceUID") ?? ""
        micDeviceUID = d.string(forKey: "micDeviceUID") ?? ""
        hearOriginals = d.object(forKey: "hearOriginals") as? Bool ?? false
        overlayClickThrough = d.bool(forKey: "overlayClickThrough")
        overlayHiddenFromScreenShare = d.bool(forKey: "overlayHiddenFromScreenShare")
        selectedWorkspaceId = d.string(forKey: "selectedWorkspaceId") ?? ""
    }

    private func save(_ key: String, _ value: Any) { d.set(value, forKey: key) }
}
