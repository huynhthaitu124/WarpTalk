import { useEffect, useRef, useState, type FormEvent } from 'react';
import { ArrowClockwise, Check, Crown, EyeSlash, Hand, Microphone, MicrophoneSlash, PaperPlaneRight, Translate, X } from '@phosphor-icons/react';
import { Avatar, Badge, Button, Input, StatusPill, Tabs, TabsContent, TabsList, TabsTrigger } from '@/ui';
import { cn } from '@/lib/cn';
import { CHAT_MAX_CHARS } from '@/lib/chat';
import { languageLabel } from '@/lib/languages';
import type { Participant } from '@/lib/schemas';
import type { CaptionLine } from '@/lib/captions';
import { CaptionList } from '../MeetingParts';
import type { ChatMessage } from './protocol';
import { speakerColor } from './speakers';

export type PanelTab = 'transcript' | 'people' | 'chat';

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
  return (
    <li data-testid="person" data-status={p.status} className="flex min-h-12 items-center gap-3 px-4 py-2">
      <Avatar name={p.displayName} size="md" />
      <div className="min-w-0 flex-1">
        <p className="flex items-center gap-1.5 text-body font-medium">
          <span className="truncate">{p.displayName}{r.isSelf ? ' (you)' : ''}</span>
          {r.host && <Crown weight="fill" className="size-3.5 shrink-0 text-warning-text" role="img" aria-label="Host" />}
        </p>
        <p className="truncate text-caption text-ink-subtle">{languageLabel(p.speakLanguage)} → {languageLabel(p.listenLanguage)}</p>
      </div>
      {r.hand && <Hand weight="fill" className="size-4 shrink-0 text-warning-text" role="img" aria-label="Hand raised" />}
      {r.inCall && (r.micOn ? <Microphone className="size-4 shrink-0 text-ink-subtle" role="img" aria-label="Microphone on" /> : <MicrophoneSlash className="size-4 shrink-0 text-ink-subtle" role="img" aria-label="Microphone off" />)}
      {waiting && canModerate ? (
        <div className="flex shrink-0 gap-1">
          <Button size="sm" variant="primary" disabled={busy} onClick={() => onAdmit(p.id)} aria-label={`Admit ${p.displayName}`}><Check className="size-4" aria-hidden />Admit</Button>
          <Button size="sm" disabled={busy} onClick={() => onDeny(p.id)} aria-label={`Deny ${p.displayName}`}><X className="size-4" aria-hidden />Deny</Button>
        </div>
      ) : (
        <StatusPill status={r.host ? 'HOST' : p.status} className="shrink-0" />
      )}
    </li>
  );
}

function PeopleTab({ rows, canModerate, onAdmit, onDeny, onAdmitAll, busy }: { rows: PersonRow[]; canModerate: boolean; onAdmit(id: string): void; onDeny(id: string): void; onAdmitAll(): void; busy: boolean }) {
  const waiting = rows.filter((r) => r.p.status === 'WAITING');
  const inRoom = rows.filter((r) => r.p.status === 'CONNECTED');
  const others = rows.filter((r) => !['WAITING', 'CONNECTED'].includes(r.p.status));
  const group = (label: string, list: PersonRow[], testId: string, action?: React.ReactNode) => list.length > 0 && (
    <section aria-label={label} data-testid={testId}>
      <div className="flex items-center justify-between px-4 pb-1 pt-3">
        <h3 className="text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">{label} ({list.length})</h3>
        {action}
      </div>
      <ul className="divide-y divide-hairline">
        {list.map((r) => <Person key={r.p.id} r={r} canModerate={canModerate} onAdmit={onAdmit} onDeny={onDeny} busy={busy} />)}
      </ul>
    </section>
  );
  if (rows.length === 0) return <p className="p-4 text-body text-ink-subtle">Nobody is in the room yet.</p>;
  return (
    <div className="pb-3">
      {group('Waiting to be admitted', waiting, 'people-lobby', canModerate && waiting.length > 1 ? <Button size="sm" variant="ghost" disabled={busy} onClick={onAdmitAll}>Admit all</Button> : undefined)}
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

function ChatBubble({ m, lang, c }: { m: ChatMessage; lang: string; c: ChatView }) {
  const tr = m.translations[lang];
  const showTr = !!tr && tr.trim() !== m.text.trim();
  return (
    <div data-testid="chat-message" data-mine={m.mine} data-status={m.status} data-hidden={m.hidden} className={cn('flex flex-col gap-0.5', m.mine && 'items-end')}>
      <span className="flex items-center gap-1.5 text-caption text-ink-subtle">
        <span aria-hidden className="size-2 rounded-pill" style={{ background: speakerColor(m.from) }} />{m.mine ? 'You' : m.from}
        {m.hidden && <Badge tone="warning"><EyeSlash size={12} aria-hidden />Hidden from participants</Badge>}
        {m.status === 'sending' && <span>Sending…</span>}
      </span>
      <p className={cn('max-w-[85%] whitespace-pre-wrap break-words rounded-lg px-3 py-1.5 text-body', m.mine ? 'bg-primary text-on-primary' : 'bg-surface-2 text-ink', m.hidden && 'opacity-60', m.status === 'sending' && 'opacity-70')}>{m.text}</p>
      {showTr && (
        <p data-testid="chat-translation" lang={lang} className="flex max-w-[85%] items-start gap-1 whitespace-pre-wrap break-words text-body-sm text-ink-muted">
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
        <button type="button" onClick={() => c.onHide?.(m.id)} aria-label={`Hide message from ${m.mine ? 'you' : m.from}`} className="inline-flex items-center gap-1 rounded-md px-1 text-caption text-ink-subtle hover:text-ink focus-ring">
          <EyeSlash size={12} aria-hidden />Hide
        </button>
      )}
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
        <div className="flex gap-2">
          <Input aria-label="Message" placeholder={chat.disabledReason ? 'Chat is unavailable' : 'Message everyone'} value={text} onChange={(e) => setText(e.target.value)} maxLength={CHAT_MAX_CHARS} disabled={!!chat.disabledReason} autoComplete="off" />
          <Button type="submit" variant="primary" size="icon" aria-label="Send message" disabled={!!chat.disabledReason || !text.trim()}><PaperPlaneRight className="size-4" aria-hidden /></Button>
        </div>
      </form>
    </div>
  );
}

export function SidePanel({
  tab, onTab, lines, translationLang, transcriptPlaceholder, rows, canModerate, onAdmit, onDeny, onAdmitAll, moderating, chat, unread, className,
}: {
  tab: PanelTab;
  onTab(t: PanelTab): void;
  lines: CaptionLine[];
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
  className?: string;
}) {
  const lobby = rows.filter((r) => r.p.status === 'WAITING').length;
  const inRoom = rows.filter((r) => r.p.status === 'CONNECTED').length;
  return (
    <Tabs value={tab} onValueChange={(v) => onTab(v as PanelTab)} className={cn('flex min-h-0 flex-col bg-surface-1', className)} data-testid="side-panel">
      <TabsList className="shrink-0 px-2" aria-label="Meeting panel">
        <TabsTrigger value="transcript">Transcript</TabsTrigger>
        <TabsTrigger value="people" className="flex items-center gap-1.5">People<span className="text-caption text-ink-tertiary">{inRoom}<span className="sr-only"> in the meeting</span></span>{lobby > 0 && <Badge tone="warning" data-testid="lobby-badge">{lobby}<span className="sr-only"> waiting</span></Badge>}</TabsTrigger>
        <TabsTrigger value="chat" className="flex items-center gap-1.5">Chat{unread > 0 && <Badge tone="primary" data-testid="chat-unread">{unread}<span className="sr-only"> unread</span></Badge>}</TabsTrigger>
      </TabsList>
      <TabsContent value="transcript" className="mt-0 flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden">
        <CaptionList lines={lines} translationLang={translationLang} placeholder={transcriptPlaceholder} className="min-h-0 flex-1" />
      </TabsContent>
      <TabsContent value="people" className="mt-0 min-h-0 flex-1 overflow-y-auto data-[state=inactive]:hidden">
        <PeopleTab rows={rows} canModerate={canModerate} onAdmit={onAdmit} onDeny={onDeny} onAdmitAll={onAdmitAll} busy={moderating} />
      </TabsContent>
      <TabsContent value="chat" className="mt-0 flex min-h-0 flex-1 flex-col data-[state=inactive]:hidden">
        <ChatTab chat={chat} />
      </TabsContent>
    </Tabs>
  );
}
