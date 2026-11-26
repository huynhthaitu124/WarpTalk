import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ArrowClockwise, Check, Copy, Crown, EyeSlash, HandPalm, Microphone, MicrophoneSlash, PaperPlaneRight, SidebarSimple, Translate, UserPlus, X } from '@phosphor-icons/react';
import * as RT from '@radix-ui/react-tabs';
import { Badge, Button, Input, ParticipantAvatar, StatusPill, TabsContent, useToast } from '@v2/ui';
import { cn } from '@v2/lib/cn';
import { CHAT_MAX_CHARS } from '@v2/lib/chat';
import { languageLabel } from '@v2/lib/languages';
import type { Participant } from '@v2/lib/schemas';
import { CaptionList, type TimedCaptionLine } from '../MeetingParts';
import type { ChatMessage } from './protocol';
import { SuggestionsTab, type SuggestionsView } from './Suggestions';
import { AskPanel } from './AskPanel';
import { ASK } from '@v2/lib/strings';
import type { Citation } from '@v2/lib/ask';

const TAB =
  'relative inline-flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap pb-2 text-[13px] font-medium text-ink-subtle transition-colors hover:text-ink focus-ring ' +
  'data-[state=active]:text-ink after:absolute after:inset-x-0 after:-bottom-px after:h-0.5 after:rounded-full after:bg-transparent data-[state=active]:after:bg-ink';

function Count({ children, tone, testid }: { children: React.ReactNode; tone?: 'primary' | 'warn'; testid?: string }) {
  return (
    <span data-testid={testid} className={cn('grid h-4 min-w-4 place-items-center rounded-full px-1 text-[10px] font-semibold tabular-nums', tone === 'primary' ? 'bg-primary text-white' : tone === 'warn' ? 'bg-amber-400 text-black' : 'bg-surface-2 text-ink-muted')}>
      {children}
    </span>
  );
}

export type PanelTab = 'transcript' | 'people' | 'chat' | 'suggestions' | 'ask';

export interface PersonRow {
  p: Participant;
  isSelf: boolean;
  host: boolean;
  /** In the LiveKit room right now (undefined before connecting). */
  inCall?: boolean;
  micOn?: boolean;
  hand?: boolean;
}

function Person({ r, canModerate, onAdmit, onDeny, busy }: { r: PersonRow; canModerate: boolean; onAdmit(id: string): void; onDeny(id: string): void; busy: boolean }) {
  const { p } = r;
  const waiting = p.status === 'WAITING';
  const chip = 'flex size-6 shrink-0 items-center justify-center rounded-md bg-surface-2 text-ink-muted';
  return (
    <li data-testid="person" data-status={p.status} className="flex min-h-11 items-center gap-2.5 rounded-md px-2 py-1.5 hover:bg-surface-2">
      <ParticipantAvatar name={p.displayName} lang={p.speakLanguage} size="sm" />
      <div className="min-w-0 flex-1">
        <p className="flex items-center gap-1.5 text-[13px] font-medium">
          <span className="truncate">{p.displayName}{r.isSelf ? ' (you)' : ''}</span>
          {r.host && <Crown weight="fill" className="size-3.5 shrink-0 text-warning-text" role="img" aria-label="Host" />}
        </p>
        <p className="truncate text-[11px] text-ink-subtle">{r.host ? 'Host' : 'Participant'} · {languageLabel(p.speakLanguage)} → {languageLabel(p.listenLanguage)}</p>
      </div>
      {r.hand && <HandPalm weight="fill" className="size-4 shrink-0 text-warning-text" role="img" aria-label="Hand raised" />}
      {r.inCall && (
        <span className={chip}>
          {r.micOn ? <Microphone className="size-3.5" role="img" aria-label="Microphone on" /> : <MicrophoneSlash className="size-3.5 text-red-600" role="img" aria-label="Microphone off" />}
        </span>
      )}
      {waiting && canModerate ? (
        <div className="flex shrink-0 gap-1">
          <Button size="sm" variant="primary" disabled={busy} onClick={() => onAdmit(p.id)} aria-label={`Admit ${p.displayName}`}><Check className="size-4" aria-hidden />Admit</Button>
          <Button size="sm" disabled={busy} onClick={() => onDeny(p.id)} aria-label={`Deny ${p.displayName}`}><X className="size-4" aria-hidden />Deny</Button>
        </div>
      ) : p.status !== 'CONNECTED' ? (
        <StatusPill status={r.host ? 'HOST' : p.status} className="shrink-0" />
      ) : null}
    </li>
  );
}

function RoomCodeCard({ code, onInvite }: { code: string; onInvite?(): void }) {
  const toast = useToast();
  async function copy() {
    try { await navigator.clipboard.writeText(code); toast({ tone: 'success', title: 'Room code copied' }); } catch { toast({ tone: 'danger', title: 'Could not copy the room code' }); }
  }
  return (
    <div data-testid="room-code-card" className="mx-1 mb-1 mt-3 flex items-center gap-2 rounded-xl border border-border bg-surface-2/60 p-2.5">
      <div className="min-w-0 flex-1">
        <p className="text-[11px] font-medium uppercase tracking-wide text-ink-subtle">Room Code</p>
        <p className="truncate font-mono text-[15px] font-semibold tracking-wider text-ink">{code}</p>
      </div>
      <Button size="sm" onClick={() => void copy()} aria-label="Copy room code"><Copy className="size-4" aria-hidden />Copy</Button>
      {onInvite && <Button size="sm" variant="primary" onClick={onInvite}><UserPlus className="size-4" aria-hidden />Invite</Button>}
    </div>
  );
}

function PeopleTab({ rows, canModerate, onAdmit, onDeny, onAdmitAll, busy, roomCode, onInvite }: { rows: PersonRow[]; canModerate: boolean; onAdmit(id: string): void; onDeny(id: string): void; onAdmitAll(): void; busy: boolean; roomCode?: string; onInvite?(): void }) {
  const waiting = rows.filter((r) => r.p.status === 'WAITING');
  const inRoom = rows.filter((r) => r.p.status === 'CONNECTED');
  const others = rows.filter((r) => !['WAITING', 'CONNECTED'].includes(r.p.status));
  const group = (label: string, list: PersonRow[], testId: string) => list.length > 0 && (
    <section aria-label={label} data-testid={testId}>
      <h3 className="px-2 pb-1 pt-3 text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">{label} ({list.length})</h3>
      <ul className="space-y-0.5">
        {list.map((r) => <Person key={r.p.id} r={r} canModerate={canModerate} onAdmit={onAdmit} onDeny={onDeny} busy={busy} />)}
      </ul>
    </section>
  );
  return (
    <div className="px-2 pb-3">
      {roomCode && <RoomCodeCard code={roomCode} onInvite={onInvite} />}
      {canModerate && waiting.length > 0 && (
        <div data-testid="waiting-banner" className="mx-1 mt-3 flex items-center justify-between gap-2 rounded-md bg-amber-50 px-3 py-2 text-[13px] font-medium text-amber-800 dark:bg-amber-500/15 dark:text-amber-300">
          <span>{waiting.length} {waiting.length === 1 ? 'person is' : 'people are'} waiting</span>
          <Button size="sm" variant="primary" disabled={busy} onClick={onAdmitAll}>Admit all</Button>
        </div>
      )}
      {rows.length === 0 && <p className="p-2 pt-4 text-body text-ink-subtle">Nobody is in the room yet.</p>}
      {group('Waiting to be admitted', waiting, 'people-lobby')}
      {group('In the meeting', inRoom, 'people-in-room')}
      {group('Left or invited', others, 'people-others')}
    </div>
  );
}

export interface ChatView {
  messages: ChatMessage[];
  /** Listen language: the translation shown under each message. */
  lang: string;
  onSend(text: string): void;
  onRetry(id: string): void;
  onDismiss(id: string): void;
  /** Host only. */
  onHide?(id: string): void;
  hasOlder: boolean;
  loadingOlder: boolean;
  onLoadOlder(): void;
  loadError?: string;
  disabledReason?: string;
}

/** Chat panel used while no media session exists (the offline room view). */
export const chatUnavailable = (reason: string): ChatView => ({
  messages: [], lang: 'original', onSend: () => undefined, onRetry: () => undefined, onDismiss: () => undefined, hasOlder: false, loadingOlder: false, onLoadOlder: () => undefined, disabledReason: reason,
});

const timeOf = (ts: number) => new Date(ts).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });

function ChatBubble({ m, lang, c }: { m: ChatMessage; lang: string; c: ChatView }) {
  const tr = m.translations[lang];
  const showTr = !!tr && tr.trim() !== m.text.trim();
  return (
    <div data-testid="chat-message" data-mine={m.mine} data-status={m.status} data-hidden={m.hidden} className="flex gap-2.5">
      <ParticipantAvatar name={m.mine ? 'You' : m.from} size="sm" className="mt-0.5" />
      <div className="flex min-w-0 flex-1 flex-col gap-0.5">
        <span className="flex items-center gap-1.5 text-[11px] text-ink-subtle">
          <span className="font-semibold text-ink-muted">{m.mine ? 'You' : m.from}</span>
          <time className="font-mono">{timeOf(m.ts)}</time>
          {m.hidden && <Badge tone="warning"><EyeSlash size={12} aria-hidden />Hidden from participants</Badge>}
          {m.status === 'sending' && <span>Sending…</span>}
        </span>
        <p className={cn('whitespace-pre-wrap break-words text-[13px] text-ink', m.hidden && 'opacity-60', m.status === 'sending' && 'opacity-70')}>{m.text}</p>
        {showTr && (
          <p data-testid="chat-translation" lang={lang} className="mt-0.5 flex items-start gap-1 whitespace-pre-wrap break-words rounded-md bg-surface-2 px-2 py-1.5 text-[13px] text-ink-muted">
            <Translate size={14} className="mt-0.5 shrink-0" aria-hidden /><span><span className="sr-only">Translated to {languageLabel(lang)}: </span>{tr}</span>
          </p>
        )}
        {m.status === 'failed' && (
          <span role="alert" className="flex flex-wrap items-center gap-2 text-caption text-danger-text">
            Not sent{m.error ? `: ${m.error}` : ''}
            <Button size="sm" variant="ghost" onClick={() => c.onRetry(m.id)} aria-label="Retry sending message"><ArrowClockwise size={14} aria-hidden />Retry</Button>
            <Button size="sm" variant="ghost" onClick={() => c.onDismiss(m.id)} aria-label="Discard unsent message">Discard</Button>
          </span>
        )}
        {c.onHide && m.status === 'sent' && !m.hidden && (
          <button type="button" onClick={() => c.onHide?.(m.id)} aria-label={`Hide message from ${m.mine ? 'you' : m.from}`} className="inline-flex items-center gap-1 self-start rounded-md px-1 text-caption text-ink-subtle hover:text-ink focus-ring">
            <EyeSlash size={12} aria-hidden />Hide
          </button>
        )}
      </div>
    </div>
  );
}

function ChatTab({ chat }: { chat: ChatView }) {
  const [text, setText] = useState('');
  const end = useRef<HTMLDivElement>(null);
  const last = chat.messages[chat.messages.length - 1];
  // Stick to the bottom for new messages only (loading older history must not jump).
  useEffect(() => { end.current?.scrollIntoView?.({ block: 'end' }); }, [last?.id, last?.status]);
  function submit(e: FormEvent) {
    e.preventDefault();
    const t = text.trim();
    if (!t) return;
    chat.onSend(t);
    setText('');
  }
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div role="log" aria-live="polite" aria-label="Chat messages" data-testid="chat-log" className="min-h-0 flex-1 space-y-3 overflow-y-auto p-4">
        {chat.hasOlder && <div className="flex justify-center"><Button size="sm" variant="ghost" loading={chat.loadingOlder} onClick={chat.onLoadOlder}>Load older messages</Button></div>}
        {chat.loadError && <p role="alert" className="text-body-sm text-danger-text">Could not load chat history: {chat.loadError}</p>}
        {chat.messages.length === 0 && !chat.loadError && <p className="text-body text-ink-subtle">{chat.disabledReason ?? 'No messages yet. Say hello to everyone in the meeting.'}</p>}
        {chat.messages.map((m) => <ChatBubble key={m.id} m={m} lang={chat.lang} c={chat} />)}
        <div ref={end} />
      </div>
      <form onSubmit={submit} className="border-t border-hairline p-3">
        <div className="flex gap-2 rounded-md border border-border p-1">
          <Input aria-label="Message" placeholder={chat.disabledReason ? 'Chat is unavailable' : 'Message everyone'} value={text} onChange={(e) => setText(e.target.value)} maxLength={CHAT_MAX_CHARS} disabled={!!chat.disabledReason} autoComplete="off" />
          <Button type="submit" variant="primary" size="icon" aria-label="Send message" disabled={!!chat.disabledReason || !text.trim()}><PaperPlaneRight className="size-4" aria-hidden /></Button>
        </div>
      </form>
    </div>
  );
}

export function SidePanel({
  tab, onTab, lines, translationLang, transcriptPlaceholder, rows, canModerate, onAdmit, onDeny, onAdmitAll, moderating, chat, unread, suggestions, askRoomId, onCitation, className, roomCode, onInvite, onCollapse,
}: {
  tab: PanelTab;
  onTab(t: PanelTab): void;
  lines: TimedCaptionLine[];
  translationLang: string;
  transcriptPlaceholder?: string;
  rows: PersonRow[];
  canModerate: boolean;
  onAdmit(participantId: string): void;
  onDeny(participantId: string): void;
  onAdmitAll(): void;
  moderating: boolean;
  chat: ChatView;
  unread: number;
  /** Undefined when the backend has no suggestions endpoint: the tab is not shown. */
  suggestions?: SuggestionsView;
  /** Room whose assistant the "Ask AI" tab talks to; the tab is hidden without it. */
  askRoomId?: string;
  /** A transcript citation was clicked in the Ask AI tab. */
  onCitation?(c: Citation): void;
  className?: string;
  /** Shown in the People tab's Room Code card. */
  roomCode?: string;
  onInvite?(): void;
  /** Hides the panel (the stage shows a button to bring it back). */
  onCollapse?(): void;
}) {
  const lobby = rows.filter((r) => r.p.status === 'WAITING').length;
  const inRoom = rows.filter((r) => r.p.status === 'CONNECTED').length;
  return (
    <RT.Root value={tab} onValueChange={(v) => onTab(v as PanelTab)} className={cn('flex min-h-0 flex-col bg-surface-1', className)} data-testid="side-panel">
      <div className="flex shrink-0 items-start border-b border-hairline pr-2">
        <RT.List className="flex min-w-0 flex-1 gap-3 overflow-x-auto px-3 pb-0 pt-3 pr-10 [scrollbar-width:none] lg:pr-3" aria-label="Meeting panel">
          <RT.Trigger value="transcript" className={TAB}>Transcript</RT.Trigger>
          <RT.Trigger value="chat" className={TAB}>Chat{unread > 0 && <Count tone="primary" testid="chat-unread">{unread}<span className="sr-only"> unread</span></Count>}</RT.Trigger>
          <RT.Trigger value="people" className={TAB}>People<Count>{inRoom}<span className="sr-only"> in the meeting</span></Count>{lobby > 0 && <Count tone="warn" testid="lobby-badge">{lobby}<span className="sr-only"> waiting</span></Count>}</RT.Trigger>
          {suggestions && <RT.Trigger value="suggestions" className={TAB}>Suggestions{suggestions.items.length > 0 && <Count tone="primary" testid="suggestions-badge">{suggestions.items.length}<span className="sr-only"> new</span></Count>}</RT.Trigger>}
          {askRoomId && <RT.Trigger value="ask" className={TAB}>{ASK.title}</RT.Trigger>}
        </RT.List>
        {onCollapse && (
          <button type="button" aria-label="Collapse side panel" title="Collapse side panel" onClick={onCollapse} className="mt-2 flex size-7 shrink-0 items-center justify-center rounded-md text-ink-muted hover:bg-surface-2 hover:text-ink focus-ring">
            <SidebarSimple className="size-5 -scale-x-100" aria-hidden />
          </button>
        )}
      </div>
      <TabsContent value="transcript" className="mt-0 flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden">
        <CaptionList lines={lines} translationLang={translationLang} placeholder={transcriptPlaceholder} selfUserId={rows.find((r) => r.isSelf)?.p.userId ?? undefined} className="min-h-0 flex-1" />
      </TabsContent>
      <TabsContent value="people" className="mt-0 min-h-0 flex-1 overflow-y-auto data-[state=inactive]:hidden">
        <PeopleTab rows={rows} canModerate={canModerate} onAdmit={onAdmit} onDeny={onDeny} onAdmitAll={onAdmitAll} busy={moderating} roomCode={roomCode} onInvite={onInvite} />
      </TabsContent>
      <TabsContent value="chat" className="mt-0 flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden">
        <ChatTab chat={chat} />
      </TabsContent>
      {suggestions && (
        <TabsContent value="suggestions" className="mt-0 min-h-0 flex-1 overflow-y-auto data-[state=inactive]:hidden">
          <SuggestionsTab view={suggestions} />
        </TabsContent>
      )}
      {askRoomId && (
        <TabsContent value="ask" className="mt-0 flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden">
          <AskPanel roomId={askRoomId} onCitation={onCitation} className="min-h-0 flex-1" />
        </TabsContent>
      )}
    </RT.Root>
  );
}
