import SwiftUI

@main
struct WarpTalkApp: App {
    @StateObject private var app = AppState()

    var body: some Scene {
        WindowGroup("WarpTalk") {
            RootView()
                .environmentObject(app)
                .environmentObject(app.settings)
                .environmentObject(app.captions)
                .environmentObject(app.permissions)
                .frame(minWidth: 960, minHeight: 640)
        }
        .commands {
            CommandGroup(after: .windowArrangement) {
                Button("Bật/tắt Overlay phụ đề") { app.overlay.toggle() }
                    .keyboardShortcut("o", modifiers: [.command, .shift])
            }
        }
    }
}
