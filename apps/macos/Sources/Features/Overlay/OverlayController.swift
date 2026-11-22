import AppKit
import Combine
import SwiftUI

struct OverlayLine: Identifiable, Equatable {
    let id: String
    var speaker: String
    /// Pipeline agent identities and empty names are not speakers: show the text alone (contract B-03).
    var speakerPrefix: String { speaker.isEmpty || speaker.hasPrefix("pipeline-agent") ? "" : "\(speaker): " }
    var text: String
    var translation: String?
    var isFinal: Bool
}

/// Caption lines pushed from the web app (`pushOverlayCaption`). Upserts by `segmentId` when given.
@MainActor
final class OverlayModel: ObservableObject {
    @Published private(set) var lines: [OverlayLine] = []
    private let maxLines = 20
    private var counter = 0

    func push(speaker: String, text: String, translation: String?, segmentId: String?, isFinal: Bool) {
        if let sid = segmentId, let i = lines.lastIndex(where: { $0.id == sid }) {
            if lines[i].isFinal && !isFinal { return }          // never downgrade a final line with a late interim
            lines[i] = OverlayLine(id: sid, speaker: speaker, text: text, translation: translation ?? lines[i].translation, isFinal: isFinal)
            return
        }
        counter += 1
        lines.append(OverlayLine(id: segmentId ?? "local-\(counter)", speaker: speaker, text: text, translation: translation, isFinal: isFinal))
        if lines.count > maxLines { lines.removeFirst(lines.count - maxLines) }
    }

    func recent(_ n: Int) -> [OverlayLine] { Array(lines.suffix(n)) }
    func clear() { lines = [] }
}

/// Non-activating floating caption window. Visible over full-screen apps and on every Space.
final class OverlayPanel: NSPanel {
    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

@MainActor
final class OverlayController {
    private var panel: OverlayPanel?
    private var settings: SettingsStore?
    private var model: OverlayModel?
    private var cancellables = Set<AnyCancellable>()

    var isVisible: Bool { panel?.isVisible ?? false }

    func bind(settings: SettingsStore, model: OverlayModel) {
        self.settings = settings
        self.model = model
        settings.$overlayClickThrough.sink { [weak self] _ in self?.applySettings() }.store(in: &cancellables)
        settings.$overlayHiddenFromScreenShare.sink { [weak self] _ in self?.applySettings() }.store(in: &cancellables)
    }

    func setVisible(_ v: Bool) { v ? show() : hide() }
    func toggle() { isVisible ? hide() : show() }

    func show() {
        guard let model, let settings else { return }
        if panel == nil {
            let screen = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
            let size = NSSize(width: min(900, screen.width - 80), height: 150)
            let rect = NSRect(x: screen.midX - size.width / 2, y: screen.minY + 60, width: size.width, height: size.height)
            let p = OverlayPanel(contentRect: rect, styleMask: [.borderless, .nonactivatingPanel, .resizable],
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
            p.contentView = NSHostingView(rootView: OverlayView().environmentObject(model).environmentObject(settings))
            panel = p
        }
        applySettings()
        panel?.orderFrontRegardless()                 // never activates WarpTalk
    }

    func hide() { panel?.orderOut(nil) }

    func applySettings() {
        guard let panel, let settings else { return }
        panel.ignoresMouseEvents = settings.overlayClickThrough
        // .none keeps the overlay out of screen shares (best effort; some capture paths ignore it).
        panel.sharingType = settings.overlayHiddenFromScreenShare ? .none : .readOnly
    }
}

struct OverlayView: View {
    @EnvironmentObject var model: OverlayModel

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(model.recent(3)) { line in
                VStack(alignment: .leading, spacing: 1) {
                    if let t = line.translation, !t.isEmpty {
                        Text(t).font(.system(size: 22, weight: .semibold)).foregroundStyle(.white)
                        Text("\(line.speakerPrefix)\(line.text)").font(.system(size: 13)).foregroundStyle(.white.opacity(0.65))
                    } else {
                        Text("\(line.speakerPrefix)\(line.text)").font(.system(size: 20, weight: .medium)).foregroundStyle(.white)
                    }
                }
                .opacity(line.isFinal ? 1 : 0.7)
            }
            if model.lines.isEmpty {
                Text("Đang chờ phụ đề…").font(.system(size: 16)).foregroundStyle(.white.opacity(0.6))
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
        .background(RoundedRectangle(cornerRadius: 14).fill(.black.opacity(0.72)))
    }
}
