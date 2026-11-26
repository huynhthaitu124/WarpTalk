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

enum OverlayMode: Equatable { case captions, copilot }

/// One live transcript line in copilot mode.
struct CopilotOverlayLine: Identifiable, Equatable {
    let id: String
    var channel: CopilotChannel
    var speaker: String
    var text: String
    var isFinal: Bool
}

/// Caption lines pushed from the web app (`pushOverlayCaption`). Upserts by `segmentId` when given.
/// Copilot mode (ADR 0001) additionally holds the live transcript from the core and the suggestions pushed by the web app.
@MainActor
final class OverlayModel: ObservableObject {
    @Published private(set) var lines: [OverlayLine] = []
    @Published private(set) var mode: OverlayMode = .captions
    @Published private(set) var copilotLines: [CopilotOverlayLine] = []
    @Published private(set) var suggestions: [OverlaySuggestion] = []
    private(set) var speakerNames: [CopilotChannel: String] = Self.defaultNames
    static let defaultNames: [CopilotChannel: String] = [.me: "Tôi", .others: "Người khác"]
    static let maxCopilotLines = 30
    static let maxSuggestions = 6
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
    func clear() { lines = []; copilotLines = []; suggestions = [] }

    // MARK: Copilot mode

    func enterCopilotMode(meName: String? = nil, othersName: String? = nil) {
        speakerNames = [.me: meName ?? Self.defaultNames[.me] ?? "", .others: othersName ?? Self.defaultNames[.others] ?? ""]
        copilotLines = []
        suggestions = []
        mode = .copilot
    }

    func exitCopilotMode() {
        mode = .captions
        speakerNames = Self.defaultNames
        copilotLines = []
        suggestions = []
    }

    /// Live transcript ordering: committed lines in arrival order (a segment with a known `clientSegmentId` updates its line in
    /// place and never downgrades final -> interim); the in-progress partial of a channel is ONE line (`partial-<channel>`)
    /// that updates in place and is replaced by the channel's next segment (which is appended at the end). Echo-flagged `me`
    /// segments are not shown.
    func applyCopilot(_ e: CopilotEngineEvent) {
        switch e {
        case .partial(let ch, let text):
            let pid = "partial-\(ch.rawValue)"
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.isEmpty { copilotLines.removeAll { $0.id == pid }; return }
            let line = CopilotOverlayLine(id: pid, channel: ch, speaker: speakerNames[ch] ?? "", text: trimmed, isFinal: false)
            if let i = copilotLines.firstIndex(where: { $0.id == pid }) { copilotLines[i] = line } else { copilotLines.append(line) }
            trimCopilot()
        case .segment(let s):
            let text = s.text.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !text.isEmpty, !s.likelyEcho else { return }
            copilotLines.removeAll { $0.id == "partial-\(s.channel.rawValue)" }
            let line = CopilotOverlayLine(id: s.clientSegmentId, channel: s.channel, speaker: speakerNames[s.channel] ?? "", text: text, isFinal: s.isFinal)
            if let i = copilotLines.firstIndex(where: { $0.id == line.id }) {
                if copilotLines[i].isFinal && !line.isFinal { return }
                copilotLines[i] = line
            } else {
                copilotLines.append(line)
            }
            trimCopilot()
        default:
            break
        }
    }

    private func trimCopilot() {
        if copilotLines.count > Self.maxCopilotLines { copilotLines.removeFirst(copilotLines.count - Self.maxCopilotLines) }
    }

    func recentCopilot(_ n: Int) -> [CopilotOverlayLine] { Array(copilotLines.suffix(n)) }

    // MARK: Suggestions (pushed by the web app)

    /// Same `id` replaces in place (keeps its position); new ids go last; the oldest are dropped beyond `maxSuggestions`.
    func upsertSuggestion(_ s: OverlaySuggestion) {
        if let i = suggestions.firstIndex(where: { $0.id == s.id }) { suggestions[i] = s } else { suggestions.append(s) }
        if suggestions.count > Self.maxSuggestions { suggestions.removeFirst(suggestions.count - Self.maxSuggestions) }
    }

    func removeSuggestion(id: String) { suggestions.removeAll { $0.id == id } }

    /// The newest `n` suggestions (what the overlay shows).
    func visibleSuggestions(_ n: Int = 3) -> [OverlaySuggestion] { Array(suggestions.suffix(n)) }
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
        // @Published sinks fire before the property changes (willSet semantics): re-layout on the next runloop turn.
        settings.$overlayPosition.dropFirst().sink { [weak self] _ in DispatchQueue.main.async { self?.applyLayout() } }.store(in: &cancellables)
        model.$mode.dropFirst().sink { [weak self] _ in DispatchQueue.main.async { self?.applyLayout() } }.store(in: &cancellables)
    }

    func setVisible(_ v: Bool) { v ? show() : hide() }
    func toggle() { isVisible ? hide() : show() }

    func show() {
        guard let model, let settings else { return }
        if panel == nil {
            let p = OverlayPanel(contentRect: targetFrame(), styleMask: [.borderless, .nonactivatingPanel, .resizable],
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
        applyLayout()
        panel?.orderFrontRegardless()                 // never activates WarpTalk
    }

    /// Size by mode (copilot mode also shows suggestions) and origin by the `overlayPosition` setting.
    private func targetFrame() -> NSRect {
        let screen = NSScreen.main?.visibleFrame ?? NSRect(x: 0, y: 0, width: 1440, height: 900)
        let copilot = model?.mode == .copilot
        let size = NSSize(width: min(copilot ? 720 : 900, screen.width - 80), height: copilot ? 240 : 150)
        let origin = (settings?.overlayPosition ?? .bottomCenter).origin(in: screen, size: size)
        return NSRect(origin: origin, size: size)
    }

    func applyLayout() {
        guard let panel else { return }
        panel.setFrame(targetFrame(), display: true)
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
        switch model.mode {
        case .captions: CaptionOverlayContent()
        case .copilot: CopilotOverlayContent()
        }
    }
}

private struct CaptionOverlayContent: View {
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

// VERIFY: Text + Text with .foregroundStyle(Color) (macOS 14 Text.foregroundStyle returns Text) compiles as written.
/// Copilot mode: last 3 transcript lines with a speaker label, and a suggestions / follow-ups area underneath.
private struct CopilotOverlayContent: View {
    @EnvironmentObject var model: OverlayModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(model.recentCopilot(3)) { line in
                    (Text("\(line.speaker): ").font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(line.channel == .me ? Color.cyan : Color.orange)
                     + Text(line.text).font(.system(size: 17, weight: .medium)).foregroundStyle(Color.white))
                        .opacity(line.isFinal ? 1 : 0.7)
                        .lineLimit(2)
                }
                if model.copilotLines.isEmpty {
                    Text("Đang lắng nghe…").font(.system(size: 15)).foregroundStyle(.white.opacity(0.6))
                }
            }
            let shown = model.visibleSuggestions()
            if !shown.isEmpty {
                Divider().overlay(Color.white.opacity(0.25))
                VStack(alignment: .leading, spacing: 6) {
                    ForEach(shown) { s in SuggestionRow(suggestion: s) }
                }
            }
        }
        .padding(14)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .bottomLeading)
        .background(RoundedRectangle(cornerRadius: 14).fill(.black.opacity(0.72)))
    }
}

private struct SuggestionRow: View {
    let suggestion: OverlaySuggestion

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                Text((suggestion.isFollowUp ? "Follow-up" : suggestion.category).uppercased())
                    .font(.system(size: 10, weight: .bold)).foregroundStyle(.black)
                    .padding(.horizontal, 5).padding(.vertical, 1)
                    .background(RoundedRectangle(cornerRadius: 4).fill(suggestion.isFollowUp ? Color.green : Color.yellow))
                Text(suggestion.content).font(.system(size: 14)).foregroundStyle(.white).lineLimit(3)
            }
            if let f = suggestion.followUp, !f.isEmpty {
                Text("↳ \(f)").font(.system(size: 13)).foregroundStyle(.green.opacity(0.9)).lineLimit(2)
            }
        }
    }
}
