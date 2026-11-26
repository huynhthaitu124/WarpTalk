import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { Broadcast as Cast, Microphone as Mic, Play, ArrowsClockwise as RefreshCw, Stop as Square, Users } from '@phosphor-icons/react';
import {
  Alert, Badge, Button, EmptyState, Field, Input, PageHeader, Select, StatusPill, Switch, useToast,
} from '@v2/ui';
import { ApiError } from '@v2/lib/api';
import { useAuth, useProfile } from '@v2/lib/auth';
import { useWorkspace } from '@v2/lib/workspace';
import { useEndMeeting, useMeetingJoin, useRoom, useUpdateMyLanguages } from '@v2/lib/queries';
import { sendTestLine, useFollowUps, useSuggestionStream, useSuggestionTestMode } from '@v2/lib/suggestions';
import { languageLabel } from '@v2/lib/languages';
import { languageOptions } from '@v2/lib/languageOptions';
import {
  PROVIDER_LABEL, freshAccessToken, leaseIsStale, leaseRemainingSeconds, parseMeetingLink, useBridgeAudioMode, useBridgeRelease, useBridgeTakeover, useClaimBridge, useStartBridgeRoom,
} from '@v2/lib/bridge';
import {
  nativeErrorCode, nativeErrorMessage, useCaptureLive, useNative,
  type AudioApp, type Capabilities, type CaptureMode, type NativeError, type NativeEvent, type OverlayAsk, type OverlayLine, type OverlayState, type PermissionKind, type Permissions,
} from '@v2/lib/native';
import { mergeCaption, pickTranslation, type CaptionLine } from '@v2/lib/captions';
import { session } from '@v2/lib/session';
import type { JoinGrant } from '@v2/lib/schemas';
import { CaptionsPanel, StatusStrip, type ConnectionLabel } from './MeetingParts';
import {
  AppRow, BrowserFallback, ErrorRow, KeyValue, LevelMeter, PermissionsPanel, StepCard, ago, useNow, type Recovery,
} from './ExternalParts';
import { errCode, errMsg } from './errors';
import { historyToMessages } from './meeting/AskPanel';
import { ASK_MAX_CHARS, useAskHistory, useAssistant } from '@v2/lib/ask';

const ExternalCaptions = lazy(() => import('./ExternalCaptions'));

const LANG_OPTIONS = languageOptions();
const LIVE = ['IN_PROGRESS', 'PAUSED'];
const TERMINAL = ['ENDED', 'CANCELLED', 'EXPIRED', 'FAILED'];
const BROWSER_RE = /chrome|safari|firefox|edge|brave|arc|opera|vivaldi|browser/i;

/** The running audio app that most likely plays this provider's call (Teams app, a browser for Meet, else any audible app). */
function pickAppFor(apps: AudioApp[], provider: string | undefined): AudioApp | undefined {
  const audible = (list: AudioApp[]) => list.find((a) => a.producingAudio) ?? list[0];
  if (provider === 'teams') {
    const m = apps.filter((a) => /teams/i.test(a.bundleId) || /teams/i.test(a.name));
    if (m.length) return audible(m);
  } else if (provider === 'google_meet') {
    const m = apps.filter((a) => BROWSER_RE.test(a.bundleId) || BROWSER_RE.test(a.name));
    if (m.length) return audible(m);
  } else if (provider === 'zoom') {
    const m = apps.filter((a) => /zoom/i.test(a.bundleId) || /zoom/i.test(a.name));
    if (m.length) return audible(m);
  }
  return apps.find((a) => a.producingAudio);
}

type CaptionsState = 'idle' | 'connecting' | 'connected' | 'unavailable' | 'error';

export function ExternalPage() {
  const native = useNative();
  const { userId } = useAuth();
  const { current } = useWorkspace();
  const profile = useProfile();
  const toast = useToast();
  const now = useNow();

  // ---- form
  const [params] = useSearchParams();
  const auto = params.get('auto') === '1';
  const [link, setLink] = useState(() => params.get('link') ?? '');
  const [title, setTitle] = useState(() => params.get('title') ?? '');
  const [far, setFar] = useState(() => params.get('far') || 'en');
  const [listen, setListen] = useState(() => params.get('listen') || 'vi');
  /** Copilot: the user speaks in the call too, so their mic is published into the room. */
  const copilot = auto;
  const [micOn, setMicOn] = useState(true);
  const [mode, setMode] = useState<CaptureMode>('text');
  const [appId, setAppId] = useState('');
  const [dubDevice, setDubDevice] = useState('');
  const parsed = useMemo(() => parseMeetingLink(link), [link]);
  const sameLang = far === listen;

  // ---- native
  const { live, apply, dismissError, clearAuthRequired, pushError } = useCaptureLive(native);
  const [caps, setCaps] = useState<Capabilities | null>(null);
  const [apps, setApps] = useState<AudioApp[]>([]);
  const [appsBusy, setAppsBusy] = useState(false);
  const [perms, setPerms] = useState<Permissions | null>(null);
  const [permsBusy, setPermsBusy] = useState(false);

  // ---- room / session
  const [roomId, setRoomId] = useState<string | null>(null);
  const [claimInfo, setClaimInfo] = useState<{ created: boolean; heartbeatIntervalSeconds: number; leaseSeconds: number } | null>(null);
  const room = useRoom(roomId ?? undefined);
  const claim = useClaimBridge();
  const takeover = useBridgeTakeover(roomId ?? undefined);
  const release = useBridgeRelease(roomId ?? undefined);
  const audioMode = useBridgeAudioMode(roomId ?? undefined);
  const startRoom = useStartBridgeRoom(roomId ?? undefined);
  const [busy, setBusy] = useState<'start' | 'stop' | null>(null);
  const [hasRun, setHasRun] = useState(false);
  const endMeeting = useEndMeeting(roomId ?? '');
  const [actionError, setActionError] = useState<{ code?: string; message: string } | null>(null);

  const r = room.data;
  const bridge = r?.bridge;
  const isCapturer = !!bridge && bridge.capturerUserId === userId;
  /** After a release nobody holds the lease: this machine may capture again (it takes the role first). */
  const canCapture = isCapturer || (!!bridge && !bridge.capturerUserId);
  const isHost = !!r && (r.effectiveHostId === userId || r.hostId === userId);
  const roomLive = !!r && LIVE.includes(r.status);
  const roomTerminal = !!r && TERMINAL.includes(r.status);
  const running = live.status.state === 'running' || live.status.state === 'starting';
  const leaseLeft = leaseRemainingSeconds(bridge, room.dataUpdatedAt, now);
  const stale = !!bridge && leaseIsStale(bridge, room.dataUpdatedAt, now);

  const voiceReady = !!caps?.voiceMode && (caps?.outputDevices.length ?? 0) > 0;
  const outputDevices = useMemo(() => caps?.outputDevices ?? [], [caps]);

  // Capabilities + apps + permissions once the host is there.
  const refreshApps = useCallback(async () => {
    if (!native) return;
    setAppsBusy(true);
    try { setApps(await native.listAudioApps()); } catch (e) { setActionError({ code: nativeErrorCode(e), message: nativeErrorMessage(e) }); } finally { setAppsBusy(false); }
  }, [native]);
  const refreshPerms = useCallback(async () => {
    if (!native) return;
    setPermsBusy(true);
    try { setPerms(await native.permissions()); } catch { /* shown as "checking" */ } finally { setPermsBusy(false); }
  }, [native]);

  useEffect(() => {
    if (!native) return;
    void native.getCapabilities().then(setCaps).catch(() => undefined);
    void refreshApps();
    void refreshPerms();
    const t = setInterval(() => { void native.listAudioApps().then(setApps).catch(() => undefined); }, 5000);
    const onFocus = () => { void refreshPerms(); };
    window.addEventListener('focus', onFocus);
    return () => { clearInterval(t); window.removeEventListener('focus', onFocus); };
  }, [native, refreshApps, refreshPerms]);

  // Default dub device: a virtual one (BlackHole) when present.
  useEffect(() => {
    if (!dubDevice && outputDevices.length) setDubDevice((outputDevices.find((d) => d.isVirtual) ?? outputDevices[0])!.uid);
  }, [dubDevice, outputDevices]);
  useEffect(() => { if (mode === 'voice' && caps && !voiceReady) setMode('text'); }, [mode, caps, voiceReady]);

  // Resume a capture that is already running natively (page reload inside the app).
  useEffect(() => {
    const st = live.status;
    if (roomId || st.state === 'idle' || !st.roomId) return;
    setRoomId(st.roomId);
    if (st.appBundleId) setAppId(st.appBundleId);
    if (st.mode) setMode(st.mode);
  }, [live.status, roomId]);

  // ---- claim
  async function doClaim() {
    if (!parsed.ok || !current) return;
    setActionError(null);
    try {
      const res = await claim.mutateAsync({
        workspaceId: current.id,
        link: parsed,
        title: title.trim(),
        displayName: profile.data?.fullName,
        sourceLanguage: far,
        targetLanguages: [listen],
        listenLanguage: listen,
      });
      setRoomId(res.id);
      setClaimInfo({ created: res.created, heartbeatIntervalSeconds: res.heartbeatIntervalSeconds, leaseSeconds: res.leaseSeconds });
      setLines([]);
      setCaptionsState('idle');
      attempted.current = null;
    } catch { /* shown by the claim alert */ }
  }
  function resetMeeting() {
    setRoomId(null);
    setClaimInfo(null);
    setActionError(null);
    setLines([]);
    setGrant(null);
    setCaptionsState('idle');
    claim.reset();
  }

  // ---- start / stop capture
  async function startCapture(appOverride?: string) {
    const useApp = appOverride ?? appId;
    if (!native || !r || !useApp) return;
    setActionError(null);
    setBusy('start');
    try {
      if (!isCapturer) await takeover.mutateAsync();
      if (!LIVE.includes(r.status) && !TERMINAL.includes(r.status)) await startRoom.mutateAsync();
      if (bridge && bridge.audioMode !== mode) await audioMode.mutateAsync(mode);
      const accessToken = await freshAccessToken();
      if (!accessToken) throw Object.assign(new Error('Your session expired. Sign in again.'), { code: 'AUTH_EXPIRED' });
      const st = await native.startExternalCapture({
        appBundleId: useApp,
        roomId: r.id,
        mode,
        accessToken,
        heartbeatIntervalSeconds: claimInfo?.heartbeatIntervalSeconds,
        ...(mode === 'voice' ? { listenLanguage: listen, dubOutputDeviceUid: dubDevice || undefined } : {}),
      });
      apply(st);
      setHasRun(true);
    } catch (e) {
      if (e instanceof ApiError) {
        if (e.code === 'BRIDGE_AUDIO_MODE_LOCKED') setMode('text');
        setActionError({ code: e.code, message: e.message });
      } else {
        pushError({ code: nativeErrorCode(e), message: nativeErrorMessage(e), fatal: false });
      }
    } finally {
      setBusy(null);
    }
  }
  async function stopCapture() {
    if (!native) return;
    setBusy('stop');
    try {
      apply(await native.stopExternalCapture());
      // Hand the capturer role back right away instead of letting the lease run out (best effort).
      if (isCapturer) await release.mutateAsync().catch(() => undefined);
    } catch (e) { pushError({ code: nativeErrorCode(e), message: nativeErrorMessage(e) }); } finally { setBusy(null); }
  }

  // ---- auth-required -> refresh the session -> updateAuth
  const authBusy = useRef(false);
  useEffect(() => {
    if (!live.authRequired || !native || authBusy.current) return;
    authBusy.current = true;
    void (async () => {
      try {
        const next = await session.refresh();
        if (next) {
          await native.updateAuth({ accessToken: next.accessToken });
          dismissError('AUTH_EXPIRED');
          toast({ tone: 'success', title: 'Session refreshed', description: 'The desktop app is using the new access token.' });
        } else {
          pushError({ code: 'AUTH_EXPIRED', message: 'Your session expired. Sign in again to keep capturing.' });
        }
      } catch (e) {
        pushError({ code: nativeErrorCode(e) === 'INTERNAL' ? 'AUTH_REFRESH_FAILED' : nativeErrorCode(e), message: `Could not refresh the session: ${nativeErrorMessage(e)}` });
      } finally {
        clearAuthRequired();
        authBusy.current = false;
      }
    })();
  }, [live.authRequired, native, clearAuthRequired, dismissError, pushError, toast]);

  // ---- permissions
  async function requestPerm(k: PermissionKind) {
    if (!native) return;
    try { setPerms(await native.requestPermission(k)); } catch (e) { setActionError({ code: nativeErrorCode(e), message: nativeErrorMessage(e) }); }
  }
  async function openPerm(k: PermissionKind | 'privacy') {
    if (!native) return;
    try { await native.openSystemSettings(k); } catch (e) { setActionError({ code: nativeErrorCode(e), message: nativeErrorMessage(e) }); }
  }
  const firstMissing = (kinds: PermissionKind[]) => kinds.find((k) => perms && perms[k] !== 'granted');

  // ---- captions (LiveKit listener) + overlay mirror
  const meetingJoin = useMeetingJoin(roomId ?? '');
  const [grant, setGrant] = useState<JoinGrant | null>(null);
  const [captionsState, setCaptionsState] = useState<CaptionsState>('idle');
  const [captionsMsg, setCaptionsMsg] = useState('');
  const [lines, setLines] = useState<CaptionLine[]>([]);
  const attempted = useRef<string | null>(null);

  const connectCaptions = useCallback(async () => {
    if (!roomId) return;
    setCaptionsState('connecting');
    setCaptionsMsg('');
    try {
      setGrant(await meetingJoin.mutateAsync({ displayName: profile.data?.fullName, ...(copilot ? {} : { clientTag: 'listener' as const }) }));
    } catch (e) {
      if (e instanceof ApiError && (e.code === 'LIVEKIT_NOT_CONFIGURED' || e.status === 503)) {
        setCaptionsState('unavailable');
        setCaptionsMsg('Captions are unavailable: the server has no LiveKit credentials. Capture still works.');
      } else {
        setCaptionsState('error');
        setCaptionsMsg(errMsg(e));
      }
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [roomId, profile.data?.fullName, copilot]);

  // Join once per claimed room, as soon as it is live.
  useEffect(() => {
    if (!roomId || !roomLive || attempted.current === roomId) return;
    attempted.current = roomId;
    void connectCaptions();
  }, [roomId, roomLive, connectCaptions]);

  const onCaption = useCallback((l: CaptionLine) => setLines((cur) => mergeCaption(cur, l)), []);

  const [overlayOn, setOverlayOn] = useState(false);
  const [overlayClosed, setOverlayClosed] = useState(false);
  const [clickThrough, setClickThrough] = useState(false);
  const [hideShare, setHideShare] = useState(false);
  async function toggleOverlay(v: boolean) {
    if (!native) return;
    try {
      if (v) await native.showOverlay(true);
      else { await native.clearOverlay(); await native.showOverlay(false); }
      setOverlayOn(v);
    } catch (e) { setActionError({ code: nativeErrorCode(e), message: nativeErrorMessage(e) }); }
  }
  async function setOverlayOpt(o: { clickThrough?: boolean; hideFromScreenShare?: boolean }) {
    if (!native) return;
    try {
      const res = await native.setOverlayOptions(o);
      setClickThrough(res.clickThrough);
      setHideShare(res.hideFromScreenShare);
    } catch (e) { setActionError({ code: nativeErrorCode(e), message: nativeErrorMessage(e) }); }
  }
  const pushed = useRef('');
  useEffect(() => {
    if (!native || !overlayOn) return;
    const l = lines[lines.length - 1];
    if (!l) return;
    const tr = pickTranslation(l, listen);
    const key = `${l.id}|${l.text}|${tr ?? ''}|${l.final}`;
    if (pushed.current === key) return;
    pushed.current = key;
    void native.pushOverlayCaption({ speaker: l.speaker, text: l.text, translation: tr, segmentId: l.id, isFinal: l.final }).catch(() => undefined);
  }, [native, overlayOn, lines, listen]);

  // ---- copilot window: auto-start, state relay, commands
  const suggestionStream = useSuggestionStream(roomId ?? undefined, listen, !!roomId);
  const followUps = useFollowUps(roomId ?? undefined, !!roomId);
  const testMode = useSuggestionTestMode();
  // WarpBot for the copilot window: persisted conversation + the turn in flight, relayed as plain messages.
  const askHistory = useAskHistory(roomId ?? undefined);
  const assistant = useAssistant(roomId ?? '');
  const askState = useMemo<OverlayAsk>(() => ({
    busy: assistant.busy,
    messages: [...historyToMessages(askHistory.data?.pages ?? []), ...assistant.local].slice(-60).map((m) => ({
      key: m.key, role: m.role, content: m.content,
      ...(m.status ? { status: m.status } : {}), ...(m.error ? { error: m.error } : {}), ...(m.incomplete ? { incomplete: true } : {}),
    })),
  }), [askHistory.data, assistant.local, assistant.busy]);
  const lineAt = useRef(new Map<string, number>());
  const overlayState = useMemo<OverlayState>(() => {
    const at = lineAt.current;
    // Arrival stamps are cached per line id, so re-renders do not change them.
    // eslint-disable-next-line react-hooks/purity
    const now = Date.now();
    const out: OverlayLine[] = lines.slice(-200).map((l) => {
      if (!at.has(l.id)) at.set(l.id, now);
      const self = !!userId && l.speakerUserId === userId;
      const tr = pickTranslation(l, listen);
      return { id: l.id, speaker: self ? 'You' : l.speaker, self, text: l.text, ...(tr ? { translation: tr } : {}), ...(l.lang ? { lang: l.lang } : {}), final: l.final, at: at.get(l.id) ?? now };
    });
    const st = live.status.state;
    const status: OverlayState['status'] = roomTerminal ? 'ended'
      : running && st === 'running' ? 'translating'
        : !roomId || busy === 'start' || st === 'starting' ? 'connecting'
          : hasRun ? 'stopped' : 'ready';
    const err = live.errors[0]?.message ?? actionError?.message ?? (captionsState === 'error' ? captionsMsg : undefined);
    return {
      title: title.trim() || r?.title || '',
      status, myLang: listen, farLang: far, micOn, lines: out,
      suggestions: suggestionStream.items.map((s) => ({ id: s.suggestionId, category: s.category, content: s.content, ...(s.detail ? { detail: s.detail } : {}) })),
      followUps: (followUps.data ?? []).map((f) => ({ id: f.id, text: f.text, ...(f.owner ? { owner: f.owner } : {}) })),
      suggestionsAvailable: suggestionStream.status !== 'unavailable',
      ...(testMode ? { testMode: true } : {}),
      ...(roomId ? { ask: askState } : {}),
      ...(err ? { error: err } : {}),
    };
  }, [askState, testMode, lines, userId, listen, far, micOn, title, r?.title, roomTerminal, running, roomId, busy, hasRun, live.status.state, live.errors, actionError, captionsState, captionsMsg, suggestionStream.items, suggestionStream.status, followUps.data]);
  const overlayStateRef = useRef(overlayState);
  overlayStateRef.current = overlayState;
  useEffect(() => { void native?.pushOverlayState?.(overlayState).catch(() => undefined); }, [native, overlayState]);

  async function endSession() {
    if (running) await stopCapture();
    try { if (roomId) await endMeeting.mutateAsync(); } catch (e) { setActionError({ code: errCode(e), message: errMsg(e) }); }
  }
  const openCopilot = useCallback(() => {
    if (!native?.openOverlayWindow) return;
    setOverlayClosed(false);
    void native.openOverlayWindow().catch(() => undefined);
  }, [native]);

  const updateLangs = useUpdateMyLanguages(roomId ?? '');
  // Copilot mode: the pill is the whole UI, so show it as soon as the page opens (it reads "Connecting…").
  const pillShown = useRef(false);
  useEffect(() => {
    if (!copilot || !native?.openOverlayWindow || pillShown.current) return;
    pillShown.current = true;
    openCopilot();
  }, [copilot, native, openCopilot]);

  // Auto-start (link opened from "create meeting"): claim -> pick the call's app -> capture -> open the copilot window. Once.
  const autoClaimed = useRef(false);
  const autoStarted = useRef(false);
  useEffect(() => {
    if (!auto || !native || !current || roomId || autoClaimed.current || !parsed.ok || sameLang) return;
    autoClaimed.current = true;
    void doClaim();
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto, native, current, roomId, parsed.ok, sameLang]);
  useEffect(() => {
    if (!auto || !native || !r || autoStarted.current || !canCapture || roomTerminal || running || busy) return;
    const app = pickAppFor(apps, parsed.ok ? parsed.provider : undefined);
    if (!app) return; // the app list refreshes every few seconds; try again then
    autoStarted.current = true;
    setAppId(app.bundleId);
    void startCapture(app.bundleId);
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [auto, native, r, canCapture, roomTerminal, running, busy, apps]);

  // Commands from the copilot window (relayed by the host as `warptalk-native` events).
  const cmdRef = useRef<(command: string, value?: string) => void>(() => undefined);
  cmdRef.current = (command, value) => {
    switch (command) {
      case 'start': {
        const app = apps.find((a) => a.bundleId === appId) ?? pickAppFor(apps, parsed.ok ? parsed.provider : undefined);
        if (app && !running) void startCapture(app.bundleId);
        break;
      }
      case 'stop': if (running) void stopCapture(); break;
      case 'listen':
        if (!value) break;
        setListen(value);
        // One language per person: the user speaks it and reads everything in it. The server adds it as a target.
        if (roomId) void updateLangs.mutateAsync({ speakLanguage: value, listenLanguage: value }).catch((e) => setActionError({ code: errCode(e), message: errMsg(e) }));
        break;
      case 'far': if (value) setFar(value); break;
      case 'mic': setMicOn(value === 'on'); break;
      case 'ready': void native?.pushOverlayState?.(overlayStateRef.current).catch(() => undefined); break;
      case 'closed': setOverlayClosed(true); break;
      case 'end': void endSession(); break;
      case 'ask': {
        // Suggestion actions carry the hint's context and may run long: the server caps questions at ASK_MAX_CHARS.
        const q = value?.trim().slice(0, ASK_MAX_CHARS);
        if (!roomId || !q) break;
        void assistant.ask(q, 'meeting+docs');
        break;
      }
      case 'askStop': assistant.stop(); break;
      case 'testLine': {
        if (!roomId || !testMode || !value?.trim()) break;
        // Dev test mode: the line goes into the meeting as a final transcript segment; the scripted engine answers it
        // over the suggestion stream. It never passes through the captions channel, so it is shown here.
        void sendTestLine(roomId, value, listen)
          .then((r) => onCaption({ id: r.segmentId, speaker: r.speakerName || 'You', text: r.text, lang: r.language, translations: {}, final: true, speakerUserId: userId ?? undefined, named: true }))
          .catch((e) => setActionError({ code: errCode(e), message: errMsg(e) }));
        break;
      }
    }
  };
  useEffect(() => {
    const on = (e: Event) => {
      const d = (e as CustomEvent<NativeEvent>).detail;
      if (d?.type === 'overlay-command') cmdRef.current(d.command, d.value);
    };
    window.addEventListener('warptalk-native', on);
    return () => window.removeEventListener('warptalk-native', on);
  }, []);

  // ---- errors and the way out of each
  const errors: NativeError[] = live.errors;
  function recoveryFor(e: NativeError): Recovery {
    const act = (id: string, label: string, run: () => void) => ({ id, label, run });
    switch (e.code) {
      case 'PERMISSION_DENIED': {
        const req = firstMissing(['screenRecording', 'microphone']);
        const pane = firstMissing(['audioCapture', 'screenRecording', 'microphone']) ?? 'privacy';
        return {
          hint: 'macOS blocked the audio capture. Grant the permission, then start the capture again.',
          actions: [
            ...(req ? [act('request', 'Request permission', () => { void requestPerm(req); })] : []),
            act('settings', 'Open System Settings', () => { void openPerm(pane); }),
          ],
        };
      }
      case 'NO_AUDIO_PROCESS':
        return { hint: 'The app is not playing sound yet. Join the call, make sure someone is audible, then try again.', actions: [act('retry', 'Try again', () => { dismissError(e.code); void startCapture(); })] };
      case 'APP_NOT_RUNNING':
        return { hint: 'The selected app is not running. Open it, then refresh the list.', actions: [act('apps', 'Refresh apps', () => { dismissError(e.code); void refreshApps(); })] };
      case 'ALREADY_RUNNING':
        return { hint: 'A capture is already running in the desktop app.', actions: [act('sync', 'Sync status', () => { dismissError(e.code); void native?.captureStatus().then(apply); })] };
      case 'LEASE_LOST':
        return { hint: 'Another machine took over the capturer role. Use "Take over capture" if this is the machine that should listen.', actions: [act('room', 'Refresh room', () => { dismissError(e.code); void room.refetch(); })] };
      case 'AUTH_EXPIRED':
      case 'AUTH_REFRESH_FAILED':
        return {
          hint: 'The access token the desktop app uses is no longer valid.',
          actions: [act('refresh', 'Refresh session', () => {
            dismissError(e.code);
            void session.refresh().then(async (s) => {
              if (s) await native?.updateAuth({ accessToken: s.accessToken });
              else pushError({ code: 'AUTH_EXPIRED', message: 'Your session expired. Sign in again.' });
            }).catch(() => undefined);
          })],
        };
      default:
        return { hint: e.fatal ? 'Start the capture again once the problem is fixed.' : 'The capture keeps running; dismiss this when you have checked it.', actions: [] };
    }
  }

  const connection: ConnectionLabel = captionsState === 'connected' ? 'connected' : captionsState === 'connecting' ? 'connecting' : captionsState === 'idle' || captionsState === 'unavailable' ? 'idle' : 'disconnected';
  const captionPlaceholder = !roomId
    ? 'Claim a meeting to see captions here.'
    : !roomLive
      ? 'Captions start when the room is live.'
      : captionsState === 'unavailable' || captionsState === 'error'
        ? captionsMsg
        : captionsState === 'connecting' ? 'Connecting to the caption stream…' : 'Waiting for someone to speak in the meeting.';

  const claimed = !!roomId;
  const appOptions = apps.map((a) => ({ value: a.bundleId, label: `${a.name}${a.producingAudio ? ' · playing audio' : ' · silent'}` }));
  const selectedApp = apps.find((a) => a.bundleId === appId);
  const canStart = !!native && canCapture && !!appId && !running && !roomTerminal && busy === null && (mode !== 'voice' || voiceReady);

  const captionsNode = grant && (
    <Suspense fallback={null}>
      <ExternalCaptions
        grant={grant}
        onCaption={onCaption}
        onConnected={() => setCaptionsState('connected')}
        onDisconnected={() => { setCaptionsState('idle'); setGrant(null); }}
        onError={(m) => { setCaptionsState('error'); setCaptionsMsg(m); }}
        audio={copilot && micOn}
      />
    </Suspense>
  );

  // Copilot mode (opened from "Create meeting"): the overlay pill is the whole UI; this page renders nothing and only
  // keeps the capture, captions and state relay alive.
  if (copilot && native && current) {
    return (
      <>
        {captionsNode}
        <div className="flex min-h-[60vh] flex-col items-center justify-center gap-3 text-center" data-testid="copilot-host">
          <p className="text-body font-medium text-ink">Copilot is running in the floating pill.</p>
          <p className="max-w-sm text-body-sm text-ink-subtle">Keep this window open while the meeting runs: it carries the capture and the captions.</p>
          <div className="flex gap-2">
            <Button variant="secondary" onClick={openCopilot}>Show copilot</Button>
            <Button variant="ghost" onClick={() => void endSession().finally(() => window.location.assign('/'))}>End and go back</Button>
          </div>
        </div>
      </>
    );
  }

  return (
    <>
      <PageHeader
        title="External meeting"
      />

      {!native && <div className="mb-6"><BrowserFallback /></div>}

      {!current ? (
        <EmptyState icon={<Cast className="size-5" />} title="No workspace selected" description="Create or select a workspace to claim an external meeting." action={<Link to="/workspaces?new=1" className="text-body">Create workspace</Link>} />
      ) : (
        <div className="flex flex-col gap-6">
          {claimed && r && <StatusStrip connection={connection} quality="unknown" room={r} />}
          <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_380px]">
            <div className="flex flex-col gap-6">
              <StepCard
                testId="external-meeting-card"
                title="1. Meeting"
                description="Paste the join link of the call you want translated."
                actions={claimed ? <Button size="sm" disabled={running} onClick={resetMeeting}>Use a different meeting</Button> : undefined}
              >
                <Field
                  label="Meeting link"
                  hint={parsed.ok ? undefined : 'Google Meet, Zoom or Microsoft Teams join link, or a meeting code.'}
                  error={link.trim() && !parsed.ok ? parsed.error : undefined}
                >
                  <Input value={link} onChange={(e) => setLink(e.target.value)} disabled={claimed} placeholder="https://meet.google.com/abc-defg-hij" autoComplete="off" spellCheck={false} />
                </Field>
                {parsed.ok && (
                  <p className="flex flex-wrap items-center gap-2 text-body-sm text-ink-subtle" data-testid="link-parse">
                    <Badge tone="primary" data-testid="provider-badge" data-provider={parsed.provider}>{PROVIDER_LABEL[parsed.provider]}</Badge>
                    <span>Meeting <code className="font-mono text-ink-muted" data-testid="meeting-code">{parsed.code}</code></span>
                  </p>
                )}
                <Field label="Title (optional)"><Input value={title} onChange={(e) => setTitle(e.target.value)} disabled={claimed} maxLength={255} /></Field>
                <div className="grid gap-4 sm:grid-cols-2">
                  <Field label="Far-side language" hint="What the other participants speak."><Select value={far} onValueChange={setFar} options={LANG_OPTIONS} disabled={claimed} /></Field>
                  <Field label="Listen language" error={sameLang ? 'Pick a different language than the far side.' : undefined} hint={sameLang ? undefined : 'What you want to read or hear.'}><Select value={listen} onValueChange={setListen} options={LANG_OPTIONS} disabled={claimed} /></Field>
                </div>
                {claim.isError && <Alert tone="danger" title="Could not claim the meeting">{errMsg(claim.error)}</Alert>}
                {!claimed && (
                  <div className="flex items-center gap-3">
                    <Button variant="primary" loading={claim.isPending} disabled={!parsed.ok || sameLang} onClick={() => void doClaim()}>Claim meeting</Button>
                    <span className="text-caption text-ink-subtle">Claiming joins the existing room for this meeting or creates one.</span>
                  </div>
                )}
              </StepCard>

              {claimed && (
                <StepCard
                  testId="external-room-card"
                  title="2. Room"
                  description={claimInfo?.created ? 'New room created for this meeting.' : 'Joined the existing room for this meeting.'}
                  actions={r ? <StatusPill status={r.status} /> : undefined}
                >
                  {room.isLoading && !r ? <p className="text-body-sm text-ink-subtle">Loading room…</p> : null}
                  {room.isError && <Alert tone="danger" title="Could not load the room">{errMsg(room.error)}</Alert>}
                  {r && bridge && (
                    <>
                      <dl className="grid gap-4 sm:grid-cols-2">
                        <KeyValue label="Meeting">{PROVIDER_LABEL[bridge.provider as keyof typeof PROVIDER_LABEL] ?? bridge.provider} · <span className="font-mono text-body-sm">{bridge.meetingCode.replace(/^https?:\/\//, '')}</span></KeyValue>
                        <KeyValue label="Languages">{languageLabel(r.sourceLanguage)} → {r.targetLanguages.map(languageLabel).join(', ')}</KeyValue>
                        <KeyValue label="Your role" testId="bridge-role">
                          <Badge tone={isCapturer ? 'primary' : 'neutral'} data-role={isCapturer ? 'capturer' : 'viewer'}>{isCapturer ? 'Capturer' : 'Viewer'}</Badge>
                        </KeyValue>
                        <KeyValue label="Audio mode" testId="bridge-audio-mode">{bridge.audioMode === 'voice' ? 'Voice (dubbed into the call)' : 'Text only'}</KeyValue>
                        <KeyValue label="Capturer heartbeat" testId="bridge-lease">
                          {bridge.capturerUserId ? (
                            <span className="flex items-center gap-2">{ago(bridge.capturerLeaseAt, now)}{!stale && leaseLeft !== null && <span className="text-body-sm text-ink-subtle" data-testid="lease-left">lease ends in {leaseLeft}s</span>}{stale && <Badge tone="warning" data-testid="lease-stale">Stale</Badge>}</span>
                          ) : 'No capturer'}
                        </KeyValue>
                        <KeyValue label="Server audio flag" testId="bridge-audio-flag">{bridge.audioFlowing == null ? 'Unknown' : bridge.audioFlowing ? 'Audio flowing' : 'No audio'}</KeyValue>
                      </dl>
                      {!isCapturer && (
                        <Alert tone={stale ? 'warning' : 'info'} title={stale ? 'The capturer looks inactive' : 'Someone else is capturing this meeting'}>
                          {stale
                            ? 'Its last heartbeat is older than the lease, so you can take over capture.'
                            : 'You are a viewer: captions appear here. Only take over if the capturer left; the server refuses while their heartbeat is fresh.'}
                        </Alert>
                      )}
                      {!isCapturer && !!bridge?.capturerUserId && !roomTerminal && (
                        <div className="flex flex-wrap items-center gap-2">
                          <Button variant={stale ? 'primary' : 'secondary'} loading={takeover.isPending} onClick={() => takeover.mutate()}><Users className="size-4" aria-hidden />Take over capture</Button>
                        </div>
                      )}
                      {takeover.isError && <Alert tone="danger" title="Could not take over">{errMsg(takeover.error)}{errCode(takeover.error) ? ` (${errCode(takeover.error)})` : ''}</Alert>}
                      {isCapturer && !native && (
                        <Alert tone="warning" title="This browser cannot capture">You hold the capturer role, but capture only works in the desktop app. Open the app, or let a teammate take over, otherwise the lease expires.</Alert>
                      )}
                      {roomTerminal && <Alert tone="info" title="This room has ended">Claim the meeting again to start a new room.</Alert>}
                      <div className="flex flex-wrap items-center gap-2">
                        {!roomLive && !roomTerminal && isHost && (
                          <Button variant="secondary" loading={startRoom.isPending} onClick={() => startRoom.mutate()}><Play className="size-4" aria-hidden />Start room</Button>
                        )}
                        <Link to={`/meetings/${r.id}`} className="text-body">Open meeting page</Link>
                        <Link to={`/rooms/${r.id}`} className="text-body">Room details</Link>
                      </div>
                      {startRoom.isError && <Alert tone="danger" title="Could not start the room">{errMsg(startRoom.error)}</Alert>}
                    </>
                  )}
                </StepCard>
              )}

              {native && claimed && canCapture && !roomTerminal && (
                <StepCard
                  testId="external-capture-card"
                  title="3. Capture"
                  description="Pick the app that plays the meeting audio."
                  actions={<Button size="sm" variant="ghost" loading={appsBusy} onClick={() => void refreshApps()}><RefreshCw className="size-4" aria-hidden />Refresh apps</Button>}
                >
                  <div className="grid gap-4 sm:grid-cols-2">
                    <Field label="Source app" hint={apps.length === 0 ? 'No audio apps found. Open your browser or meeting app.' : undefined}>
                      <Select value={appId} onValueChange={setAppId} options={appOptions} placeholder="Choose an app" disabled={running} />
                    </Field>
                    <Field
                      label="Mode"
                      hint={voiceReady ? 'Voice dubs the translation into the call through a virtual microphone.' : 'Voice needs an audio output device such as BlackHole 2ch; the app reports none.'}
                    >
                      <Select
                        value={mode}
                        onValueChange={(v) => setMode(v as CaptureMode)}
                        disabled={running}
                        options={[{ value: 'text', label: 'Text only' }, { value: 'voice', label: 'Voice dub', disabled: !voiceReady }]}
                      />
                    </Field>
                  </div>
                  {selectedApp && <AppRow name={selectedApp.name} producing={selectedApp.producingAudio} />}
                  {mode === 'voice' && voiceReady && (
                    <Field label="Dub output device" hint="Choose this device as the microphone in the meeting app.">
                      <Select value={dubDevice} onValueChange={setDubDevice} disabled={running} options={outputDevices.map((d) => ({ value: d.uid, label: d.isVirtual ? `${d.name} (virtual)` : d.name }))} />
                    </Field>
                  )}
                  {actionError && <Alert tone="danger" title={actionError.code === 'BRIDGE_AUDIO_MODE_LOCKED' ? 'Voice mode is locked' : 'Something went wrong'}>
                    {actionError.message}{actionError.code ? ` (${actionError.code})` : ''}
                    {actionError.code === 'BRIDGE_AUDIO_MODE_LOCKED' && ' Stay on Text only for this room, or claim a new room.'}
                  </Alert>}
                  <div className="flex items-center gap-3">
                    {running ? (
                      <Button variant="danger" loading={busy === 'stop'} onClick={() => void stopCapture()}><Square className="size-4" aria-hidden />Stop capture</Button>
                    ) : (
                      <Button variant="primary" loading={busy === 'start'} disabled={!canStart} onClick={() => void startCapture()}><Mic className="size-4" aria-hidden />Start capture</Button>
                    )}
                    {!appId && !running && <span className="text-caption text-ink-subtle">Choose a source app to start.</span>}
                  </div>
                </StepCard>
              )}

              {native && claimed && canCapture && !roomTerminal && (
                <StepCard testId="external-live-card" title="Live status" description="Reported by the desktop app" actions={<Badge tone={live.status.state === 'running' ? 'success' : live.status.state === 'error' ? 'danger' : 'neutral'} data-testid="capture-state" data-state={live.status.state}>{stateLabel(live.status.state)}</Badge>}>
                  {errors.map((e) => <ErrorRow key={e.code} error={e} recovery={recoveryFor(e)} onDismiss={() => dismissError(e.code)} />)}
                  <LevelMeter dbfs={live.level} />
                  <dl className="grid gap-4 sm:grid-cols-2">
                    <KeyValue label="Audio" testId="audio-flowing">
                      <Badge tone={live.audioFlowing ? 'success' : 'warning'} data-flowing={live.audioFlowing}>{live.audioFlowing ? 'Audio flowing' : 'No audio buffers'}</Badge>
                    </KeyValue>
                    <KeyValue label="Last sound" testId="last-audio">{ago(live.lastAudioAt, now)}</KeyValue>
                    <KeyValue label="Last heartbeat" testId="last-heartbeat">
                      {live.heartbeat ? (
                        <span className="flex flex-wrap items-center gap-2">{ago(live.heartbeat.at ?? live.heartbeat.ts, now)}<Badge tone={live.heartbeat.ok ? 'success' : 'danger'}>{live.heartbeat.ok ? 'OK' : 'Failed'}</Badge>{live.heartbeat.message && <span className="text-body-sm text-ink-subtle">{live.heartbeat.message}</span>}</span>
                      ) : 'none yet'}
                    </KeyValue>
                    <KeyValue label="Backend" testId="capture-backend">{live.status.backend === 'processTap' ? 'Process tap' : live.status.backend === 'screenCaptureKit' ? 'ScreenCaptureKit' : 'n/a'}</KeyValue>
                  </dl>
                  {live.logs.length > 0 && (
                    <ul aria-label="Native log" className="space-y-0.5 font-mono text-caption text-ink-subtle">{live.logs.map((l, i) => <li key={`${i}-${l}`}>{l}</li>)}</ul>
                  )}
                </StepCard>
              )}
            </div>

            <div className="flex flex-col gap-6">
              <CaptionsPanel
                title="Live captions"
                description="Original and translation, with delivery latency"
                lines={lines}
                translationLang={listen}
                placeholder={captionPlaceholder}
              />
              {claimed && roomLive && (captionsState === 'error' || captionsState === 'unavailable') && (
                <div><Button size="sm" onClick={() => void connectCaptions()}><RefreshCw className="size-4" aria-hidden />Retry captions</Button></div>
              )}
              {captionsNode}
              {native?.openOverlayWindow && (
                <StepCard title="Copilot window" description="Live transcript and AI suggestions in a floating window">
                  <Button size="sm" onClick={openCopilot}>{overlayClosed ? 'Reopen copilot window' : 'Open copilot window'}</Button>
                </StepCard>
              )}
              {native && (
                <StepCard title="Caption overlay" description="A floating window over the meeting app">
                  <Switch
                    checked={overlayOn}
                    onCheckedChange={(v) => void toggleOverlay(v)}
                    disabled={caps ? !caps.overlay : false}
                    label="Show caption overlay"
                    description={caps && !caps.overlay ? 'Not supported by this app build.' : 'Captions are mirrored here as they arrive.'}
                  />
                  {overlayOn && (
                    <>
                      <Switch checked={clickThrough} onCheckedChange={(v) => void setOverlayOpt({ clickThrough: v, hideFromScreenShare: hideShare })} label="Click-through" description="Mouse clicks pass through to the app below." />
                      <Switch checked={hideShare} onCheckedChange={(v) => void setOverlayOpt({ clickThrough, hideFromScreenShare: v })} label="Hide from screen sharing" description="The overlay is not captured when you share your screen." />
                    </>
                  )}
                </StepCard>
              )}
              {native && (
                <PermissionsPanel perms={perms} busy={permsBusy} onRefresh={() => void refreshPerms()} onRequest={(k) => void requestPerm(k)} onOpen={(k) => void openPerm(k)} />
              )}
            </div>
          </div>
        </div>
      )}
    </>
  );
}

function stateLabel(s: string) {
  return { idle: 'Idle', starting: 'Starting', running: 'Running', stopping: 'Stopping', error: 'Error' }[s] ?? s;
}
