import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowsClockwise, ListBullets, SidebarSimple } from '@phosphor-icons/react';
import { ParticipantAvatar, useToast } from '@v2/ui';
import { cn } from '@v2/lib/cn';
import { languageLabel } from '@v2/lib/languages';
import { baseIdentity, mergeCaption, parseCaption, pickTranslation, resolveSpeakers, senderLabel, type CaptionLine } from '@v2/lib/captions';
import { useFollowUps, useSuggestionStream, useSuggestionTriage } from '@v2/lib/suggestions';
import { useParticipantAction, useParticipants, useRoomAction, useUpdateMyLanguages } from '@v2/lib/queries';
import type { Room } from '@v2/lib/schemas';
import { StatusStrip, type TimedCaptionLine } from '../MeetingParts';
import { errMsg } from '../errors';
import { ControlBar, type DeviceMenus, type DeviceOption } from './ControlBar';
import { EndMeetingDialog, LeaveDialog } from './Dialogs';
import { roomLanguages } from './PreJoin';
import { parseReaction } from './protocol';
import { useMeetingChat } from './useChat';
import { SidePanel, type PanelTab, type PersonRow } from './SidePanel';
import { Stage, type TileInfo } from './Stage';
import type { MeetingSession } from './types';

const SHOWN = ['CONNECTED', 'WAITING', 'DISCONNECTED', 'INVITED'];
const REACTION_MS = 4000;

interface FloatingReaction { id: number; emoji: string; name: string }

/** Mic and camera inputs, refreshed when devices are plugged in or permission reveals their labels. */
function useInputDevices(enabled: boolean) {
  const [list, setList] = useState<{ mics: DeviceOption[]; cams: DeviceOption[] }>({ mics: [], cams: [] });
  useEffect(() => {
    const md = typeof navigator !== 'undefined' ? navigator.mediaDevices : undefined;
    if (!enabled || !md?.enumerateDevices) return;
    let alive = true;
    const load = () => {
      md.enumerateDevices().then((all) => {
        if (!alive) return;
        const pick = (k: MediaDeviceKind) => all.filter((d) => d.kind === k && d.deviceId && d.deviceId !== 'communications')
          .map((d, i) => ({ id: d.deviceId, label: d.label || `${k === 'audioinput' ? 'Microphone' : 'Camera'} ${i + 1}` }));
        setList({ mics: pick('audioinput'), cams: pick('videoinput') });
      }).catch(() => undefined);
    };
    load();
    md.addEventListener?.('devicechange', load);
    return () => { alive = false; md.removeEventListener?.('devicechange', load); };
  }, [enabled]);
  return list;
}

function wideScreen() {
  try { return window.matchMedia('(min-width: 1024px)').matches; } catch { return true; }
}

export interface InMeetingProps {
  room: Room;
  session: MeetingSession;
  selfUserId: string | null;
  selfName: string;
  listen: string;
  onListen(v: string): void;
  /** Language the transcript translation column follows. */
  captionLang: string;
  onLeave(): void;
  onEnded(): void;
}

/** The full-screen meeting: stage, control bar and the Transcript / People / Chat side panel. */
export function InMeeting({ room, session, selfUserId, selfName, listen, onListen, captionLang, onLeave, onEnded }: InMeetingProps) {
  const toast = useToast();
  const parts = useParticipants(room.id);
  const roomAction = useRoomAction(room.id);
  const langs = useUpdateMyLanguages(room.id);
  const partAction = useParticipantAction(room.id);
  const isHost = !!selfUserId && (room.effectiveHostId === selfUserId || room.hostId === selfUserId);
  const [panelOpen, setPanelOpen] = useState(wideScreen);
  const [tab, setTab] = useState<PanelTab>('transcript');
  const [captionsOn, setCaptionsOn] = useState(true);
  const [rawLines, setLines] = useState<CaptionLine[]>([]);
  const [stamps, setStamps] = useState<Record<string, number>>({});
  const [unread, setUnread] = useState(0);
  const [hands, setHands] = useState<Record<string, boolean>>({});
  const [floating, setFloating] = useState<FloatingReaction[]>([]);
  const [confirmEnd, setConfirmEnd] = useState(false);
  const [confirmLeave, setConfirmLeave] = useState(false);
  const inputs = useInputDevices(!!session.switchDevice);
  const [picked, setPicked] = useState<{ audioinput?: string; videoinput?: string }>({});
  const seq = useRef(0);
  const reactionSeq = useRef(0);
  const view = useRef({ panelOpen, tab });
  view.current = { panelOpen, tab };

  const chat = useMeetingChat({
    roomId: room.id,
    session,
    selfUserId,
    selfName,
    panelOpen,
    onIncoming: () => { if (!view.current.panelOpen || view.current.tab !== 'chat') setUnread((n) => n + 1); },
  });
  const hideChat = useCallback((id: string) => {
    chat.hideMessage(id).catch((e: unknown) => toast({ tone: 'danger', title: 'Could not hide the message', description: errMsg(e) }));
  }, [chat, toast]);

  const local = session.participants.find((p) => p.isLocal);
  const localId = local?.identity ?? selfUserId ?? 'me';
  const handRaised = !!hands[localId];

  // Captions: the pipeline publishes JSON on topic "captions".
  const { subscribe } = session;
  useEffect(() => subscribe('captions', (m) => {
    const line = parseCaption(m.payload, senderLabel(m.from) || 'Speaker', ++seq.current);
    if (line) {
      setLines((cur) => mergeCaption(cur, line));
      setStamps((m) => (m[line.id] ? m : { ...m, [line.id]: Date.now() }));
    }
  }), [subscribe]);

  useEffect(() => subscribe('reactions', (m) => {
    const r = parseReaction(m.payload);
    if (!r || !m.from) return;
    const from = m.from;
    if (r.t === 'hand') {
      setHands((h) => ({ ...h, [from.identity]: r.raised }));
      return;
    }
    const id = ++reactionSeq.current;
    setFloating((f) => [...f.slice(-7), { id, emoji: r.emoji, name: from.name || from.identity }]);
    setTimeout(() => setFloating((f) => f.filter((x) => x.id !== id)), REACTION_MS);
  }), [subscribe]);

  // Tell people who arrive later that our hand is up.
  const count = session.participants.length;
  useEffect(() => {
    if (handRaised && count > 1) void session.send('reactions', { t: 'hand', raised: true }).catch(() => undefined);
    // Only the head-count matters here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [count]);

  const guard = useCallback(async (fn: () => Promise<void>, what: string) => {
    try { await fn(); } catch (e) { toast({ tone: 'danger', title: `${what} unavailable`, description: e instanceof Error ? e.message : 'Permission denied' }); }
  }, [toast]);

  function toggleHand() {
    const raised = !handRaised;
    setHands((h) => ({ ...h, [localId]: raised }));
    void session.send('reactions', { t: 'hand', raised }).catch(() => undefined);
  }
  function react(emoji: string) {
    const id = ++reactionSeq.current;
    setFloating((f) => [...f.slice(-7), { id, emoji, name: 'You' }]);
    setTimeout(() => setFloating((f) => f.filter((x) => x.id !== id)), REACTION_MS);
    void session.send('reactions', { t: 'reaction', emoji }).catch(() => undefined);
  }
  const me = useMemo(() => (parts.data ?? []).find((p) => !!selfUserId && p.userId === selfUserId), [parts.data, selfUserId]);
  const speak = me?.speakLanguage ?? room.sourceLanguage;
  // Mid-meeting listen switch: record it on the participant first (routes are re-planned server side), then move the
  // `dub-{lang}` subscription. "Original audio" is a local choice only: the server has no language code for it.
  async function changeListen(l: string) {
    if (l === listen) return;
    if (l === 'original') { onListen(l); return; }
    try {
      // One language per person: hearing a language also means speaking it (unless listen-only).
      await langs.mutateAsync(speak === 'none' ? { listenLanguage: l } : { speakLanguage: l, listenLanguage: l });
      onListen(l);
      toast({ tone: 'success', title: `Your language is now ${languageLabel(l)}` });
    } catch (e) { toast({ tone: 'danger', title: 'Could not change the listen language', description: errMsg(e) }); }
  }

  const roster = useMemo(() => (parts.data ?? []).filter((p) => SHOWN.includes(p.status)), [parts.data]);
  // First time each line reached this screen: shown as the message time in the transcript.
  const lines = useMemo<TimedCaptionLine[]>(() => resolveSpeakers(rawLines, parts.data ?? []).map((l) => ({ ...l, receivedAt: stamps[l.id] })), [rawLines, parts.data, stamps]);
  const stream = useSuggestionStream(room.id, captionLang);
  const triage = useSuggestionTriage(stream.items);
  const followUps = useFollowUps(room.id, stream.status !== 'unavailable');
  const suggestions = stream.status === 'unavailable' ? undefined : { items: triage.visible, followUps: followUps.data ?? [], onDismiss: triage.dismiss, onSnooze: triage.snooze };
  const byUser = useMemo(() => new Map(roster.filter((p) => p.userId).map((p) => [p.userId as string, p])), [roster]);
  const tiles = useMemo<TileInfo[]>(() => session.participants.map((p) => {
    const r = byUser.get(baseIdentity(p.identity));
    return { p, host: p.identity === room.effectiveHostId, hand: !!hands[p.identity], langs: r && r.speakLanguage !== r.listenLanguage ? `${r.speakLanguage} → ${r.listenLanguage}` : undefined, lang: r?.speakLanguage };
  }), [session.participants, byUser, hands, room.effectiveHostId]);
  const rows = useMemo<PersonRow[]>(() => roster.map((p) => {
    const live = p.userId ? session.participants.find((s) => baseIdentity(s.identity) === p.userId) : undefined;
    return { p, isSelf: !!p.userId && p.userId === selfUserId, host: !!p.userId && p.userId === room.effectiveHostId, inCall: live ? true : undefined, micOn: live?.micOn, hand: live ? !!hands[live.identity] : false };
  }), [roster, session.participants, selfUserId, room.effectiveHostId, hands]);
  const lobby = rows.filter((r) => r.p.status === 'WAITING');

  async function moderate(pid: string, action: 'admit' | 'kick') {
    try { await partAction.mutateAsync({ pid, action }); } catch (e) { toast({ tone: 'danger', title: action === 'admit' ? 'Could not admit' : 'Could not deny', description: errMsg(e) }); }
  }
  async function admitAll() {
    for (const r of lobby) await moderate(r.p.id, 'admit');
  }
  async function hostAction(a: Parameters<typeof roomAction.mutate>[0]) {
    try {
      await roomAction.mutateAsync(a);
      toast({ tone: 'success', title: { start: 'Translation started', pause: 'Translation paused', resume: 'Translation resumed', 'translation/stop': 'Translation stopped', end: 'Meeting ended', cancel: 'Cancelled' }[a] });
    } catch (e) { toast({ tone: 'danger', title: 'Action failed', description: errMsg(e) }); }
  }

  function openPanel(t?: PanelTab) {
    if (t) setTab(t);
    setPanelOpen(true);
  }
  async function invite() {
    const text = `Join my WarpTalk meeting: ${window.location.origin}/join (room code ${room.roomCode})`;
    try { await navigator.clipboard.writeText(text); toast({ tone: 'success', title: 'Invite copied' }); } catch { toast({ tone: 'danger', title: 'Could not copy the invite' }); }
  }
  useEffect(() => { if (panelOpen && tab === 'chat') setUnread(0); }, [panelOpen, tab]);

  const last = [...lines].reverse().find((l) => l.text || Object.keys(l.translations).length);
  const overlayText = last ? pickTranslation(last, captionLang) || last.text : '';
  const connection = session.connection;
  const devices: DeviceMenus | undefined = session.switchDevice ? {
    ...inputs,
    mic: picked.audioinput ?? session.activeDevice?.('audioinput'),
    cam: picked.videoinput ?? session.activeDevice?.('videoinput'),
    onPick: (kind, id) => {
      const sw = session.switchDevice;
      if (!sw) return;
      sw(kind, id).then(() => setPicked((p) => ({ ...p, [kind]: id })))
        .catch((e: unknown) => toast({ tone: 'danger', title: kind === 'audioinput' ? 'Could not switch the microphone' : 'Could not switch the camera', description: errMsg(e) }));
    },
  } : undefined;
  // The room API has no recording state yet: the REC pill and Record button stay inert until it does.
  const recording = false;

  return (
    <section aria-label="Meeting" data-testid="meeting-room" className="relative flex h-full min-h-0 flex-col text-ink">
      <h1 className="sr-only">{room.title}</h1>
      <div className="relative flex min-h-0 flex-1 gap-3 p-3 pb-0">
        <div className="relative flex min-w-0 flex-1 flex-col gap-3">
          <div className="relative min-h-0 flex-1 overflow-hidden rounded-2xl border border-border bg-surface-1 shadow-sm">
            <div data-testid="stage" className="absolute inset-0">
              <Stage tiles={tiles} />
            </div>
            <time className="pointer-events-auto absolute left-4 top-4 z-30 rounded-full border border-border/70 bg-surface-1/90 px-2.5 py-1 text-[12px] font-medium tabular-nums text-ink shadow-sm backdrop-blur" aria-label="Meeting duration">
              <Elapsed since={room.startedAt} />
            </time>
            {!panelOpen && (
              <button type="button" onClick={() => openPanel()} aria-label="Open side panel" title="Open side panel" className="absolute right-4 top-4 z-30 flex size-8 items-center justify-center rounded-full border border-border/70 bg-surface-1/90 text-ink-muted shadow-sm backdrop-blur hover:text-ink focus-ring">
                <SidebarSimple className="size-4 -scale-x-100" aria-hidden />
              </button>
            )}
            {recording && (
              <span data-testid="rec-pill" className="absolute left-4 top-14 z-30 flex items-center gap-1.5 rounded-full border border-border/70 bg-surface-1/90 px-2.5 py-1 text-[12px] font-medium text-red-600 shadow-sm backdrop-blur">
                <span aria-hidden className="size-2 animate-pulse rounded-full bg-red-500" />REC
              </span>
            )}
            <div className="pointer-events-none absolute inset-x-4 top-4 z-10 flex items-center justify-center gap-2 pl-24">
              {/* Status is not shown on the stage (only the reconnecting banner below); kept for screen readers. */}
              <span className="sr-only">
                <StatusStrip compact connection={connection} quality={session.quality} room={room} />
              </span>
              {(connection === 'reconnecting' || connection === 'connecting') && (
                <div role="status" data-testid="reconnecting-banner" className="pointer-events-auto flex h-7 items-center gap-2 rounded-full bg-amber-50 px-3 text-[12px] font-medium text-amber-800 shadow-sm">
                  <ArrowsClockwise className="size-3.5 shrink-0 animate-spin" aria-hidden />
                  {connection === 'connecting' ? 'Connecting to the meeting…' : 'Connection lost. Reconnecting, audio resumes automatically…'}
                </div>
              )}
            </div>
            {floating.length > 0 && (
              <ul aria-label="Reactions" className="pointer-events-none absolute bottom-4 left-4 z-30 flex flex-col gap-1.5">
                {floating.map((f) => (
                  <li key={f.id} data-testid="reaction" className="flex animate-slide-up items-center gap-2 rounded-pill bg-black/55 px-2.5 py-1 text-body-sm text-white backdrop-blur">
                    <span className="text-title" aria-hidden>{f.emoji}</span>{f.name}<span className="sr-only"> reacted {f.emoji}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
          {captionsOn && (
            <CaptionLane lines={lines} lang={captionLang} overlay={overlayText && last ? { speaker: last.speaker, text: overlayText } : null} onOpen={() => openPanel('transcript')} />
          )}
        </div>
        <section
          aria-label="Meeting panel"
          className={cn(
            'z-20 min-h-0 flex-col overflow-hidden rounded-2xl border border-border bg-surface-1 shadow-sm',
            'max-lg:fixed max-lg:bottom-24 max-lg:right-3 max-lg:top-3 max-lg:w-[min(360px,calc(100vw-24px))] max-lg:shadow-pop lg:flex lg:w-[300px] lg:shrink-0 xl:w-[340px]',
            panelOpen ? 'flex' : 'hidden',
          )}
        >
          <SidePanel
            className="min-h-0 flex-1"
            tab={tab === 'suggestions' && !suggestions ? 'transcript' : tab}
            onTab={setTab}
            lines={lines}
            translationLang={captionLang}
            rows={rows}
            canModerate={isHost}
            onAdmit={(id) => void moderate(id, 'admit')}
            onDeny={(id) => void moderate(id, 'kick')}
            onAdmitAll={() => void admitAll()}
            moderating={partAction.isPending}
            roomCode={room.roomCode}
            onInvite={() => void invite()}
            chat={{
              messages: chat.messages,
              lang: captionLang,
              onSend: chat.send,
              onRetry: chat.retry,
              onDismiss: chat.dismiss,
              onHide: isHost ? hideChat : undefined,
              hasOlder: chat.hasOlder,
              loadingOlder: chat.loadingOlder,
              onLoadOlder: () => void chat.loadOlder(),
              loadError: chat.loadError,
            }}
            unread={unread}
            suggestions={suggestions}
            onCollapse={() => setPanelOpen(false)}
          />
        </section>
      </div>
      <div className="flex shrink-0 justify-center px-3 py-3">
        <ControlBar
          micOn={!!local?.micOn}
          camOn={!!local?.camOn}
          sharing={!!local?.sharing}
          canShare={session.canShare}
          onMic={() => void guard(session.toggleMic, 'Microphone')}
          onCamera={() => void guard(session.toggleCamera, 'Camera')}
          onShare={() => void guard(session.toggleShare, 'Screen sharing')}
          captionsOn={captionsOn}
          onCaptions={() => setCaptionsOn((v) => !v)}
          listen={listen}
          onListen={(l) => void changeListen(l)}
          languages={roomLanguages(room)}
          languagesBusy={langs.isPending}
          devices={devices}
          handRaised={handRaised}
          onHand={toggleHand}
          onReact={react}
          recording={recording}
          host={isHost ? { status: room.status, translationActive: room.translationActive, busy: roomAction.isPending, lobby: lobby.length, onAction: (a) => void hostAction(a), onAdmitAll: () => void admitAll() } : null}
          onLeave={() => (isHost ? setConfirmLeave(true) : onLeave())}
          onEnd={() => setConfirmEnd(true)}
        />
      </div>
      <LeaveDialog open={confirmLeave} onOpenChange={setConfirmLeave} isHost={isHost} onLeave={onLeave} />
      <EndMeetingDialog roomId={room.id} open={confirmEnd} onOpenChange={setConfirmEnd} onEnded={onEnded} />
    </section>
  );
}

function Elapsed({ since }: { since?: string | null }) {
  const [start] = useState(() => (since ? new Date(since).getTime() : Date.now()));
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => { const id = setInterval(() => setNow(Date.now()), 1000); return () => clearInterval(id); }, []);
  const s = Math.max(0, Math.floor((now - start) / 1000));
  const pad = (n: number) => String(n).padStart(2, '0');
  return <>{pad(Math.floor(s / 3600))}:{pad(Math.floor((s % 3600) / 60))}:{pad(s % 60)}</>;
}

/** Caption strip under the stage: the last few lines, newest at the bottom. */
function CaptionLane({ lines, lang, overlay, onOpen }: { lines: CaptionLine[]; lang: string; overlay: { speaker: string; text: string } | null; onOpen: () => void }) {
  const recent = lines.filter((l) => l.text || Object.keys(l.translations).length).slice(-3);
  const wantsTr = lang !== 'original';
  return (
    <div className="relative flex h-[clamp(96px,15vh,148px)] shrink-0 flex-col justify-end overflow-hidden rounded-2xl bg-surface-2/60 px-10 py-2 text-center">
      {overlay ? (
        <div aria-hidden data-testid="caption-overlay" className="flex flex-col items-center gap-1.5">
          {recent.map((l, i) => {
            const tr = pickTranslation(l, lang);
            const older = i < recent.length - 1;
            return (
              <div key={l.id} className={cn('flex max-w-full flex-col items-center leading-snug text-ink', older && 'opacity-55')}>
                <p className="flex items-center justify-center gap-1.5 text-[13px]">
                  <ParticipantAvatar name={l.speaker} lang={l.lang} size="xs" />
                  <span className="min-w-0"><span className="font-bold">{l.speaker}:</span> {l.text || tr}</span>
                </p>
                {wantsTr && tr && l.text && tr !== l.text && <p className="text-[15px] font-medium">{tr}</p>}
              </div>
            );
          })}
        </div>
      ) : (
        <p aria-hidden data-testid="caption-overlay" className="pb-2 text-center text-[13px] text-ink-subtle">Captions will appear here when someone speaks.</p>
      )}
      <button type="button" onClick={onOpen} aria-label="Open transcript" title="Open transcript" className="absolute right-1.5 top-1.5 flex size-7 items-center justify-center rounded-full text-ink-muted hover:bg-surface-3/60 hover:text-ink focus-ring">
        <ListBullets className="size-4" aria-hidden />
      </button>
    </div>
  );
}
