// Live-mode pieces of the copilot session UI that need the desktop host: capture status per channel, partial lines,
// recovery for capture errors and the suggestion mirror to the overlay. Native contract: src/lib/native.ts.
import { useCallback, useEffect, useRef, useState } from 'react';
import { Microphone, SpeakerHigh } from '@phosphor-icons/react';
import { useNavigate } from 'react-router-dom';
import { Badge, Tooltip } from '@/ui';
import { useWorkspaceOptional } from '@/lib/workspace';
import { session } from '@/lib/session';
import type { Suggestion } from '@/lib/suggestions';
import { nativeErrorCode, nativeErrorMessage, type AudioApp, type NativeError, type PermissionKind, type Permissions, type WarptalkNative } from '@/lib/native';
import type { ChannelKind, CopilotLive } from '@/lib/copilotLive';
import { STT } from '@/lib/strings';
import { ErrorRow, LevelMeter, type Recovery } from './ExternalParts';


const KIND_LABEL: Record<ChannelKind, string> = { idle: 'Off', connecting: 'Connecting', connected: 'Connected', reconnecting: 'Reconnecting', error: 'Problem' };
const KIND_TONE: Record<ChannelKind, 'success' | 'warning' | 'danger' | 'neutral'> = { idle: 'neutral', connecting: 'warning', connected: 'success', reconnecting: 'warning', error: 'danger' };

/** Capture status of the two channels: me = microphone, others = the meeting app. */
export function LiveCapturePanel({ live }: { live: CopilotLive }) {
  return (
    <div data-testid="live-capture" role="group" aria-label="Capture status" className="grid gap-x-6 gap-y-2 rounded-lg border border-hairline bg-surface-2 px-3 py-2.5 sm:grid-cols-2">
      {([['me', 'Me (microphone)', Microphone], ['others', 'Others (meeting app)', SpeakerHigh]] as const).map(([ch, label, Icon]) => {
        const c = live.channels[ch];
        return (
          <div key={ch} data-testid={`channel-${ch}`} data-kind={c.kind} className="flex min-w-0 flex-col gap-1.5">
            <div className="flex items-center gap-2">
              <Icon size={16} aria-hidden className="shrink-0 text-ink-subtle" />
              <span className="min-w-0 flex-1 truncate text-body-sm font-medium text-ink">{label}</span>
              {c.route === 'server' && (
                <Tooltip content={STT.hint}>
                  <span tabIndex={0} className="inline-flex rounded-pill focus-ring" data-testid={`channel-${ch}-route`} data-route="server">
                    <Badge tone="info" size="sm">{STT.badge}</Badge>
                    <span className="sr-only">. {STT.hint}</span>
                  </span>
                </Tooltip>
              )}
              <Badge tone={KIND_TONE[c.kind]} size="sm" data-testid={`channel-${ch}-status`}>{KIND_LABEL[c.kind]}</Badge>
            </div>
            <LevelMeter dbfs={c.level} label={`${ch === 'me' ? 'Microphone' : 'Meeting audio'} level`} testId={`meter-${ch}`} />
            {c.detail && <p className="truncate text-caption text-ink-subtle" title={c.detail} data-testid={`channel-${ch}-detail`}>{c.detail}</p>}
          </div>
        );
      })}
    </div>
  );
}

/** Interim recognition per side: replaced by the persisted line once it is final. */
export function PartialLines({ partials }: { partials: CopilotLive['partials'] }) {
  const rows = (['me', 'others'] as const).filter((c) => partials[c].trim());
  if (rows.length === 0) return null;
  return (
    <ul aria-label="Live partial lines" data-testid="partials" className="border-b border-row">
      {rows.map((c) => (
        <li key={c} data-testid="partial-line" data-channel={c} className="flex gap-3 bg-surface-2/50 px-4 py-2">
          <span className="w-12 shrink-0 pt-0.5 text-caption text-ink-subtle" aria-hidden>…</span>
          <span className="min-w-0 flex-1">
            <span className="block text-body-sm font-medium text-ink-muted">{c === 'me' ? 'Me' : 'Others'}</span>
            <span className="block break-words text-body italic text-ink-subtle">{partials[c]}</span>
          </span>
        </li>
      ))}
    </ul>
  );
}

/** Meeting apps currently known to the host (`listAudioApps`). */
export function useAudioApps(native: WarptalkNative | undefined) {
  const [apps, setApps] = useState<AudioApp[]>([]);
  const [mics, setMics] = useState<{ uid: string; name: string }[]>([]);
  const refresh = useCallback(async () => {
    if (!native) return;
    try { setApps(await native.listAudioApps()); } catch { setApps([]); }
    try { setMics((await native.listInputDevices?.()) ?? []); } catch { setMics([]); }
  }, [native]);
  useEffect(() => {
    if (!native) return;
    let alive = true;
    native.listAudioApps().then((a) => { if (alive) setApps(a); }).catch(() => undefined);
    native.listInputDevices?.().then((m) => { if (alive) setMics(m); }).catch(() => undefined);
    return () => { alive = false; };
  }, [native]);
  return { apps, mics, refresh };
}

/**
 * A capture error with the way out (patterns of /external): permission errors offer macOS' request and System Settings,
 * a vanished app a refresh, an expired token a refreshed token, everything else retry.
 */
export function CaptureRecovery({
  native, error, onRetry, onDismiss, onRefreshApps,
}: { native: WarptalkNative; error: NativeError; onRetry?(): void; onDismiss(): void; onRefreshApps?(): void }) {
  const nav = useNavigate();
  const ws = useWorkspaceOptional();
  const [perms, setPerms] = useState<Permissions | null>(null);
  const [note, setNote] = useState('');
  const denied = error.code === 'PERMISSION_DENIED';
  useEffect(() => {
    if (!denied) return;
    let alive = true;
    native.permissions().then((p) => { if (alive) setPerms(p); }).catch(() => undefined);
    return () => { alive = false; };
  }, [native, denied]);

  const missing = (kinds: PermissionKind[]) => kinds.find((k) => perms && perms[k] !== 'granted');
  const act = (id: string, label: string, run: () => void) => ({ id, label, run });
  const guard = (p: Promise<unknown>) => p.catch((e: unknown) => setNote(`${nativeErrorCode(e)}: ${nativeErrorMessage(e)}`));
  const retry = onRetry ? [act('retry', 'Try again', () => { onDismiss(); onRetry(); })] : [];

  let recovery: Recovery;
  switch (error.code) {
    case 'PERMISSION_DENIED': {
      const req = missing(['microphone', 'screenRecording']);
      const pane = missing(['microphone', 'audioCapture', 'screenRecording']) ?? 'privacy';
      recovery = {
        hint: 'macOS blocked the microphone or the meeting audio. Grant the permission, then try again.',
        actions: [
          ...(req ? [act('request', 'Request permission', () => { void guard(native.requestPermission(req).then(setPerms)); })] : []),
          act('settings', 'Open System Settings', () => { void guard(native.openSystemSettings(pane)); }),
          ...retry,
        ],
      };
      break;
    }
    case 'INSUFFICIENT_CREDITS':
      recovery = {
        hint: 'This workspace has run out of credits, so live transcription is paused. Top up or upgrade, then try again.',
        actions: [act('billing', 'Open billing', () => nav(ws ? ws.wsPath('/billing') : '/')), ...retry],
      };
      break;
    case 'NO_AUDIO_PROCESS':
      recovery = { hint: 'The meeting app is not playing sound yet. Join the call, make sure someone is audible, then try again.', actions: retry };
      break;
    case 'APP_NOT_RUNNING':
      recovery = { hint: 'The selected app is not running. Open it, refresh the list, then try again.', actions: [...(onRefreshApps ? [act('apps', 'Refresh apps', onRefreshApps)] : []), ...retry] };
      break;
    case 'AUTH_EXPIRED': case 'AUTH_REFRESH_FAILED': case 'UNAUTHORIZED':
      recovery = {
        hint: 'The access token the desktop app uses is no longer valid.',
        actions: [act('refresh', 'Refresh session', () => {
          void guard(session.refresh().then(async (s) => { if (s) { await native.updateAuth({ accessToken: s.accessToken }); onDismiss(); onRetry?.(); } else setNote('Your session expired. Sign in again.'); }));
        })],
      };
      break;
    default:
      recovery = { hint: error.fatal ? 'Capture did not start. Fix the problem and try again.' : 'Capture keeps running; dismiss this when you have checked it.', actions: error.fatal ? retry : [] };
  }
  return (
    <div className="flex flex-col gap-1">
      <ErrorRow error={error} recovery={recovery} onDismiss={onDismiss} />
      {note && <p role="status" className="text-caption text-ink-subtle" data-testid="recovery-note">{note}</p>}
    </div>
  );
}

/**
 * Mirrors the visible suggestions to the host overlay: pushes new ones, clears those that were dismissed, snoozed or expired,
 * and clears everything when capture stops or the page closes.
 */
export function useOverlayMirror(native: WarptalkNative | undefined, active: boolean, visible: Suggestion[]) {
  const pushed = useRef(new Set<string>());
  useEffect(() => {
    if (!native?.pushOverlaySuggestion || !active) return;
    const want = new Map(visible.map((s) => [s.suggestionId, s]));
    for (const [id, s] of want) {
      if (pushed.current.has(id)) continue;
      pushed.current.add(id);
      void native.pushOverlaySuggestion({ id, category: s.category, content: s.content, ...(s.followUp ? { followUp: s.followUp } : {}) }).catch(() => undefined);
    }
    for (const id of [...pushed.current]) {
      if (want.has(id)) continue;
      pushed.current.delete(id);
      void native.clearOverlaySuggestion?.(id)?.catch(() => undefined);
    }
  }, [native, active, visible]);
  // Capture stopped / unmounted: drop whatever is still on the overlay.
  useEffect(() => {
    if (active) return;
    const ids = [...pushed.current];
    pushed.current.clear();
    for (const id of ids) void native?.clearOverlaySuggestion?.(id)?.catch(() => undefined);
  }, [native, active]);
  useEffect(() => () => {
    for (const id of pushed.current) void native?.clearOverlaySuggestion?.(id)?.catch(() => undefined);
    pushed.current.clear();
  }, [native]);
}
