# WarpTalk native bridge (`window.warptalkNative`)

Contract between the macOS app (WKWebView host) and the web app (`apps/web`). The web app owns auth, rooms, the meeting UI, captions and the LiveKit session for native meetings. The macOS app owns: capturing another app's audio, publishing it to the bridge room, voice-mode dub playback into a virtual device, the floating caption overlay, and macOS permissions.

Status: written before the Swift code could be compiled; implementation is `Sources/Web/*` and `Sources/Features/External/NativeCaptureController.swift`.

## Detecting the bridge
`window.warptalkNative` exists only inside the macOS app, only for the configured web origin (scheme + host + port; default `http://localhost:5173`), only in the main frame. The script is injected at document start. Also fires `window.dispatchEvent(new Event('warptalk-native-ready'))`.
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
  | { type: 'heartbeat'; ts: string; ok: boolean; at?: string; message?: string }
  | { type: 'error'; ts: string; code: string; message: string; fatal: boolean } // fatal => capture already stopped, state 'error' or 'idle'
  | { type: 'auth-required'; ts: string; reason: 'heartbeat' }           // 401: call updateAuth({accessToken})
  | { type: 'log'; ts: string; message: string };                        // informational, e.g. fallback to ScreenCaptureKit

window.addEventListener('warptalk-native', (e) => {
  const ev = (e as CustomEvent<NativeEvent>).detail;
});
```

## Error codes
`FORBIDDEN_ORIGIN`, `INVALID_ARGUMENT`, `UNKNOWN_METHOD`, `ALREADY_RUNNING`, `APP_NOT_RUNNING`, `NO_AUDIO_PROCESS` (app not playing sound yet), `PERMISSION_DENIED`, `CAPTURE_FAILED`, `AUTH_EXPIRED` (401 from backend), `LEASE_LOST` (heartbeat 409: another machine took over the capturer role), `NOT_REQUESTABLE`, `INTERNAL`, plus backend error codes passed through (e.g. `INVALID_STATE`, `FORBIDDEN`).

## Intended flow (web app)
1. `listAudioApps()`; the user picks one (usually Chrome).
2. Web app claims the room: `POST /translation-rooms/bridge/claim` (shape per spec 10 §2.1; it also handles takeover and `audio-mode`). The bridge-token endpoint requires the capturer role, so claim first.
3. `startExternalCapture({ appBundleId, roomId, mode, accessToken, listenLanguage, ... })`.
4. Native: bridge token -> publish captured audio (48 kHz mono) -> heartbeat every `heartbeatIntervalSeconds` with `{ audioFlowing, lastAudioAt, rmsDbfs }`.
5. Web app receives captions through its own LiveKit connection (topic `captions`) and mirrors them: `pushOverlayCaption(...)`. The web app does NOT need to play the dub in external mode: voice mode plays it natively.
6. `stopExternalCapture()` on stop; the web app ends the room via its own API if wanted.

Voice mode needs a native audio path because WKWebView has no `setSinkId`: native opens its own listener connection (join token), subscribes ONLY to `dub-{listenLanguage}`, and plays it to `dubOutputDeviceUid` via AVAudioEngine. Set the meeting app's microphone to that device (BlackHole 2ch).

## Notes for the web agent
- Bridge calls are rejected unless the page origin equals the URL in native Settings. If the dev server moves port, change it there.
- Heartbeat semantics: `audioFlowing` = native capture is alive. A quiet meeting reports `audioFlowing: true` with an old `lastAudioAt`.
- Native meetings run in the web view with browser LiveKit and `getUserMedia`; WKWebView supports WebRTC on macOS 14. Autoplay of remote audio is enabled. Output-device selection (`setSinkId`) is unavailable.
- Playwright: run the web app in Chromium without the bridge and inject a stub before load:
```ts
await page.addInitScript(() => {
  const listeners: any[] = [];
  (window as any).warptalkNative = {
    version: 1,
    getCapabilities: async () => ({ version: 1, platform: 'macos', osVersion: '14.5.0', processTap: true, screenCaptureKit: true, voiceMode: true, overlay: true, supportedApps: [], outputDevices: [], blackHoleInstalled: false }),
    listAudioApps: async () => [{ bundleId: 'com.google.Chrome', name: 'Google Chrome', pid: 1, producingAudio: true }],
    startExternalCapture: async (o: any) => ({ state: 'running', roomId: o.roomId, appBundleId: o.appBundleId, mode: o.mode, backend: 'processTap', rmsDbfs: -30, audioFlowing: true, lastAudioAt: null, lastHeartbeatAt: null, dubTrack: null }),
    stopExternalCapture: async () => ({ state: 'idle' }),
    captureStatus: async () => ({ state: 'idle' }),
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
