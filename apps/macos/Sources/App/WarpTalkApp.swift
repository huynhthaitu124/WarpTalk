import SwiftUI

@main
struct WarpTalkApp: App {
    @StateObject private var app = AppState()

    var body: some Scene {
        WindowGroup("WarpTalk") {
            WebContainerView()
                .environmentObject(app.web)
                .frame(minWidth: 1000, minHeight: 680)
        }
        .commands {
            CommandGroup(after: .toolbar) {
                Button("Tải lại web") { app.web.reload() }.keyboardShortcut("r", modifiers: .command)
                Button("Bật/tắt Overlay phụ đề") { app.overlay.toggle() }.keyboardShortcut("o", modifiers: [.command, .shift])
            }
        }
        Settings {
            SettingsView()
                .environmentObject(app.settings)
                .environmentObject(app.web)
        }
    }
}
