import AppKit
import Combine
import SwiftUI

/// Non-activating floating caption window. Visible over full-screen apps and on every Space.
final class OverlayPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

@MainActor
final class OverlayController {
    private var panel: OverlayPanel?
    private var settings: SettingsStore?
    private var captions: CaptionStore?
    private var cancellables = Set<AnyCancellable>()

    var isVisible: Bool { panel?.isVisible ?? false }

    func bind(settings: SettingsStore, captions: CaptionStore) {
        self.settings = settings
        self.captions = captions
        settings.$overlayClickThrough.sink { [weak self] _ in self?.applySettings() }.store(in: &cancellables)
        settings.$overlayHiddenFromScreenShare.sink { [weak self] _ in self?.applySettings() }.store(in: &cancellables)
    }

    func toggle() { isVisible ? hide() : show() }

    func show() {
        guard let captions, let settings else { return }
        if panel == nil {
            let screen = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
            let size = NSSize(width: min(900, screen.width - 80), height: 150)
            let rect = NSRect(x: screen.midX - size.width / 2, y: screen.minY + 60, width: size.width, height: size.height)
            let p = OverlayPanel(contentRect: rect,
                                 styleMask: [.borderless, .nonactivatingPanel, .resizable],
                                 backing: .buffered, defer: false)
            p.level = .statusBar                      // above normal & floating windows (Chrome full screen included)
            p.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle]
            p.isFloatingPanel = true
            p.hidesOnDeactivate = false
            p.becomesKeyOnlyIfNeeded = true
            p.isOpaque = false
            p.backgroundColor = .clear
            p.hasShadow = false
            p.isMovableByWindowBackground = true
            p.contentView = NSHostingView(rootView: OverlayView().environmentObject(captions).environmentObject(settings))
            panel = p
        }
        applySettings()
        panel?.orderFrontRegardless()                 // never activates WarpTalk
    }

    func hide() { panel?.orderOut(nil) }

    func applySettings() {
        guard let panel, let settings else { return }
        panel.ignoresMouseEvents = settings.overlayClickThrough
        // .none keeps the overlay out of screen shares / recordings (best effort; some capture paths ignore it).
        panel.sharingType = settings.overlayHiddenFromScreenShare ? .none : .readOnly
    }
}

struct OverlayView: View {
    @EnvironmentObject var captions: CaptionStore

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(captions.recent(3)) { line in
                VStack(alignment: .leading, spacing: 1) {
                    if let t = line.translation {
                        Text(t).font(.system(size: 22, weight: .semibold)).foregroundStyle(.white)
                        Text("\(line.speaker): \(line.original)").font(.system(size: 13)).foregroundStyle(.white.opacity(0.65))
                    } else {
                        Text("\(line.speaker): \(line.original)").font(.system(size: 20, weight: .medium)).foregroundStyle(.white)
                    }
                }
                .opacity(line.isFinal ? 1 : 0.7)
            }
            if captions.lines.isEmpty {
                Text("Đang chờ phụ đề…").font(.system(size: 16)).foregroundStyle(.white.opacity(0.6))
            }
            if let b = captions.latencyBadge {
                Text("⏱ \(b)").font(.system(size: 11).monospacedDigit()).foregroundStyle(.white.opacity(0.5))
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
        .background(RoundedRectangle(cornerRadius: 14).fill(.black.opacity(0.72)))
    }
}
