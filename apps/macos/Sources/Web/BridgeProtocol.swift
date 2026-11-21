import Foundation

/// Pure (WebKit-free) parts of the JS <-> native bridge, so they can be unit-tested. Contract: apps/macos/BRIDGE.md.
struct BridgeRequest {
    let method: String
    let params: [String: Any]

    init?(_ raw: Any) {
        guard let d = raw as? [String: Any], let m = d["method"] as? String, !m.isEmpty else { return nil }
        method = m
        params = (d["params"] as? [String: Any]) ?? [:]
    }

    func string(_ key: String) throws -> String {
        guard let v = params[key] as? String, !v.isEmpty else {
            throw BridgeError(code: "INVALID_ARGUMENT", message: "Thiếu hoặc sai tham số '\(key)' (string).")
        }
        return v
    }
    func optString(_ key: String) -> String? { (params[key] as? String).flatMap { $0.isEmpty ? nil : $0 } }
    func optBool(_ key: String) -> Bool? { params[key] as? Bool }
    func optDouble(_ key: String) -> Double? { (params[key] as? NSNumber)?.doubleValue }
}

enum BridgeReply {
    /// `{ ok: true, data }` — the injected JS resolves with `data`.
    static func ok(_ data: Any = NSNull()) -> [String: Any] { ["ok": true, "data": data] }
    /// `{ ok: false, error: {code, message} }` — the injected JS rejects with an Error carrying `.code`.
    static func fail(_ code: String, _ message: String) -> [String: Any] { ["ok": false, "error": ["code": code, "message": message]] }
}

/// Only the configured web app origin may use the bridge (scheme + host + port).
enum OriginPolicy {
    static func isTrusted(scheme: String, host: String, port: Int, allowed: String) -> Bool {
        guard let u = URL(string: allowed), let aScheme = u.scheme?.lowercased(), let aHost = u.host?.lowercased() else { return false }
        func eff(_ scheme: String, _ port: Int?) -> Int {
            if let p = port, p != 0 { return p }
            return scheme == "https" ? 443 : (scheme == "http" ? 80 : 0)
        }
        return scheme.lowercased() == aScheme && host.lowercased() == aHost && eff(scheme.lowercased(), port) == eff(aScheme, u.port)
    }
}

/// JS injected at document start (main frame, page world). Exposes `window.warptalkNative`.
enum BridgeScript {
    static let source = """
    (() => {
      if (window.warptalkNative) return;
      const call = async (method, params) => {
        const r = await window.webkit.messageHandlers.warptalk.postMessage({ method, params: params || {} });
        if (r && r.ok) return r.data;
        const e = new Error((r && r.error && r.error.message) || 'native bridge error');
        e.code = (r && r.error && r.error.code) || 'INTERNAL';
        throw e;
      };
      window.warptalkNative = Object.freeze({
        version: 1,
        getCapabilities: () => call('getCapabilities'),
        listAudioApps: () => call('listAudioApps'),
        startExternalCapture: (opts) => call('startExternalCapture', opts),
        stopExternalCapture: () => call('stopExternalCapture'),
        captureStatus: () => call('captureStatus'),
        updateAuth: (opts) => call('updateAuth', opts),
        showOverlay: (visible) => call('showOverlay', { visible: !!visible }),
        pushOverlayCaption: (c) => call('pushOverlayCaption', c),
        clearOverlay: () => call('clearOverlay'),
        setOverlayOptions: (o) => call('setOverlayOptions', o),
        permissions: () => call('permissions'),
        requestPermission: (kind) => call('requestPermission', { kind }),
        openSystemSettings: (pane) => call('openSystemSettings', { pane }),
      });
      window.dispatchEvent(new CustomEvent('warptalk-native-ready'));
    })();
    """
}
