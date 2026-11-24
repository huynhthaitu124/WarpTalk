# WarpTalk native bridge (`window.warptalkNative`)

Contract between the macOS app (WKWebView host) and the web app (`apps/web`). The web app owns auth, rooms, the meeting UI, captions and the LiveKit session for native meetings. The macOS app owns: capturing another app's audio, publishing it to the bridge room, voice-mode dub playback into a virtual device, the floating caption overlay, and macOS permissions.

Status: the Swift code (including `Sources/Features/Copilot`) has not been compiled on a Mac yet (written on Linux; LiveKit and Apple API names were checked against the real SDK sources, see README §8). Implementation: `Sources/Web/*` and `Sources/Features/External/NativeCaptureController.swift`.

## Detecting the bridge
`window.warptalkNative` exists only inside the macOS app, only for the ACTIVE web origin (scheme + host + port), only in the main frame. The active origin is the first URL that answers from: Settings URL (default `http://localhost:3000`, the Next.js dev server) -> fallback list (default `http://localhost:5173`) -> the bundled export `warptalk://app/` (when the app was built with `make bundle`). The script is injected at document start. Also fires `window.dispatchEvent(new Event('warptalk-native-ready'))`.
```ts
const native = (window as any).warptalkNative as WarptalkNative | undefined; // undefined in a normal browser
```
Every method returns a Promise. On failure the Promise rejects with an `Error` that has a string `code` (see Error codes). Tests in plain Chromium/Playwright can stub the object (see the end).

## TypeScript definitions
```ts
export type PermissionState = 'granted' | 'denied' | 'notDetermined' | 'unknown';
export type PermissionKind = 'microphone' | 'screenRecording' | 'audioCapture';
export type SettingsPane = PermissionKind | 'privacy';
export type CaptureMode = 'text' | 'voice';
export type CaptureBackend = 'auto' | 'processTap' | 'screenCaptureKit';

/** Runtime configuration injected by the host (read it instead of hard-coding the backend). */
export interface NativeConfig {
  version: 1;
  /** Backend API base WITHOUT `/api/v1`. Bundled app: "warptalk://app" (same-origin proxy, forwards `/api/**` to the backend URL in
   *  native Settings, so the backend needs no CORS entry). Dev server: the backend URL from native Settings (e.g. "http://localhost:8080"). */
  apiBaseUrl: string;
  /** true when the page is served from the app bundle (`warptalk://app`). */
  bundled: boolean;
}

export interface Capabilities {
  version: 1;
  platform: 'macos';
  osVersion: string;                 // "14.5.0"
  processTap: boolean;               // macOS >= 14.2
  screenCaptureKit: boolean;
  voiceMode: boolean;
  overlay: boolean;
  supportedApps: { name: string; bundleIds: string[] }[];
  outputDevices: { uid: string; name: string; isVirtual: boolean }[];
  blackHoleInstalled: boolean;
  copilot: boolean;                  // copilot engine linked (built with `make core`)
  inputDevices: { uid: string; name: string }[];
}

export interface AudioApp {
  bundleId: string;                  // "com.google.Chrome", "us.zoom.xos", "com.microsoft.teams2", "com.apple.Safari"
  name: string;
  pid: number;
  producingAudio: boolean;           // an audio process of the app is currently outputting sound
}

export interface StartExternalCaptureOptions {
  appBundleId: string;               // from listAudioApps()
  roomId: string;                    // the bridge room the WEB app already claimed (POST /translation-rooms/bridge/claim)
  mode: CaptureMode;                 // default 'text'
  accessToken: string;               // web app's JWT; native calls bridge-token, join (voice) and heartbeat with it
  backendUrl?: string;               // e.g. "http://localhost:8080" (no /api/v1); falls back to the native Settings value
  heartbeatIntervalSeconds?: number; // default 15 (from the claim response)
  listenLanguage?: string;           // REQUIRED when mode === 'voice': subscribes to track "dub-{listenLanguage}"
  nativeOverlayCaptions?: boolean;   // voice mode, default false: native also decodes its own `captions` packets and drives the overlay (do NOT also call pushOverlayCaption)
  dubOutputDeviceUid?: string;       // voice mode output device (BlackHole 2ch); defaults to native Settings value
  captureBackend?: CaptureBackend;   // default 'auto' = Process Tap, fall back to ScreenCaptureKit
}

export type CaptureState = 'idle' | 'starting' | 'running' | 'stopping' | 'error';

export interface CaptureStatus {
  state: CaptureState;
  roomId: string | null;
  appBundleId: string | null;
  mode: CaptureMode | null;
  backend: 'processTap' | 'screenCaptureKit' | null;
  rmsDbfs: number;                   // -120 when silent / not capturing
  audioFlowing: boolean;             // capture pipeline is delivering buffers (liveness, NOT "someone talks")
  lastAudioAt: string | null;        // ISO time the level last exceeded -60 dBFS
  lastHeartbeatAt: string | null;
  leaseExpiresAt: string | null;     // server-computed end of the capturer lease from the last heartbeat (null before the first one)
  dubTrack: string | null;           // voice mode: subscribed track name, e.g. "dub-vi"
  error?: { code: string; message: string };
}

export interface OverlayCaption {
  speaker?: string;                  // display name (web resolves speakerName / roster); never the pipeline agent id
  text: string;                      // original
  translation?: string;              // in the viewer's language; shown large when present
  segmentId?: string;                // same id updates the line in place (interim -> final)
  isFinal?: boolean;                 // default true; a late interim never overwrites a final line
}

export interface WarptalkNative {
  version: 1;
  config: NativeConfig;
  getCapabilities(): Promise<Capabilities>;
  listAudioApps(): Promise<AudioApp[]>;
  startExternalCapture(opts: StartExternalCaptureOptions): Promise<CaptureStatus>;
  stopExternalCapture(): Promise<CaptureStatus>;
  captureStatus(): Promise<CaptureStatus>;
  /** Push a refreshed JWT while capture runs (answer to the 'auth-required' event). */
  updateAuth(opts: { accessToken: string }): Promise<{ updated: true }>;
  showOverlay(visible: boolean): Promise<{ visible: boolean }>;
  pushOverlayCaption(caption: OverlayCaption): Promise<null>;
  clearOverlay(): Promise<null>;
  setOverlayOptions(o: { clickThrough?: boolean; hideFromScreenShare?: boolean }): Promise<{ clickThrough: boolean; hideFromScreenShare: boolean }>;
  permissions(): Promise<Record<PermissionKind, PermissionState>>;
  /** 'microphone' and 'screenRecording' only. 'audioCapture' rejects NOT_REQUESTABLE (the OS prompt appears on first capture). */
  requestPermission(kind: PermissionKind): Promise<Record<PermissionKind, PermissionState>>;
  openSystemSettings(pane: SettingsPane): Promise<null>;
}
```
`updateAuth`, `clearOverlay` and `setOverlayOptions` are additions to the originally listed set.

## Events
Native pushes `window.dispatchEvent(new CustomEvent('warptalk-native', { detail }))`. Every `detail` has `type` and `ts` (ISO time).
```ts
export type NativeEvent =
  | ({ type: 'status'; ts: string } & CaptureStatus)                     // every state change
  | { type: 'level'; ts: string; rmsDbfs: number; audioFlowing: boolean } // ~5 Hz while running
  | { type: 'audioFlowing'; ts: string; audioFlowing: boolean; lastAudioAt: string | null } // on change only
  | { type: 'heartbeat'; ts: string; ok: boolean; at?: string; message?: string;
      leaseExpiresAt?: string; serverTime?: string; leaseRemainingSeconds?: number }  // lease fields from the server (judge the lease with serverTime, not the Mac clock)
  | { type: 'error'; ts: string; code: string; message: string; fatal: boolean } // fatal => capture already stopped, state 'error' or 'idle'
  | { type: 'auth-required'; ts: string; reason: 'heartbeat' | 'copilot' }  // 401: call updateAuth({accessToken})
  | CopilotNativeEvent                                                    // see Copilot sessions
  | { type: 'log'; ts: string; message: string };                        // informational, e.g. fallback to ScreenCaptureKit

window.addEventListener('warptalk-native', (e) => {
  const ev = (e as CustomEvent<NativeEvent>).detail;
});
```

## Copilot sessions (P1, ADR 0001)

WarpTalk is a meeting assistant copilot (transparent: the overlay is NOT hidden from screen share by default). The web app creates a session (`POST /copilot/sessions`, room type `COPILOT_SESSION`) and asks native to capture audio for it. Native runs the on-device Rust engine (`crates/pipeline/copilot-core` through UniFFI): microphone = channel `me` ("Tôi"), meeting-app audio (Process Tap, fallback ScreenCaptureKit) = channel `others` ("Người khác"). The engine does VAD/endpointing + streaming STT (token from `POST /copilot/sessions/{id}/stt-token`) and posts text segments to `POST /copilot/sessions/{id}/segments` itself; the web app only has to start/stop, refresh the token, and render the `copilot.*` events + suggestions.

Implementation: `Sources/Features/Copilot/*`. The web UI shows "Start capture" when `typeof native.startCopilotSession === 'function'`; check `capabilities.copilot` (false = app built without `make core`: start rejects `ENGINE_UNAVAILABLE`).

```ts
export type CopilotChannel = 'me' | 'others';
export type CopilotStatusKind = 'started' | 'stopped' | 'speechStart' | 'speechEnd' | 'sttConnected' | 'sttUnavailable'
  | 'sinkOnline' | 'sinkOffline' | 'audioDropped';

export interface StartCopilotSessionOptions {
  sessionId: string;                 // id from POST /copilot/sessions
  accessToken: string;               // web JWT; refresh with updateAuth() (also forwarded to the engine)
  apiBaseUrl: string;                // backend origin WITHOUT /api/v1. A non-http(s) value (bundled "warptalk://app") falls back to the native Settings backend URL
  appBundleId?: string;              // meeting app for the `others` channel; default = native Settings (Chrome). '' / omitted => Settings value
  micDeviceUid?: string;             // input device uid (capabilities.inputDevices); default = Settings, '' = system default
  languages?: string[];              // STT language hint (first entry is used), e.g. ['vi','en']
  speakerNames?: { me?: string; others?: string }; // overlay labels, default "Tôi" / "Người khác"
}

/** CaptureStatus (above) plus copilot fields. mode/dubTrack/lastHeartbeatAt/leaseExpiresAt are always null. */
export interface CopilotStatus extends Omit<CaptureStatus, 'mode'> {
  mode: null;
  sessionId: string | null;          // == roomId
  channels: { me: 'running' | 'off'; others: 'running' | 'waiting' | 'off' };
  // others 'waiting': meeting app not running / not playing audio yet; native retries every 5 s and emits copilot.error once.
}

export interface OverlaySuggestionInput {
  id: string;                        // same id updates in place
  category: string;                  // free text, shown as a chip ("question", "risk", ...)
  content: string;
  followUp?: string | boolean;       // string: follow-up question text under the content; true: this item IS a follow-up
}

interface WarptalkNative {
  capabilities: ...                  // getCapabilities() additionally returns { copilot: boolean; inputDevices: {uid:string;name:string}[] }
  /** Starts mic + meeting-app capture and the engine. Resolves with CopilotStatus (state 'running') once the engine started and the
   *  mic runs; the `others` channel may still be 'waiting'. Shows the overlay in copilot mode (unless Settings disable auto-show). */
  startCopilotSession?(o: StartCopilotSessionOptions): Promise<CopilotStatus>;
  /** Stops capture, flushes pending segments to the backend (can take seconds), resolves with state 'idle'. The backend finalizes the
   *  transcript (`__MEETING_END__`) on its own side. Safe to call when idle. */
  stopCopilotSession?(): Promise<CopilotStatus>;
  copilotStatus?(): Promise<CopilotStatus>;
  /** Suggestions / follow-ups (e.g. from GET /meetings/{id}/suggestions/stream; use the direct backend URL, SSE is not proxied). */
  pushOverlaySuggestion?(s: OverlaySuggestionInput): Promise<null>;
  clearOverlaySuggestion?(id: string): Promise<null>;
}

export type CopilotNativeEvent =
  | { type: 'copilot.partial'; ts: string; channel: CopilotChannel; text: string }                 // live text of the current utterance
  | { type: 'copilot.segment'; ts: string; clientSegmentId: string; channel: CopilotChannel; text: string; language: string;
      startMs: number; endMs: number; confidence: number | null; isFinal: boolean; likelyEcho: boolean; utteranceSeq: number }
  | { type: 'copilot.status'; ts: string; channel: CopilotChannel | null; kind: CopilotStatusKind; detail: string | null }
  | { type: 'copilot.error'; ts: string; channel: CopilotChannel | null; code: string; message: string; fatal?: boolean }
  | { type: 'copilot.levels'; ts: string; channel: CopilotChannel; rms: number };                  // throttled by the core, linear RMS 0..1
```

- Events arrive on `window` `warptalk-native` like the others; `status` events (state changes, `CopilotStatus` shape) and `auth-required` (now `reason: 'heartbeat' | 'copilot'`: the engine got a 401, call `updateAuth({accessToken})`) are shared with external capture.
- `copilot.segment` is a transcript line; the same `clientSegmentId` can arrive again (early sentence -> final): upsert by it. `likelyEcho: true` = a `me` utterance dominated by `others` (speaker bleed): the overlay hides it, the web UI should de-emphasise or hide it. Times are ms offsets from session start. The backend already receives every segment from the engine: the web app must NOT post them again.
- `copilot.error` with `fatal: true` means start failed (`state: 'error'`, capture already torn down); other errors are non-fatal (engine STT hiccup, `others` capture waiting: `APP_NOT_RUNNING` / `NO_AUDIO_PROCESS`).
- `updateAuth` refreshes the bearer for both external capture and the copilot engine. `clearOverlay()` also clears copilot lines and suggestions.
- One capture at a time per feature: `startCopilotSession` while a copilot session runs rejects `ALREADY_RUNNING`. External capture and copilot are independent controllers (do not run both on the same app).
- Error codes (rejections of start): `ALREADY_RUNNING`, `INVALID_ARGUMENT`, `PERMISSION_DENIED` (microphone), `ENGINE_UNAVAILABLE`, `CAPTURE_FAILED`, `AUTH_EXPIRED`.
- Native Settings (not part of the bridge): default meeting app (Chrome/Zoom/Teams/Safari), microphone, overlay position, auto-show overlay. Hidden `copilotFakeStt` default (`defaults write <bundle id> copilotFakeStt -bool YES`) uses the core's fake STT for demos.

## Error codes
`FORBIDDEN_ORIGIN`, `INVALID_ARGUMENT`, `UNKNOWN_METHOD`, `ALREADY_RUNNING`, `APP_NOT_RUNNING`, `NO_AUDIO_PROCESS` (app not playing sound yet), `PERMISSION_DENIED`, `CAPTURE_FAILED`, `AUTH_EXPIRED` (401 from backend), `LEASE_LOST` (heartbeat 409 with backend code `CAPTURER_LEASE_LOST`: another machine took over the capturer role), `NOT_REQUESTABLE`, `DUB_LISTENER_DISCONNECTED` (non-fatal event: the voice listener connection dropped), `INTERNAL`, plus backend error codes passed through unchanged (e.g. `INVALID_STATE`, `NOT_BRIDGE_ROOM` on a heartbeat 409 that is NOT a lost lease, `INVALID_CLIENT_TAG`, `FORBIDDEN`).

## Intended flow (web app)
1. `listAudioApps()`; the user picks one (usually Chrome).
2. Web app claims the room: `POST /translation-rooms/bridge/claim` (shape per spec 10 §2.1; it also handles takeover and `audio-mode`). The bridge-token endpoint requires the capturer role, so claim first.
3. `startExternalCapture({ appBundleId, roomId, mode, accessToken, listenLanguage, ... })`.
4. Native: bridge token (publish-only, stand-in identity) -> publish captured audio (48 kHz mono) -> heartbeat every `heartbeatIntervalSeconds` with body `{ audioFlowing, lastAudioAt, rmsDbfs }` (`POST /translation-rooms/{id}/bridge/capturer/heartbeat`; the response carries `leaseExpiresAt` + `serverTime`, surfaced in the `heartbeat` event).
5. Web app receives captions through its own LiveKit connection (topic `captions`) and mirrors them: `pushOverlayCaption(...)`. The web app does NOT need to play the dub in external mode: voice mode plays it natively.
6. `stopExternalCapture()` on stop: native tears down, then calls `POST /translation-rooms/{id}/bridge/capturer/release` (best effort, errors only logged; skipped after `LEASE_LOST`). Calling release from the web as well is harmless. The web app ends the room via its own API if wanted.

Voice mode needs a native audio path because WKWebView has no `setSinkId`: native opens its own listener connection (`POST /meetings/{id}/join` with `{ "clientTag": "bridge-listener" }` => LiveKit identity `{userId}:bridge-listener`, subscribe-only, so it never evicts the web client of the same user), subscribes ONLY to the track named `dub-{listenLanguage}`, and plays it to `dubOutputDeviceUid` via AVAudioEngine. Set the meeting app's microphone to that device (BlackHole 2ch).

### Captions JSON (topic `captions`, produced by `apps/pipeline/src/downstream.rs`)
Only decoded natively with `nativeOverlayCaptions: true`; the web parser is `apps/web/src/lib/captions.ts`. Fields: `type` (`stt` | `translation`), `segment_id` (shared by the `stt` message and all translation messages of a segment: one overlay line), `speakerId`, `speakerName` (nullable; a UUID id is never shown as a name), `speakerUserId`, `text` (cumulative source text), `lang`, `translations` (CUMULATIVE per target language: replace, never append), `sentenceIndex`, `isLastSentence`, `sentenceText`, `sentenceTranslations`, `is_early` (always false: text is final), `is_flash` (flash-mode sentence, still final), `emittedAtMs`. Swift: `Sources/API/CaptionPacket.swift`, fixtures in `Tests/CaptionPacketTests.swift`.

## Notes for the web agent
- Bridge calls are rejected unless the page origin equals the ACTIVE web URL (see top). If the dev server moves port, change it in native Settings (or add it to the fallback list).
- Use `window.warptalkNative.config.apiBaseUrl` as the API base when present (bundled app: same-origin proxy `warptalk://app/api/**` -> backend; no CORS). The static export must not assume it is served from `/` of a real host: the bundled app serves `index.html`, `<route>.html`, `<route>/index.html`, `_next/**`, and falls back to `index.html` for unknown extensionless paths. Server-Sent Events/streaming responses are NOT proxied (responses are buffered); use the direct backend URL for SSE.
- `getUserMedia`/WebRTC under `warptalk://app` depends on WebKit treating the scheme as a secure context (unverified, README §8); if `navigator.mediaDevices` is undefined, run against `http://localhost:3000` instead.
- Heartbeat semantics: `audioFlowing` = native capture is alive. A quiet meeting reports `audioFlowing: true` with an old `lastAudioAt`.
- Native meetings run in the web view with browser LiveKit and `getUserMedia`; WKWebView supports WebRTC on macOS 14. Autoplay of remote audio is enabled. Output-device selection (`setSinkId`) is unavailable.
- Playwright: run the web app in Chromium without the bridge and inject a stub before load:
```ts
await page.addInitScript(() => {
  const listeners: any[] = [];
  (window as any).warptalkNative = {
    version: 1,
    config: { version: 1, apiBaseUrl: 'http://localhost:8080', bundled: false },
    getCapabilities: async () => ({ version: 1, platform: 'macos', osVersion: '14.5.0', processTap: true, screenCaptureKit: true, voiceMode: true, overlay: true, supportedApps: [], outputDevices: [], blackHoleInstalled: false }),
    listAudioApps: async () => [{ bundleId: 'com.google.Chrome', name: 'Google Chrome', pid: 1, producingAudio: true }],
    startExternalCapture: async (o: any) => ({ state: 'running', roomId: o.roomId, appBundleId: o.appBundleId, mode: o.mode, backend: 'processTap', rmsDbfs: -30, audioFlowing: true, lastAudioAt: null, lastHeartbeatAt: null, leaseExpiresAt: null, dubTrack: null }),
    stopExternalCapture: async () => ({ state: 'idle' }),
    captureStatus: async () => ({ state: 'idle' }),
    // optional copilot methods (omit to test the browser-only path)
    startCopilotSession: async (o: any) => ({ state: 'running', roomId: o.sessionId, sessionId: o.sessionId, channels: { me: 'running', others: 'running' }, mode: null, rmsDbfs: -30, audioFlowing: true }),
    stopCopilotSession: async () => ({ state: 'idle' }),
    copilotStatus: async () => ({ state: 'idle' }),
    pushOverlaySuggestion: async () => null, clearOverlaySuggestion: async () => null,
    updateAuth: async () => ({ updated: true }),
    showOverlay: async (v: boolean) => ({ visible: v }),
    pushOverlayCaption: async () => null, clearOverlay: async () => null,
    setOverlayOptions: async (o: any) => ({ clickThrough: !!o.clickThrough, hideFromScreenShare: !!o.hideFromScreenShare }),
    permissions: async () => ({ microphone: 'granted', screenRecording: 'granted', audioCapture: 'granted' }),
    requestPermission: async () => ({ microphone: 'granted', screenRecording: 'granted', audioCapture: 'granted' }),
    openSystemSettings: async () => null,
  };
});
// emit events in a test:
await page.evaluate(() => window.dispatchEvent(new CustomEvent('warptalk-native', { detail: { type: 'level', ts: new Date().toISOString(), rmsDbfs: -20, audioFlowing: true } })));
```
