// Scriptable stand-in for the macOS host bridge (`window.warptalkNative`, contract: apps/macos/BRIDGE.md).
// Installed with page.addInitScript so it exists before the app boots, exactly like the real host.
import type { Page } from '@playwright/test';

export interface MockApp { bundleId: string; name: string; pid: number; producingAudio: boolean }
export interface MockOptions {
  capabilities?: Record<string, unknown>;
  apps?: MockApp[];
  permissions?: Partial<Record<'microphone' | 'screenRecording' | 'audioCapture', string>>;
  /** Native rejects startExternalCapture with this error (code + message). */
  startError?: { code: string; message: string };
  /** requestPermission() flips the kind to granted. */
  grantOnRequest?: boolean;
}

export interface NativeCall { method: string; args: unknown[] }

export const CHROME: MockApp = { bundleId: 'com.google.Chrome', name: 'Google Chrome', pid: 101, producingAudio: true };
export const ZOOM: MockApp = { bundleId: 'us.zoom.xos', name: 'zoom.us', pid: 202, producingAudio: false };
export const BLACKHOLE = { uid: 'BlackHole2ch_UID', name: 'BlackHole 2ch', isVirtual: true };

export async function installNativeMock(page: Page, opts: MockOptions = {}) {
  await page.addInitScript((o) => {
    const calls: { method: string; args: unknown[] }[] = [];
    const state = {
      caps: {
        version: 1, platform: 'macos', osVersion: '14.5.0', processTap: true, screenCaptureKit: true, voiceMode: true, overlay: true,
        supportedApps: [], outputDevices: [], blackHoleInstalled: false, ...(o.capabilities ?? {}),
      },
      apps: o.apps ?? [{ bundleId: 'com.google.Chrome', name: 'Google Chrome', pid: 101, producingAudio: true }],
      perms: { microphone: 'granted', screenRecording: 'granted', audioCapture: 'granted', ...(o.permissions ?? {}) } as Record<string, string>,
      startError: o.startError ?? null as { code: string; message: string } | null,
      grantOnRequest: !!o.grantOnRequest,
      status: { state: 'idle', roomId: null, appBundleId: null, mode: null, backend: null, rmsDbfs: -120, audioFlowing: false, lastAudioAt: null, lastHeartbeatAt: null, dubTrack: null } as Record<string, unknown>,
    };
    const emit = (detail: Record<string, unknown>) =>
      window.dispatchEvent(new CustomEvent('warptalk-native', { detail: { ts: new Date().toISOString(), ...detail } }));
    const rec = <T,>(method: string, args: unknown[], v: T): Promise<T> => {
      calls.push({ method, args: JSON.parse(JSON.stringify(args)) });
      return Promise.resolve(v);
    };
    const reject = (code: string, message: string) => Promise.reject(Object.assign(new Error(message), { code }));
    (window as unknown as Record<string, unknown>).warptalkNative = {
      version: 1,
      getCapabilities: () => rec('getCapabilities', [], state.caps),
      listAudioApps: () => rec('listAudioApps', [], state.apps),
      startExternalCapture: (opts: Record<string, unknown>) => {
        calls.push({ method: 'startExternalCapture', args: [JSON.parse(JSON.stringify(opts))] });
        if (state.startError) return reject(state.startError.code, state.startError.message);
        state.status = {
          ...state.status, state: 'running', roomId: opts.roomId, appBundleId: opts.appBundleId, mode: opts.mode, backend: 'processTap',
          rmsDbfs: -45, audioFlowing: true, dubTrack: opts.mode === 'voice' ? `dub-${String(opts.listenLanguage)}` : null,
        };
        return Promise.resolve(state.status);
      },
      stopExternalCapture: () => {
        state.status = { ...state.status, state: 'idle', rmsDbfs: -120, audioFlowing: false };
        calls.push({ method: 'stopExternalCapture', args: [] });
        return Promise.resolve(state.status);
      },
      captureStatus: () => rec('captureStatus', [], state.status),
      // Copilot capture (P1 in the real app): recorded only; the web UI turns "Start/Stop capture" on when these exist.
      startCopilotSession: (o: unknown) => rec('startCopilotSession', [o], { ...state.status, state: 'running' }),
      stopCopilotSession: () => rec('stopCopilotSession', [], { ...state.status, state: 'idle' }),
      updateAuth: (a: unknown) => rec('updateAuth', [a], { updated: true }),
      showOverlay: (v: boolean) => rec('showOverlay', [v], { visible: v }),
      pushOverlayCaption: (c: unknown) => rec('pushOverlayCaption', [c], null),
      clearOverlay: () => rec('clearOverlay', [], null),
      setOverlayOptions: (x: { clickThrough?: boolean; hideFromScreenShare?: boolean }) =>
        rec('setOverlayOptions', [x], { clickThrough: !!x.clickThrough, hideFromScreenShare: !!x.hideFromScreenShare }),
      permissions: () => rec('permissions', [], { ...state.perms }),
      requestPermission: (k: string) => {
        if (state.grantOnRequest) state.perms[k] = 'granted';
        return rec('requestPermission', [k], { ...state.perms });
      },
      openSystemSettings: (p: string) => rec('openSystemSettings', [p], null),
    };
    (window as unknown as Record<string, unknown>).__nativeMock = {
      calls,
      emit,
      setApps: (a: unknown[]) => { state.apps = a as typeof state.apps; },
      setPerms: (p: Record<string, string>) => { Object.assign(state.perms, p); },
      setStartError: (e: { code: string; message: string } | null) => { state.startError = e; },
    };
  }, opts);
}

export const nativeCalls = (page: Page, method?: string): Promise<NativeCall[]> =>
  page.evaluate((m) => {
    const all = (window as unknown as { __nativeMock: { calls: { method: string; args: unknown[] }[] } }).__nativeMock.calls;
    return m ? all.filter((c) => c.method === m) : all.slice();
  }, method);

/** Dispatch a `warptalk-native` event exactly as the host does. */
export const emitNative = (page: Page, detail: Record<string, unknown>) =>
  page.evaluate((d) => (window as unknown as { __nativeMock: { emit(x: unknown): void } }).__nativeMock.emit(d), detail);

export const mockSet = {
  apps: (page: Page, apps: MockApp[]) => page.evaluate((a) => (window as unknown as { __nativeMock: { setApps(x: unknown): void } }).__nativeMock.setApps(a), apps),
  perms: (page: Page, p: Record<string, string>) => page.evaluate((a) => (window as unknown as { __nativeMock: { setPerms(x: unknown): void } }).__nativeMock.setPerms(a), p),
  startError: (page: Page, e: { code: string; message: string } | null) => page.evaluate((a) => (window as unknown as { __nativeMock: { setStartError(x: unknown): void } }).__nativeMock.setStartError(a), e),
};
