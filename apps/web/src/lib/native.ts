// Typed access to the macOS app bridge `window.warptalkNative` (contract: apps/macos/BRIDGE.md).
import { useCallback, useEffect, useRef, useState } from 'react';

export type PermissionState = 'granted' | 'denied' | 'notDetermined' | 'unknown';
export type PermissionKind = 'microphone' | 'screenRecording' | 'audioCapture';
export type SettingsPane = PermissionKind | 'privacy';
export type CaptureMode = 'text' | 'voice';
export type CaptureState = 'idle' | 'starting' | 'running' | 'stopping' | 'error';

export interface Capabilities {
  version: 1;
  platform: 'macos';
  osVersion: string;
  processTap: boolean;
  screenCaptureKit: boolean;
  voiceMode: boolean;
  overlay: boolean;
  overlayWindow?: boolean;
  supportedApps: { name: string; bundleIds: string[] }[];
  outputDevices: { uid: string; name: string; isVirtual: boolean }[];
  blackHoleInstalled: boolean;
}
export interface AudioApp { bundleId: string; name: string; pid: number; producingAudio: boolean }
export interface StartExternalCaptureOptions {
  appBundleId: string;
  roomId: string;
  mode: CaptureMode;
  accessToken: string;
  backendUrl?: string;
  heartbeatIntervalSeconds?: number;
  listenLanguage?: string;
  dubOutputDeviceUid?: string;
  captureBackend?: 'auto' | 'processTap' | 'screenCaptureKit';
}
export interface CaptureStatus {
  state: CaptureState;
  roomId?: string | null;
  appBundleId?: string | null;
  mode?: CaptureMode | null;
  backend?: 'processTap' | 'screenCaptureKit' | null;
  rmsDbfs?: number;
  audioFlowing?: boolean;
  lastAudioAt?: string | null;
  lastHeartbeatAt?: string | null;
  dubTrack?: string | null;
  error?: { code: string; message: string };
}
export interface OverlayCaption { speaker?: string; text: string; translation?: string; segmentId?: string; isFinal?: boolean }
export type Permissions = Record<PermissionKind, PermissionState>;

export interface WarptalkNative {
  version: 1;
  getCapabilities(): Promise<Capabilities>;
  listAudioApps(): Promise<AudioApp[]>;
  startExternalCapture(o: StartExternalCaptureOptions): Promise<CaptureStatus>;
  stopExternalCapture(): Promise<CaptureStatus>;
  captureStatus(): Promise<CaptureStatus>;
  updateAuth(o: { accessToken: string }): Promise<{ updated: true }>;
  showOverlay(visible: boolean): Promise<{ visible: boolean }>;
  pushOverlayCaption(c: OverlayCaption): Promise<null>;
  clearOverlay(): Promise<null>;
  setOverlayOptions(o: { clickThrough?: boolean; hideFromScreenShare?: boolean }): Promise<{ clickThrough: boolean; hideFromScreenShare: boolean }>;
  /** Copilot window (macOS builds with `capabilities.overlayWindow`). */
  openOverlayWindow?(): Promise<{ visible: boolean }>;
  closeOverlayWindow?(): Promise<{ visible: boolean }>;
  pushOverlayState?(state: OverlayState): Promise<null>;
  overlayCommand?(command: OverlayCommand, value?: string): Promise<null>;
  permissions(): Promise<Permissions>;
  requestPermission(k: PermissionKind): Promise<Permissions>;
  openSystemSettings(p: SettingsPane): Promise<null>;
}

/** State the main page relays to the copilot window (`/overlay`), which renders only this. */
export interface OverlayLine {
  id: string; speaker: string; self: boolean; text: string; translation?: string; lang?: string; final: boolean; at: number;
}
export interface OverlayState {
  title: string;
  status: 'connecting' | 'ready' | 'translating' | 'stopped' | 'ended';
  myLang: string;
  farLang: string;
  micOn: boolean;
  lines: OverlayLine[];
  suggestions: { id: string; category: string; content: string; detail?: string }[];
  followUps: { id: string; text: string; owner?: string }[];
  suggestionsAvailable: boolean;
  error?: string;
}
/** `expand` / `collapse` / `hide` / `resize` (value `WxH`) / `hitrect` (value `x,y,w,h`) only affect the native window and never reach the main page. */
export type OverlayCommand = 'start' | 'stop' | 'listen' | 'far' | 'mic' | 'end' | 'closed' | 'ready' | 'expand' | 'collapse' | 'hide' | 'resize' | 'hitrect';

export type NativeEvent =
  | ({ type: 'status'; ts: string } & CaptureStatus)
  | { type: 'level'; ts: string; rmsDbfs: number; audioFlowing: boolean }
  | { type: 'audioFlowing'; ts: string; audioFlowing: boolean; lastAudioAt: string | null }
  | { type: 'heartbeat'; ts: string; ok: boolean; at?: string; message?: string }
  | { type: 'error'; ts: string; code: string; message: string; fatal: boolean }
  | { type: 'auth-required'; ts: string; reason: 'heartbeat' }
  | { type: 'log'; ts: string; message: string }
  | { type: 'overlay-command'; ts: string; command: OverlayCommand; value?: string };

export function getNative(): WarptalkNative | undefined {
  if (typeof window === 'undefined') return undefined;
  return (window as unknown as { warptalkNative?: WarptalkNative }).warptalkNative;
}

/** Re-renders once when the host announces itself after first paint (`warptalk-native-ready`). */
export function useNative(): WarptalkNative | undefined {
  const [native, setNative] = useState(getNative);
  useEffect(() => {
    const on = () => setNative(getNative());
    on();
    window.addEventListener('warptalk-native-ready', on);
    return () => window.removeEventListener('warptalk-native-ready', on);
  }, []);
  return native;
}

export function nativeErrorCode(e: unknown): string {
  const c = (e as { code?: unknown } | null)?.code;
  return typeof c === 'string' ? c : 'INTERNAL';
}
export function nativeErrorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  const m = (e as { message?: unknown } | null)?.message;
  return typeof m === 'string' ? m : 'Native call failed';
}

export interface NativeError { code: string; message: string; fatal: boolean; ts: string }
export interface HeartbeatInfo { ok: boolean; at?: string; message?: string; ts: string }

export interface CaptureLive {
  status: CaptureStatus;
  /** Last RMS in dBFS from `level` events (-120 = silent). */
  level: number;
  audioFlowing: boolean;
  lastAudioAt: string | null;
  heartbeat: HeartbeatInfo | null;
  errors: NativeError[];
  logs: string[];
  authRequired: boolean;
}

const INITIAL: CaptureLive = {
  status: { state: 'idle' }, level: -120, audioFlowing: false, lastAudioAt: null, heartbeat: null, errors: [], logs: [], authRequired: false,
};

/** Mirrors native capture state from `captureStatus()` and the `warptalk-native` events. */
export function useCaptureLive(native: WarptalkNative | undefined) {
  const [live, setLive] = useState<CaptureLive>(INITIAL);
  const alive = useRef(true);

  const apply = useCallback((s: CaptureStatus) => {
    setLive((l) => ({
      ...l,
      status: s,
      level: s.state === 'running' ? (s.rmsDbfs ?? l.level) : -120,
      audioFlowing: s.state === 'running' ? (s.audioFlowing ?? false) : false,
      lastAudioAt: s.lastAudioAt ?? l.lastAudioAt,
      heartbeat: s.lastHeartbeatAt ? { ok: true, at: s.lastHeartbeatAt, ts: s.lastHeartbeatAt } : l.heartbeat,
      errors: s.error
        ? [{ ...s.error, fatal: s.state === 'error', ts: new Date().toISOString() }, ...l.errors.filter((e) => e.code !== s.error?.code)].slice(0, 5)
        : l.errors,
    }));
  }, []);

  useEffect(() => {
    alive.current = true;
    if (!native) return;
    native.captureStatus().then((s) => { if (alive.current) apply(s); }).catch(() => undefined);
    const onEvent = (e: Event) => {
      const d = (e as CustomEvent<NativeEvent>).detail;
      if (!d) return;
      switch (d.type) {
        case 'status': apply(d); break;
        case 'level': setLive((l) => ({ ...l, level: d.rmsDbfs, audioFlowing: d.audioFlowing })); break;
        case 'audioFlowing': setLive((l) => ({ ...l, audioFlowing: d.audioFlowing, lastAudioAt: d.lastAudioAt ?? l.lastAudioAt })); break;
        case 'heartbeat': setLive((l) => ({ ...l, heartbeat: { ok: d.ok, at: d.at, message: d.message, ts: d.ts } })); break;
        case 'error': setLive((l) => ({ ...l, errors: [{ code: d.code, message: d.message, fatal: d.fatal, ts: d.ts }, ...l.errors.filter((x) => x.code !== d.code)].slice(0, 5) })); break;
        case 'auth-required': setLive((l) => ({ ...l, authRequired: true })); break;
        case 'log': setLive((l) => ({ ...l, logs: [d.message, ...l.logs].slice(0, 4) })); break;
      }
    };
    window.addEventListener('warptalk-native', onEvent);
    return () => { alive.current = false; window.removeEventListener('warptalk-native', onEvent); };
  }, [native, apply]);

  const dismissError = useCallback((code: string) => setLive((l) => ({ ...l, errors: l.errors.filter((e) => e.code !== code) })), []);
  const pushError = useCallback((e: { code: string; message: string; fatal?: boolean }) =>
    setLive((l) => ({ ...l, errors: [{ code: e.code, message: e.message, fatal: !!e.fatal, ts: new Date().toISOString() }, ...l.errors.filter((x) => x.code !== e.code)].slice(0, 5) })), []);
  const clearAuthRequired = useCallback(() => setLive((l) => ({ ...l, authRequired: false })), []);
  return { live, apply, dismissError, clearAuthRequired, pushError };
}
