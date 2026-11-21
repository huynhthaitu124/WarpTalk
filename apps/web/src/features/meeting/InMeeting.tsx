import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { ArrowsClockwise, X } from '@phosphor-icons/react';
import { Badge, Button, useToast } from '@/ui';
import { cn } from '@/lib/cn';
import { mergeCaption, parseCaption, pickTranslation, type CaptionLine } from '@/lib/captions';
import { useParticipantAction, useParticipants, useRoomAction } from '@/lib/queries';
import type { Room } from '@/lib/schemas';
import { StatusStrip } from '../MeetingParts';
import { errMsg } from '../errors';
import { ControlBar } from './ControlBar';
import { EndMeetingDialog, LeaveDialog } from './Dialogs';
import { roomLanguages } from './PreJoin';
import { parseChat, parseReaction, type ChatMessage } from './protocol';
import { SidePanel, type PanelTab, type PersonRow } from './SidePanel';
import { Stage, type TileInfo } from './Stage';
import type { MeetingSession } from './types';

const SHOWN = ['CONNECTED', 'WAITING', 'DISCONNECTED', 'INVITED'];
const REACTION_MS = 4000;

interface FloatingReaction { id: number; emoji: string; name: string }

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
  const partAction = useParticipantAction(room.id);
  const isHost = !!selfUserId && (room.effectiveHostId === selfUserId || room.hostId === selfUserId);
  const [panelOpen, setPanelOpen] = useState(wideScreen);
  const [tab, setTab] = useState<PanelTab>('transcript');
  const [captionsOn, setCaptionsOn] = useState(true);
  const [lines, setLines] = useState<CaptionLine[]>([]);
  const [chat, setChat] = useState<ChatMessage[]>([]);
  const [unread, setUnread] = useState(0);
  const [hands, setHands] = useState<Record<string, boolean>>({});
  const [floating, setFloating] = useState<FloatingReaction[]>([]);
  const [confirmEnd, setConfirmEnd] = useState(false);
  const [confirmLeave, setConfirmLeave] = useState(false);
  const seq = useRef(0);
  const reactionSeq = useRef(0);
  const view = useRef({ panelOpen, tab });
  view.current = { panelOpen, tab };

  const local = session.participants.find((p) => p.isLocal);
  const localId = local?.identity ?? selfUserId ?? 'me';
  const handRaised = !!hands[localId];

  // Captions: the pipeline publishes JSON on topic "captions".
  const { subscribe } = session;
  useEffect(() => subscribe('captions', (m) => {
    const line = parseCaption(m.payload, m.from?.name || m.from?.identity || 'Speaker', ++seq.current);
    if (line) setLines((cur) => mergeCaption(cur, line));
  }), [subscribe]);

  useEffect(() => subscribe('chat', (m) => {
    const c = parseChat(m.payload);
    if (!c) return;
    setChat((cur) => (cur.some((x) => x.id === c.id) ? cur : [...cur, { id: c.id, from: m.from?.name || m.from?.identity || 'Someone', fromIdentity: m.from?.identity, text: c.text, ts: c.ts, mine: false }]));
    if (!view.current.panelOpen || view.current.tab !== 'chat') setUnread((n) => n + 1);
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
  function sendChat(text: string) {
    const msg = { id: `${localId}-${Date.now()}-${++seq.current}`, text, ts: Date.now() };
    setChat((cur) => [...cur, { id: msg.id, from: selfName, fromIdentity: localId, text, ts: msg.ts, mine: true }]);
    void session.send('chat', msg).catch((e: unknown) => toast({ tone: 'danger', title: 'Message not sent', description: errMsg(e) }));
  }

  const roster = useMemo(() => (parts.data ?? []).filter((p) => SHOWN.includes(p.status)), [parts.data]);
  const byUser = useMemo(() => new Map(roster.filter((p) => p.userId).map((p) => [p.userId as string, p])), [roster]);
  const tiles = useMemo<TileInfo[]>(() => session.participants.map((p) => {
    const r = byUser.get(p.identity);
    return { p, host: p.identity === room.effectiveHostId, hand: !!hands[p.identity], langs: r && r.speakLanguage !== r.listenLanguage ? `${r.speakLanguage} → ${r.listenLanguage}` : undefined };
  }), [session.participants, byUser, hands, room.effectiveHostId]);
  const rows = useMemo<PersonRow[]>(() => roster.map((p) => {
    const live = p.userId ? session.participants.find((s) => s.identity === p.userId) : undefined;
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
  function togglePanel() {
    if (panelOpen) setPanelOpen(false); else openPanel();
  }
  useEffect(() => { if (panelOpen && tab === 'chat') setUnread(0); }, [panelOpen, tab]);

  const last = [...lines].reverse().find((l) => l.text || Object.keys(l.translations).length);
  const overlayText = last ? pickTranslation(last, captionLang) || last.text : '';
  const connection = session.connection;

  return (
    <section aria-label="Meeting" data-testid="meeting-room" className="fixed inset-0 z-[45] flex flex-col bg-canvas text-ink">
      <header className="flex min-h-12 shrink-0 flex-wrap items-center gap-x-4 gap-y-1 border-b border-hairline px-4 py-2 max-sm:px-3">
        <h1 className="min-w-0 max-w-[40%] truncate text-title font-semibold max-sm:max-w-[55%] max-sm:text-body">{room.title}</h1>
        <div className="hidden md:block"><StatusStrip connection={connection} quality={session.quality} room={room} /></div>
        <Badge tone={connection === 'connected' ? 'success' : connection === 'disconnected' ? 'danger' : 'warning'} data-testid="status-connection-compact" className="md:hidden">{connection === 'connected' ? 'Connected' : connection === 'reconnecting' ? 'Reconnecting' : connection === 'connecting' ? 'Connecting' : 'Disconnected'}</Badge>
        <div className="ml-auto" />
      </header>
      {(connection === 'reconnecting' || connection === 'connecting') && (
        <div role="status" data-testid="reconnecting-banner" className="flex shrink-0 items-center justify-center gap-2 bg-warning-soft px-4 py-2 text-body-sm text-warning-text">
          <ArrowsClockwise className="size-4 shrink-0 animate-spin" aria-hidden />
          {connection === 'connecting' ? 'Connecting to the meeting…' : 'Connection lost. Reconnecting, audio resumes automatically…'}
        </div>
      )}
      <div className="relative flex min-h-0 flex-1">
        <div className="relative min-w-0 flex-1">
          <div className="absolute inset-0 overflow-y-auto p-2 sm:p-4" data-testid="stage">
            <Stage tiles={tiles} />
          </div>
          {floating.length > 0 && (
            <ul aria-label="Reactions" className="pointer-events-none absolute bottom-16 left-3 flex flex-col gap-1.5">
              {floating.map((f) => (
                <li key={f.id} data-testid="reaction" className="flex animate-slide-up items-center gap-2 rounded-pill bg-black/75 px-2.5 py-1 text-body-sm text-white">
                  <span className="text-title" aria-hidden>{f.emoji}</span>{f.name}<span className="sr-only"> reacted {f.emoji}</span>
                </li>
              ))}
            </ul>
          )}
          {captionsOn && overlayText && last && (
            <div aria-hidden data-testid="caption-overlay" className="pointer-events-none absolute inset-x-3 bottom-3 flex justify-center">
              <p className="max-w-2xl rounded-lg bg-black/75 px-4 py-2 text-center text-body-lg text-white"><span className="mr-2 text-body-sm opacity-80">{last.speaker}</span>{overlayText}</p>
            </div>
          )}
        </div>
        {panelOpen && (
          <section
            aria-label="Meeting panel"
            className={cn(
              'z-10 flex min-h-0 flex-col overflow-hidden border-hairline-strong bg-surface-1',
              'max-lg:absolute max-lg:inset-x-0 max-lg:bottom-0 max-lg:h-[72%] max-lg:rounded-t-xl max-lg:border-t max-lg:shadow-pop lg:w-[360px] lg:shrink-0 lg:border-l',
            )}
          >
            <SidePanel
              className="min-h-0 flex-1"
              tab={tab}
              onTab={setTab}
              lines={lines}
              translationLang={captionLang}
              rows={rows}
              canModerate={isHost}
              onAdmit={(id) => void moderate(id, 'admit')}
              onDeny={(id) => void moderate(id, 'kick')}
              onAdmitAll={() => void admitAll()}
              moderating={partAction.isPending}
              chat={chat}
              onSend={sendChat}
              unread={unread}
            />
            <Button variant="ghost" size="icon" className="absolute right-2 top-1 lg:hidden" aria-label="Close panel" onClick={() => setPanelOpen(false)}><X className="size-4" aria-hidden /></Button>
          </section>
        )}
      </div>
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
        onListen={onListen}
        languages={roomLanguages(room)}
        handRaised={handRaised}
        onHand={toggleHand}
        onReact={react}
        panelOpen={panelOpen}
        onPanel={togglePanel}
        panelBadge={(isHost ? lobby.length : 0) + unread}
        host={isHost ? { status: room.status, translationActive: room.translationActive, busy: roomAction.isPending, lobby: lobby.length, onAction: (a) => void hostAction(a), onAdmitAll: () => void admitAll() } : null}
        onLeave={() => (isHost ? setConfirmLeave(true) : onLeave())}
        onEnd={() => setConfirmEnd(true)}
      />
      <LeaveDialog open={confirmLeave} onOpenChange={setConfirmLeave} isHost={isHost} onLeave={onLeave} />
      <EndMeetingDialog roomId={room.id} open={confirmEnd} onOpenChange={setConfirmEnd} onEnded={onEnded} />
    </section>
  );
}
