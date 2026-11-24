// Live copilot capture state mirrored from the native host (`startCopilotSession` + `copilot.*` events on `warptalk-native`)
// and the helpers that merge native segments with the polled transcript (the server stays the source of truth).
import { useCallback, useEffect, useState } from 'react';
import { session } from './session';
import { apiBaseUrl } from './env';
import type { TranscriptLine } from './copilot';
import {
  nativeErrorCode, nativeErrorMessage, type CopilotChannel, type CopilotSegmentEvent, type NativeError, type NativeEvent, type StartCopilotSessionOptions, type WarptalkNative,
} from './native';

export type ChannelKind = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'error';
export interface ChannelState { kind: ChannelKind; level: number; detail?: string }
export interface LiveSegment { clientSegmentId: string; channel: CopilotChannel; text: string; startMs?: number }

export const SILENT_DB = -120;
const idleChannel = (): ChannelState => ({ kind: 'idle', level: SILENT_DB });

export function channelKind(kind: string): ChannelKind {
  const k = kind.toLowerCase();
  if (/reconnect|retry/.test(k)) return 'reconnecting';
  if (/connecting|starting|opening|pending/.test(k)) return 'connecting';
  if (/error|fail|denied/.test(k)) return 'error';
  if (/stop|idle|closed|end|off/.test(k)) return 'idle';
  return 'connected'; // connected, ready, listening, running, streaming
}

/** `copilot.levels` carries RMS: negative = dBFS, otherwise linear 0..1. */
export function rmsToDb(rms: number): number {
  if (!Number.isFinite(rms)) return SILENT_DB;
  if (rms < 0) return Math.max(SILENT_DB, rms);
  return rms <= 0 ? SILENT_DB : Math.max(SILENT_DB, 20 * Math.log10(Math.min(1, rms)));
}

// ---- UUID v5 (the backend stores a segment under uuid5(sessionId, clientSegmentId)), used to recognise our own segments in the poll.
function sha1(msg: Uint8Array): Uint8Array {
  const rotl = (n: number, b: number) => ((n << b) | (n >>> (32 - b))) >>> 0;
  const len = msg.length;
  const padded = new Uint8Array((((len + 8) >> 6) + 1) << 6);
  padded.set(msg);
  padded[len] = 0x80;
  const dv = new DataView(padded.buffer);
  dv.setUint32(padded.length - 4, (len * 8) >>> 0);
  dv.setUint32(padded.length - 8, Math.floor((len * 8) / 2 ** 32));
  let h0 = 0x67452301, h1 = 0xefcdab89, h2 = 0x98badcfe, h3 = 0x10325476, h4 = 0xc3d2e1f0;
  const w = new Uint32Array(80);
  for (let o = 0; o < padded.length; o += 64) {
    for (let i = 0; i < 16; i++) w[i] = dv.getUint32(o + i * 4);
    for (let i = 16; i < 80; i++) w[i] = rotl(w[i - 3]! ^ w[i - 8]! ^ w[i - 14]! ^ w[i - 16]!, 1);
    let a = h0, b = h1, c = h2, d = h3, e = h4;
    for (let i = 0; i < 80; i++) {
      const [f, k] = i < 20 ? [(b & c) | (~b & d), 0x5a827999] : i < 40 ? [b ^ c ^ d, 0x6ed9eba1] : i < 60 ? [(b & c) | (b & d) | (c & d), 0x8f1bbcdc] : [b ^ c ^ d, 0xca62c1d6];
      const t = (rotl(a, 5) + (f as number) + e + (k as number) + w[i]!) >>> 0;
      e = d; d = c; c = rotl(b, 30); b = a; a = t;
    }
    h0 = (h0 + a) >>> 0; h1 = (h1 + b) >>> 0; h2 = (h2 + c) >>> 0; h3 = (h3 + d) >>> 0; h4 = (h4 + e) >>> 0;
  }
  const out = new Uint8Array(20);
  const odv = new DataView(out.buffer);
  [h0, h1, h2, h3, h4].forEach((h, i) => odv.setUint32(i * 4, h));
  return out;
}

export function uuidV5(namespace: string, name: string): string | undefined {
  const hex = namespace.replace(/-/g, '');
  if (!/^[0-9a-f]{32}$/i.test(hex)) return undefined;
  const ns = Uint8Array.from(hex.match(/../g)!.map((h) => parseInt(h, 16)));
  const nm = new TextEncoder().encode(name);
  const buf = new Uint8Array(ns.length + nm.length);
  buf.set(ns); buf.set(nm, ns.length);
  const d = sha1(buf).slice(0, 16);
  d[6] = (d[6]! & 0x0f) | 0x50;
  d[8] = (d[8]! & 0x3f) | 0x80;
  const h = Array.from(d, (b) => b.toString(16).padStart(2, '0')).join('');
  return `${h.slice(0, 8)}-${h.slice(8, 12)}-${h.slice(12, 16)}-${h.slice(16, 20)}-${h.slice(20)}`;
}

const norm = (t: string) => t.trim().replace(/\s+/g, ' ').toLowerCase();

/**
 * Native final segments the polled transcript does not contain yet. A segment is "already persisted" when a polled line has its
 * clientSegmentId, its server id (uuid v5 of the session + clientSegmentId) or, as a fallback, the same text (each polled line
 * accounts for at most one native segment).
 */
export function pendingSegments(sessionId: string, polled: TranscriptLine[], local: LiveSegment[]): LiveSegment[] {
  const claimed = new Set<string>();
  const byId = new Map(polled.map((p) => [p.id, p]));
  const pending: LiveSegment[] = [];
  for (const l of local) {
    const direct = byId.get(l.clientSegmentId) ?? byId.get(uuidV5(sessionId, l.clientSegmentId) ?? '');
    if (direct) { claimed.add(direct.id); continue; }
    const text = norm(l.text);
    const twin = polled.find((p) => !claimed.has(p.id) && norm(p.text) === text);
    if (twin) { claimed.add(twin.id); continue; }
    pending.push(l);
  }
  return pending;
}

/** Fresh access token for the native host (refreshes when the current one is about to expire). */
export async function freshAccessToken(): Promise<string> {
  const exp = session.info?.expiresAt;
  if (!session.accessToken || !exp || exp - Date.now() < 10 * 60_000) {
    try { await session.refresh(); } catch { /* use what we have */ }
  }
  return session.accessToken ?? '';
}

export async function startCopilotCapture(native: WarptalkNative, o: Omit<StartCopilotSessionOptions, 'accessToken' | 'apiBaseUrl'>) {
  if (!native.startCopilotSession) throw Object.assign(new Error('This app cannot capture audio.'), { code: 'UNSUPPORTED' });
  return native.startCopilotSession({ ...o, accessToken: await freshAccessToken(), apiBaseUrl: apiBaseUrl() || window.location.origin });
}

export interface CopilotLive {
  /** The session the host is capturing (null = nothing running). */
  attached: string | null;
  channels: Record<CopilotChannel, ChannelState>;
  partials: Record<CopilotChannel, string>;
  segments: LiveSegment[];
  errors: NativeError[];
}
const INITIAL: CopilotLive = {
  attached: null, channels: { me: idleChannel(), others: idleChannel() }, partials: { me: '', others: '' }, segments: [], errors: [],
};
const CHANNELS: CopilotChannel[] = ['me', 'others'];
const isChannel = (c: unknown): c is CopilotChannel => c === 'me' || c === 'others';

/**
 * Mirrors the native copilot capture. On mount it asks `copilotStatus()`, so a reload (or opening the page later) attaches to a capture
 * that is already running. Hosts without the copilot methods behave like a browser (attached stays null).
 */
export function useCopilotLive(native: WarptalkNative | undefined) {
  const [live, setLive] = useState<CopilotLive>(INITIAL);

  const markStarted = useCallback((sessionId: string) => setLive({ ...INITIAL, attached: sessionId, channels: { me: { kind: 'connected', level: SILENT_DB }, others: { kind: 'connected', level: SILENT_DB } } }), []);
  const markStopped = useCallback(() => setLive((l) => ({ ...INITIAL, errors: l.errors })), []);
  const dismissError = useCallback((code: string) => setLive((l) => ({ ...l, errors: l.errors.filter((e) => e.code !== code) })), []);
  const pushError = useCallback((e: { code: string; message: string; fatal?: boolean; channel?: CopilotChannel }) =>
    setLive((l) => ({ ...l, errors: [{ code: e.code, message: e.message, fatal: !!e.fatal, ts: new Date().toISOString() }, ...l.errors.filter((x) => x.code !== e.code)].slice(0, 5) })), []);

  useEffect(() => {
    if (!native?.copilotStatus) return;
    let alive = true;
    native.copilotStatus().then((s) => {
      if (!alive || s.state !== 'running') return;
      const id = s.sessionId ?? s.roomId ?? null;
      if (id) setLive((l) => (l.attached ? l : { ...l, attached: id, channels: { me: { kind: 'connected', level: SILENT_DB }, others: { kind: 'connected', level: SILENT_DB } } }));
    }).catch(() => undefined);
    return () => { alive = false; };
  }, [native]);

  useEffect(() => {
    if (!native) return;
    const onEvent = (e: Event) => {
      const d = (e as CustomEvent<NativeEvent>).detail;
      if (!d) return;
      switch (d.type) {
        case 'copilot.partial': {
          if (!isChannel(d.channel)) return;
          setLive((l) => ({ ...l, partials: { ...l.partials, [d.channel]: d.text ?? '' } }));
          break;
        }
        case 'copilot.segment': {
          const s: CopilotSegmentEvent | undefined = d.segment;
          if (!s || !isChannel(s.channel) || typeof s.text !== 'string') return;
          if (s.isFinal === false) { setLive((l) => ({ ...l, partials: { ...l.partials, [s.channel]: s.text } })); return; }
          setLive((l) => ({
            ...l,
            partials: { ...l.partials, [s.channel]: '' },
            segments: l.segments.some((x) => x.clientSegmentId === s.clientSegmentId) ? l.segments
              : [...l.segments, { clientSegmentId: s.clientSegmentId, channel: s.channel, text: s.text, startMs: s.startMs }].slice(-200),
          }));
          break;
        }
        case 'copilot.status': {
          const kind = channelKind(String(d.kind ?? ''));
          const targets = isChannel(d.channel) ? [d.channel] : CHANNELS;
          setLive((l) => {
            const channels = { ...l.channels };
            for (const c of targets) channels[c] = { kind, level: kind === 'connected' || kind === 'reconnecting' ? channels[c].level : SILENT_DB, detail: d.detail };
            const stopped = !isChannel(d.channel) && kind === 'idle';
            return stopped ? { ...INITIAL, errors: l.errors } : { ...l, channels };
          });
          break;
        }
        case 'copilot.error': {
          setLive((l) => ({
            ...l,
            channels: isChannel(d.channel) ? { ...l.channels, [d.channel]: { ...l.channels[d.channel], kind: 'error', detail: d.message } } : l.channels,
            errors: [{ code: d.code, message: d.message, fatal: false, ts: new Date().toISOString() }, ...l.errors.filter((x) => x.code !== d.code)].slice(0, 5),
          }));
          break;
        }
        case 'copilot.levels': {
          if (!isChannel(d.channel)) return;
          setLive((l) => ({ ...l, channels: { ...l.channels, [d.channel]: { ...l.channels[d.channel], level: rmsToDb(d.rms) } } }));
          break;
        }
      }
    };
    window.addEventListener('warptalk-native', onEvent);
    return () => window.removeEventListener('warptalk-native', onEvent);
  }, [native]);

  // The host keeps using the token it was given: hand it every rotation while capturing.
  const attached = live.attached;
  useEffect(() => {
    if (!native || !attached) return;
    return session.subscribe(() => { const t = session.accessToken; if (t) void native.updateAuth({ accessToken: t }).catch(() => undefined); });
  }, [native, attached]);

  return { live, markStarted, markStopped, dismissError, pushError };
}

export const captureError = (e: unknown) => ({ code: nativeErrorCode(e), message: nativeErrorMessage(e) });
