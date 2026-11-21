// External-meeting (bridge room) helpers: meeting link parsing and API hooks.
// Backend: crates/modules/room/src/http (bridge/claim, bridge/capturer/takeover, bridge/audio-mode).
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { api } from './api';
import { bridgeStateResponse, claimResponse, room as roomSchema } from './schemas';
import { session } from './session';

export type BridgeProvider = 'google_meet' | 'zoom' | 'teams';
export const PROVIDER_LABEL: Record<BridgeProvider, string> = { google_meet: 'Google Meet', zoom: 'Zoom', teams: 'Microsoft Teams' };

export type ParsedLink =
  | { ok: true; provider: BridgeProvider; code: string; url?: string }
  | { ok: false; error: string };

const MEET_CODE = /^[a-z]{3}-[a-z]{4}-[a-z]{3}$/;

/** Client-side mirror of the backend's `normalize_meeting_ref`, with provider detection. */
export function parseMeetingLink(input: string): ParsedLink {
  const raw = input.trim();
  if (!raw) return { ok: false, error: 'Paste the meeting link or code.' };
  const clean = raw.split(/[?#]/)[0]!;
  if (MEET_CODE.test(clean.toLowerCase())) return { ok: true, provider: 'google_meet', code: clean.toLowerCase() };
  const digits = clean.replace(/\s+/g, '');
  if (/^\d{9,11}$/.test(digits)) return { ok: true, provider: 'zoom', code: digits };
  if (!/[/.]/.test(clean)) return { ok: false, error: 'Not a recognised meeting code. Use a Meet code (abc-defg-hij), a Zoom number or a join link.' };
  const rest = clean.replace(/^https?:\/\//i, '');
  const slash = rest.indexOf('/');
  const host = (slash < 0 ? rest : rest.slice(0, slash)).toLowerCase();
  const path = (slash < 0 ? '' : rest.slice(slash + 1)).replace(/^\/+|\/+$/g, '');
  if (host === 'meet.google.com') {
    const code = path.toLowerCase();
    return MEET_CODE.test(code) ? { ok: true, provider: 'google_meet', code, url: raw } : { ok: false, error: 'Google Meet link must end with a code like abc-defg-hij.' };
  }
  if (host === 'zoom.us' || host.endsWith('.zoom.us')) {
    const id = path.replace(/^j\//, '');
    return /^\d{9,11}$/.test(id) ? { ok: true, provider: 'zoom', code: id, url: raw } : { ok: false, error: 'Zoom link must look like zoom.us/j/123456789.' };
  }
  if ((host === 'teams.microsoft.com' || host === 'teams.live.com') && path) {
    return { ok: true, provider: 'teams', code: `https://${host}/${path}`, url: raw };
  }
  return { ok: false, error: 'Only Google Meet, Zoom and Microsoft Teams links are supported.' };
}

export interface ClaimInput {
  workspaceId: string;
  link: Extract<ParsedLink, { ok: true }>;
  title?: string;
  displayName?: string;
  sourceLanguage: string;
  targetLanguages: string[];
  /** Caption language when this claim joins an existing room as a viewer. */
  listenLanguage?: string;
}

export function useClaimBridge() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (i: ClaimInput) =>
      api('/translation-rooms/bridge/claim', {
        method: 'POST',
        schema: claimResponse,
        body: {
          workspaceId: i.workspaceId,
          provider: i.link.provider,
          ...(i.link.url ? { meetingUrl: i.link.url } : { code: i.link.code }),
          title: i.title || undefined,
          displayName: i.displayName || undefined,
          sourceLanguage: i.sourceLanguage,
          targetLanguages: i.targetLanguages,
          listenLanguage: i.listenLanguage,
        },
      }),
    onSuccess: (r) => {
      qc.setQueryData(['room', r.id], r);
      void qc.invalidateQueries({ queryKey: ['rooms'] });
    },
  });
}

export function useBridgeTakeover(roomId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api(`/translation-rooms/${roomId}/bridge/capturer/takeover`, { method: 'POST', schema: bridgeStateResponse }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['room', roomId] }),
  });
}

/** Gives up the capturer role when capture stops, so a teammate can take over immediately. */
export function useBridgeRelease(roomId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api(`/translation-rooms/${roomId}/bridge/capturer/release`, { method: 'POST' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['room', roomId] }),
  });
}

export function useBridgeAudioMode(roomId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (mode: 'voice' | 'text') =>
      api(`/translation-rooms/${roomId}/bridge/audio-mode`, { method: 'PUT', body: { mode }, schema: bridgeStateResponse }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['room', roomId] }),
  });
}

export function useStartBridgeRoom(roomId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api(`/translation-rooms/${roomId}/start`, { method: 'POST', schema: roomSchema }),
    onSuccess: (r) => {
      qc.setQueryData(['room', roomId], r);
      void qc.invalidateQueries({ queryKey: ['rooms'] });
    },
  });
}

/** A JWT that stays valid long enough for native to use it: refreshes the session when < 90s remain. */
export async function freshAccessToken(): Promise<string | null> {
  const exp = session.info?.expiresAt ?? 0;
  if (!session.accessToken || exp - Date.now() < 90_000) {
    const r = await session.refresh();
    return r?.accessToken ?? session.accessToken;
  }
  return session.accessToken;
}

/**
 * Seconds left on the capturer lease, from the server's `leaseExpiresAt` against its `serverTime`
 * (advanced by the time elapsed since the response arrived), so client clock skew never matters.
 * `null` when there is no capturer. Negative = expired.
 */
export function leaseRemainingSeconds(
  bridge: { leaseExpiresAt?: string | null; serverTime?: string | null } | undefined,
  receivedAt: number,
  now = Date.now(),
): number | null {
  if (!bridge?.leaseExpiresAt || !bridge.serverTime) return null;
  const serverNow = Date.parse(bridge.serverTime) + Math.max(0, now - receivedAt);
  return Math.ceil((Date.parse(bridge.leaseExpiresAt) - serverNow) / 1000);
}

/** Stale = no capturer or lease expired (mirrors the backend). */
export function leaseIsStale(bridge: { leaseExpiresAt?: string | null; serverTime?: string | null } | undefined, receivedAt: number, now = Date.now()): boolean {
  const left = leaseRemainingSeconds(bridge, receivedAt, now);
  return left === null || left <= 0;
}
