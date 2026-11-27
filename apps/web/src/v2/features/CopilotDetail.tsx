import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import {
  ArrowDown, BookOpen, Check, ClosedCaptioning, Copy, DownloadSimple, FileText, GearSix, House, MagnifyingGlass,
  ArrowSquareOut, CalendarBlank, Hash, Microphone, MicrophoneSlash, PencilSimple, PictureInPicture, Play, Sparkle, Stop, VideoCamera, X,
} from '@phosphor-icons/react';
import { Brand, Button, ParticipantAvatar, Select } from '@v2/ui';
import { cn } from '@v2/lib/cn';
import { defaultReadingLanguage, languageLabel } from '@v2/lib/languages';
import { languageOptions } from '@v2/lib/languageOptions';
import { PROVIDER_LABEL } from '@v2/lib/bridge';
import { summaryBusy, useActionItems, useRegenerateSummary, useSummary } from '@v2/lib/copilot';
import { useRooms, useUpdateRoom } from '@v2/lib/queries';
import { useWorkspaceOptional } from '@v2/lib/workspace';
import type { Room } from '@v2/lib/schemas';
import type { OverlayAsk } from '@v2/lib/native';
import type { FollowUp } from '@v2/lib/suggestions';
import { ASK_MAX_CHARS } from '@v2/lib/ask';
import type { CaptionLine, ConnectionLabel } from './MeetingParts';

/** How long a line may wait for its translation before the "Translating…" hint is dropped. */
const TRANSLATION_WAIT_MS = 12_000;
const base = (code?: string) => code?.split('-')[0]?.toLowerCase();

/**
 * Main window while the copilot pill runs: the app shell (sidebar with navigation and recent meetings), a meeting
 * header with the live controls, the transcript as a readable document (search, copy, download) and WarpBot + notes
 * on the right. Light theme while mounted; the pill window keeps its own.
 */
export function CopilotDetail({
  room, lines, translationLang, connection, captureState, ended, suggestions, followUps, ask, onAsk, onAskStop, onShowCopilot, onEnd, ending,
  micOn, onCommand,
}: {
  room: Room;
  lines: CaptionLine[];
  translationLang?: string;
  connection: ConnectionLabel;
  captureState: string;
  /** Latest native capture error (message + code), shown under the title. */
  /** Capture stopped because the call is gone: show "Meeting ended", not an error. */
  ended?: boolean;
  suggestions: { id: string; content: string; actions?: { label: string; prompt: string }[] }[];
  followUps: FollowUp[];
  ask: OverlayAsk;
  onAsk: (q: string) => void;
  onAskStop: () => void;
  onShowCopilot: () => void;
  onEnd: () => void;
  ending: boolean;
  micOn: boolean;
  /** Same commands the pill sends: start, stop, mic on|off, listen <lang>. */
  onCommand: (command: string, value?: string) => void;
}) {
  useEffect(() => {
    const root = document.documentElement;
    const prev = root.dataset.theme;
    root.dataset.theme = 'light';
    return () => { if (prev) root.dataset.theme = prev; else delete root.dataset.theme; };
  }, []);

  const running = captureState === 'running' || captureState === 'starting';
  const [side, setSide] = useState<'warpbot' | 'notes'>('warpbot');

  return (
    <div className="fixed inset-0 z-10 flex bg-canvas text-ink" data-testid="copilot-host">
      <AppSidebar room={room} running={running} />
      <div className="flex min-w-0 flex-1 flex-col">
        <MeetingHeader
          room={room} captureState={captureState} ended={ended} connection={connection} running={running} micOn={micOn}
          translationLang={translationLang} onCommand={onCommand} onShowCopilot={onShowCopilot} onEnd={onEnd} ending={ending}
        />
        <div className="flex min-h-0 flex-1">
          <TranscriptPane lines={lines} translationLang={translationLang} title={room.title} running={running} ended={ended} />
          <aside aria-label="WarpBot and notes" className="hidden w-[380px] shrink-0 flex-col border-l border-hairline bg-surface-1 lg:flex xl:w-[420px]">
            <div role="tablist" className="flex h-12 shrink-0 items-end gap-1 border-b border-hairline px-3">
              <Tab active={side === 'warpbot'} onClick={() => setSide('warpbot')}>
                <Sparkle size={14} weight="fill" className="text-primary" />WarpBot{suggestions.length ? <Count n={suggestions.length} /> : null}
              </Tab>
              <Tab active={side === 'notes'} onClick={() => setSide('notes')}>Notes{followUps.length ? <Count n={followUps.length} /> : null}</Tab>
            </div>
            {side === 'warpbot'
              ? <WarpBotPanel ask={ask} suggestions={suggestions} onAsk={onAsk} onAskStop={onAskStop} />
              : <NotesPanel roomId={room.id} hasTalk={lines.length > 0} followUps={followUps} />}
          </aside>
        </div>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ sidebar */

function AppSidebar({ room, running }: { room: Room; running: boolean }) {
  const ws = useWorkspaceOptional()?.current ?? null;
  const rooms = useRooms(ws?.id);
  const slug = ws?.slug;
  const recent = (rooms.data?.items ?? []).slice(0, 8);
  // The main window is the capture host: leaving it ends the live translation.
  const leave = (e: React.MouseEvent) => {
    if (running && !window.confirm('Leaving this page stops the live translation. Continue?')) e.preventDefault();
  };
  const nav: { icon: ReactNode; label: string; href: string }[] = slug ? [
    { icon: <House size={16} />, label: 'Home', href: `/${slug}/home` },
    { icon: <VideoCamera size={16} />, label: 'Meetings', href: `/${slug}/rooms` },
    { icon: <FileText size={16} />, label: 'Documents', href: `/${slug}/documents` },
    { icon: <BookOpen size={16} />, label: 'Glossary', href: `/${slug}/glossary` },
    { icon: <GearSix size={16} />, label: 'Settings', href: `/${slug}/settings` },
  ] : [];

  return (
    <nav aria-label="Primary" className="hidden w-[232px] shrink-0 flex-col border-r border-hairline bg-surface-2/50 md:flex">
      <div className="flex h-12 items-center gap-2 px-4">
        <Brand />
      </div>
      {ws && (
        <div className="mx-3 mb-3 flex items-center gap-2 rounded-lg px-2 py-1.5">
          <span className="grid size-6 place-items-center rounded-md bg-ink text-[11px] font-semibold text-canvas">{ws.name.slice(0, 1).toUpperCase()}</span>
          <span className="truncate text-[13px] font-medium">{ws.name}</span>
        </div>
      )}
      <div className="flex flex-col gap-px px-2">
        {nav.map((n) => (
          <a key={n.href} href={n.href} onClick={leave} className="flex h-[30px] items-center gap-2.5 rounded-[8px] px-2 text-[13px] font-medium text-ink/85 no-underline transition-colors hover:bg-surface-3/60 hover:text-ink">
            <span className="text-ink-muted">{n.icon}</span>{n.label}
          </a>
        ))}
      </div>
      <p className="mt-6 px-4 pb-1 text-[11px] font-semibold uppercase tracking-wide text-ink-subtle">Recent meetings</p>
      <div className="flex min-h-0 flex-1 flex-col gap-px overflow-y-auto px-2 pb-3">
        {[room, ...recent.filter((x) => x.id !== room.id)].slice(0, 8).map((x) => {
          const here = x.id === room.id;
          return (
            <a
              key={x.id}
              href={here || !slug ? undefined : `/${slug}/rooms/${x.id}`}
              onClick={here ? undefined : leave}
              aria-current={here ? 'page' : undefined}
              className={cn(
                'flex h-[30px] items-center gap-2 rounded-[8px] px-2 text-[13px] no-underline transition-colors',
                here ? 'bg-surface-3 font-medium text-ink' : 'text-ink/80 hover:bg-surface-3/60 hover:text-ink',
              )}
            >
              <span className={cn('size-1.5 shrink-0 rounded-full', here && running ? 'bg-success animate-pulse' : x.status === 'IN_PROGRESS' ? 'bg-success' : 'bg-ink-tertiary/50')} />
              <span className="truncate">{x.title || 'Meeting'}</span>
            </a>
          );
        })}
      </div>
    </nav>
  );
}

/* ------------------------------------------------------------------ header */

/** Click-to-rename title; saves through PATCH /translation-rooms/{id}. */
function EditableTitle({ room }: { room: Room }) {
  const update = useUpdateRoom(room.id);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(room.title);
  useEffect(() => { if (!editing) setDraft(room.title); }, [room.title, editing]);
  const save = () => {
    const t = draft.trim();
    setEditing(false);
    if (t && t !== room.title) update.mutate({ title: t });
  };
  if (editing) {
    return (
      <input
        autoFocus aria-label="Meeting title" value={draft} maxLength={200}
        onChange={(e) => setDraft(e.target.value)} onBlur={save}
        onKeyDown={(e) => { if (e.key === 'Enter') save(); if (e.key === 'Escape') { setDraft(room.title); setEditing(false); } }}
        className="min-w-0 flex-1 rounded-md bg-surface-2 px-1.5 text-[18px] font-semibold tracking-tight outline-none ring-1 ring-primary/40"
      />
    );
  }
  return (
    <button type="button" onClick={() => setEditing(true)} title="Rename meeting" className="group flex min-w-0 items-center gap-1.5 text-left">
      <h1 className="truncate text-[18px] font-semibold tracking-tight">{update.variables?.title ?? (room.title || 'Meeting')}</h1>
      <PencilSimple size={14} className="shrink-0 text-ink-subtle opacity-0 transition-opacity group-hover:opacity-100" />
    </button>
  );
}

/** What we know from the pasted link: provider, meeting code, join link, start time, languages. */
function MeetingInfo({ room, provider, translationLang }: { room: Room; provider?: string; translationLang?: string }) {
  const code = room.bridge?.meetingCode;
  const joinUrl = room.bridge?.joinUrl ?? undefined;
  const started = room.startedAt ?? room.createdAt;
  const when = started ? new Date(started).toLocaleString(undefined, { weekday: 'short', month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' }) : null;
  const [copied, setCopied] = useState(false);
  const copy = () => {
    if (!joinUrl) return;
    void navigator.clipboard?.writeText(joinUrl).then(() => { setCopied(true); setTimeout(() => setCopied(false), 1500); });
  };
  const chip = 'inline-flex items-center gap-1 rounded-md bg-surface-2 px-1.5 py-0.5';
  return (
    <div className="flex w-full flex-wrap items-center gap-x-2 gap-y-1 text-[12px] text-ink-subtle">
      {code && (
        <span className={cn(chip, 'max-w-[220px] font-mono')} title={code}>
          <Hash size={12} /><span className="truncate">{code}</span>
        </span>
      )}
      {joinUrl && (
        <>
          <a href={joinUrl} target="_blank" rel="noreferrer" className={cn(chip, 'text-ink-muted no-underline hover:text-ink')}>
            <ArrowSquareOut size={12} />Open in {provider ?? 'meeting app'}
          </a>
          <button type="button" onClick={copy} className={cn(chip, 'hover:text-ink')} aria-label="Copy meeting link">
            {copied ? <Check size={12} /> : <Copy size={12} />}{copied ? 'Copied' : 'Copy link'}
          </button>
        </>
      )}
      {when && <span className={chip}><CalendarBlank size={12} />{when}</span>}
      <span>Spoken {languageLabel(room.sourceLanguage)} · reading in {languageLabel(base(translationLang) ?? defaultReadingLanguage())}</span>
    </div>
  );
}

function MeetingHeader({
  room, captureState, ended, connection, running, micOn, translationLang, onCommand, onShowCopilot, onEnd, ending,
}: {
  room: Room; captureState: string; ended?: boolean; connection: ConnectionLabel; running: boolean; micOn: boolean; translationLang?: string;
  onCommand: (command: string, value?: string) => void; onShowCopilot: () => void; onEnd: () => void; ending: boolean;
}) {
  const providerKey = room.bridge?.provider as keyof typeof PROVIDER_LABEL | undefined;
  const provider = providerKey && providerKey !== 'desktop' ? PROVIDER_LABEL[providerKey] ?? providerKey : undefined;
  const elapsed = useElapsed(room.startedAt ? new Date(room.startedAt) : null);
  const state: [string, string] =
    captureState === 'running' ? ['Live', 'bg-success']
      : ended ? ['Meeting ended', 'bg-ink-tertiary']
      : captureState === 'starting' ? ['Starting…', 'bg-warning']
        : captureState === 'error' ? ['Capture error', 'bg-danger']
          : ['Paused', 'bg-ink-tertiary'];
  const langs = useMemo(() => languageOptions(), []);

  return (
    <header className="flex shrink-0 flex-wrap items-center gap-x-4 gap-y-2 border-b border-hairline bg-surface-1 px-5 py-3">
      <div className="min-w-0 flex-1">
        <p className="text-[12px] text-ink-subtle">Meetings{provider ? ` · ${provider}` : ''}</p>
        <div className="flex min-w-0 items-center gap-2.5">
          <EditableTitle room={room} />
          <span className="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-surface-2 px-2 py-0.5 text-[12px] font-medium text-ink-muted">
            <span className={cn('size-1.5 rounded-full', state[1], captureState === 'running' && 'animate-pulse')} />
            {state[0]}{elapsed ? <span className="font-mono tabular-nums text-ink-subtle">{elapsed}</span> : null}
          </span>
          {connection !== 'connected' && connection !== 'idle' && (
            <span className="shrink-0 text-[12px] text-ink-subtle">Captions {connection}…</span>
          )}
        </div>
      </div>

      <div className="flex shrink-0 items-center gap-2">
        <div className="w-[150px]">
          <Select
            aria-label="Read captions in"
            value={base(translationLang) ?? defaultReadingLanguage()}
            onValueChange={(v) => onCommand('listen', v)}
            options={langs}
          />
        </div>
        <IconButton label={micOn ? 'Mute my microphone' : 'Unmute my microphone'} onClick={() => onCommand('mic', micOn ? 'off' : 'on')} active={!micOn}>
          {micOn ? <Microphone size={16} /> : <MicrophoneSlash size={16} />}
        </IconButton>
        <IconButton label={running ? 'Pause translation' : 'Resume translation'} onClick={() => onCommand(running ? 'stop' : 'start')}>
          {running ? <Stop size={16} weight="fill" /> : <Play size={16} weight="fill" />}
        </IconButton>
        <IconButton label="Show copilot pill" onClick={onShowCopilot}><PictureInPicture size={16} /></IconButton>
        <Button variant="secondary" onClick={onEnd} disabled={ending} className="!text-danger-text">End</Button>
      </div>
      <MeetingInfo room={room} provider={provider} translationLang={translationLang} />
    </header>
  );
}

function useElapsed(start: Date | null) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!start) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [start]);
  if (!start) return '';
  const s = Math.max(0, Math.floor((now - start.getTime()) / 1000));
  const h = Math.floor(s / 3600);
  const mm = String(Math.floor((s % 3600) / 60)).padStart(2, '0');
  const ss = String(s % 60).padStart(2, '0');
  return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

/* ------------------------------------------------------------------ transcript */

function TranscriptPane({ lines, translationLang, title, running, ended }: { lines: CaptionLine[]; translationLang?: string; title: string; running: boolean; ended?: boolean }) {
  const target = base(translationLang);
  const [query, setQuery] = useState('');
  const [showOriginal, setShowOriginal] = useState(true);
  const [copied, setCopied] = useState(false);
  const [atBottom, setAtBottom] = useState(true);
  const scroller = useRef<HTMLDivElement>(null);

  // First time each line reached this screen: shown as its time, and bounds the "Translating…" hint.
  const [seen, setSeen] = useState<Record<string, number>>({});
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = Date.now();
    setSeen((prev) => {
      const missing = lines.filter((l) => !(l.id in prev));
      if (!missing.length) return prev;
      const next = { ...prev };
      for (const l of missing) next[l.id] = t;
      return next;
    });
  }, [lines]);
  // Re-evaluate the "Translating…" hint while it can still expire.
  useEffect(() => {
    if (!running) return;
    const t = window.setInterval(() => setNow(Date.now()), 2000);
    return () => window.clearInterval(t);
  }, [running]);

  const rows = useMemo(() => lines.map((l) => {
    const same = !!target && base(l.lang) === target;
    const exact = target ? l.translations[target] ?? Object.entries(l.translations).find(([k]) => base(k) === target)?.[1] : undefined;
    const other = !exact && !same ? Object.entries(l.translations)[0] : undefined;
    return { l, same, translation: exact, other, at: seen[l.id] ?? now };
  }), [lines, target, seen, now]);

  const q = query.trim().toLowerCase();
  const visible = q ? rows.filter((r) => `${r.l.speaker} ${r.l.text} ${r.translation ?? ''}`.toLowerCase().includes(q)) : rows;

  useEffect(() => {
    const el = scroller.current;
    if (el && atBottom && !q) el.scrollTop = el.scrollHeight;
  }, [lines, atBottom, q]);

  const asText = () => rows.filter((r) => r.l.final).map((r) => {
    const t = new Date(r.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
    return `[${t}] ${r.l.speaker || 'Speaker'}: ${r.l.text}${r.translation ? `\n    → ${r.translation}` : ''}`;
  }).join('\n');
  const copy = () => { void navigator.clipboard?.writeText(asText()).then(() => { setCopied(true); window.setTimeout(() => setCopied(false), 1500); }); };
  const download = () => {
    const url = URL.createObjectURL(new Blob([asText()], { type: 'text/plain;charset=utf-8' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = `${(title || 'meeting').replace(/[^\p{L}\p{N}]+/gu, '-').toLowerCase()}-transcript.txt`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <section aria-label="Transcript" className="flex min-w-0 flex-1 flex-col bg-canvas">
      <div className="flex h-12 shrink-0 items-center gap-2 border-b border-hairline px-5">
        <h2 className="mr-auto text-[14px] font-semibold">Transcript</h2>
        <label className="flex h-8 w-[220px] items-center gap-2 rounded-lg border border-hairline bg-surface-1 px-2.5 focus-within:border-hairline-strong">
          <MagnifyingGlass size={14} className="text-ink-subtle" />
          <input value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Search transcript" aria-label="Search transcript" className="min-w-0 flex-1 bg-transparent text-[13px] outline-none placeholder:text-ink-subtle" />
          {query && <button type="button" aria-label="Clear search" onClick={() => setQuery('')} className="text-ink-subtle hover:text-ink"><X size={12} /></button>}
        </label>
        <button
          type="button"
          aria-pressed={showOriginal}
          onClick={() => setShowOriginal((v) => !v)}
          className={cn('h-8 rounded-lg px-2.5 text-[13px] font-medium transition-colors', showOriginal ? 'bg-surface-3 text-ink' : 'text-ink-muted hover:bg-surface-2')}
        >
          Original
        </button>
        <IconButton label={copied ? 'Copied' : 'Copy transcript'} onClick={copy} disabled={rows.length === 0}>{copied ? <Check size={16} /> : <Copy size={16} />}</IconButton>
        <IconButton label="Download transcript" onClick={download} disabled={rows.length === 0}><DownloadSimple size={16} /></IconButton>
      </div>

      <div className="relative min-h-0 flex-1">
        <div
          ref={scroller}
          role="log"
          aria-live="polite"
          aria-label="Live captions"
          data-testid="captions-log"
          onScroll={(e) => { const el = e.currentTarget; setAtBottom(el.scrollHeight - el.scrollTop - el.clientHeight < 48); }}
          className="h-full overflow-y-auto"
        >
          {visible.length === 0 ? (
            <div className="flex h-full flex-col items-center justify-center gap-2 px-6 text-center">
              <ClosedCaptioning size={32} weight="light" className="text-ink-tertiary" />
              <p className="max-w-[280px] text-[14px] text-ink-subtle">
                {q ? 'Nothing in the transcript matches your search.' : running ? 'Listening… the transcript appears as people speak.' : ended ? 'The meeting has ended.' : 'Translation is paused. Press play to resume.'}
              </p>
            </div>
          ) : (
            <ol className="mx-auto flex max-w-[820px] flex-col px-6 py-6">
              {visible.map((r, i) => {
                const prev = visible[i - 1];
                const run = !prev || prev.l.speaker !== r.l.speaker || r.at - prev.at > 60_000;
                const waiting = !r.translation && !r.same && !r.other && r.l.final && now - r.at < TRANSLATION_WAIT_MS;
                const name = r.l.speaker || 'Speaker';
                return (
                  <li key={r.l.id} data-testid="caption-line" className={cn('grid grid-cols-[36px_minmax(0,1fr)] gap-x-3', run ? 'mt-5 first:mt-0' : 'mt-1.5')}>
                    <div>{run && <ParticipantAvatar name={name} lang={r.l.lang} size="sm" />}</div>
                    <div className={cn('min-w-0', !r.l.final && 'opacity-60')}>
                      {run && (
                        <div className="mb-0.5 flex items-baseline gap-2">
                          <span className="text-[13px] font-semibold">{name}</span>
                          <time className="font-mono text-[11px] text-ink-tertiary">{new Date(r.at).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}</time>
                        </div>
                      )}
                      {r.translation && !showOriginal ? null : (
                        <p data-testid="caption-original" className={cn('break-words text-[15px] leading-relaxed', r.translation ? 'text-ink-muted' : 'text-ink')}>
                          <Highlight text={r.l.text} q={q} />
                        </p>
                      )}
                      {r.translation && (
                        <p data-testid="caption-translation" className="break-words text-[15px] font-medium leading-relaxed text-ink">
                          <Highlight text={r.translation} q={q} />
                        </p>
                      )}
                      {r.other && (
                        <p className="break-words text-[14px] leading-relaxed text-ink-muted">
                          <span className="mr-1.5 rounded bg-surface-2 px-1 py-px font-mono text-[10px] uppercase text-ink-subtle">{base(r.other[0])}</span>
                          {r.other[1]}
                        </p>
                      )}
                      {waiting && <p className="text-[12px] italic text-ink-tertiary">Translating to {languageLabel(target ?? '')}…</p>}
                    </div>
                  </li>
                );
              })}
            </ol>
          )}
        </div>
        {!atBottom && !q && rows.length > 0 && (
          <button
            type="button"
            onClick={() => { const el = scroller.current; if (el) el.scrollTop = el.scrollHeight; setAtBottom(true); }}
            className="absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-1.5 rounded-full bg-ink px-3 py-1.5 text-[12px] font-medium text-canvas shadow-lg"
          >
            <ArrowDown size={12} weight="bold" />Jump to live
          </button>
        )}
      </div>
    </section>
  );
}

function Highlight({ text, q }: { text: string; q: string }) {
  if (!q) return <>{text}</>;
  const i = text.toLowerCase().indexOf(q);
  if (i < 0) return <>{text}</>;
  return <>{text.slice(0, i)}<mark className="rounded bg-chip-amber px-0.5 text-ink">{text.slice(i, i + q.length)}</mark>{text.slice(i + q.length)}</>;
}

/* ------------------------------------------------------------------ right panel */

function NotesPanel({ roomId, hasTalk, followUps }: { roomId: string; hasTalk: boolean; followUps: FollowUp[] }) {
  const summary = useSummary(roomId, true, false);
  const regenerate = useRegenerateSummary(roomId);
  const actions = useActionItems(roomId);
  const s = summary.data;
  const items = actions.data ?? [];
  const time = (d: Date) => d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  return (
    <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-5 py-4 text-[14px] leading-relaxed">
      <Block
        title="Summary"
        meta={summaryBusy(s) ? 'Updating…' : s?.generatedAt ? `Updated ${time(new Date(s.generatedAt))}` : undefined}
        action={<Button size="sm" variant="ghost" onClick={() => regenerate.mutate()} disabled={regenerate.isPending || summaryBusy(s) || !hasTalk}>Refresh</Button>}
      >
        {!s || (!s.short && !s.detailed) ? (
          <Empty>{summaryBusy(s) ? 'Writing the summary…' : 'No summary yet. Press Refresh once there is some talk; it is also written when the meeting ends.'}</Empty>
        ) : (
          <div className="flex flex-col gap-3">
            {s.short && <p className="font-medium">{s.short}</p>}
            {s.detailed && <p className="whitespace-pre-line text-ink-muted">{s.detailed}</p>}
            <Section title="Decisions" items={s.decisions} />
            <Section title="Open questions" items={s.openQuestions} />
          </div>
        )}
        {s?.status === 'failed' && <p className="text-danger-text">The summary could not be written{s.errorCode ? ` (${s.errorCode})` : ''}.</p>}
      </Block>
      <Block title="Action items" meta={items.length ? `${items.filter((a) => !a.done).length} open` : undefined}>
        {items.length === 0 ? <Empty>None yet.</Empty> : (
          <ul className="flex flex-col gap-1.5">
            {items.map((a) => (
              <li key={a.id} className={cn('flex gap-2', a.done && 'text-ink-subtle line-through')}>
                <span className={cn('mt-1.5 size-3 shrink-0 rounded border', a.done ? 'border-ink-tertiary bg-ink-tertiary' : 'border-hairline-strong')} />
                <span>{a.text}{a.owner ? <span className="text-ink-subtle"> · {a.owner}</span> : null}</span>
              </li>
            ))}
          </ul>
        )}
      </Block>
      <Block title="Follow-ups">
        {followUps.length === 0 ? <Empty>None yet.</Empty> : (
          <ul className="flex flex-col gap-1.5">
            {followUps.map((f) => (
              <li key={f.id}>{f.text}{f.owner || f.due ? <span className="text-ink-subtle"> · {[f.owner, f.due].filter(Boolean).join(' · ')}</span> : null}</li>
            ))}
          </ul>
        )}
      </Block>
    </div>
  );
}

function WarpBotPanel({ ask, suggestions, onAsk, onAskStop }: {
  ask: OverlayAsk; suggestions: { id: string; content: string; actions?: { label: string; prompt: string }[] }[]; onAsk: (q: string) => void; onAskStop: () => void;
}) {
  const [q, setQ] = useState('');
  const end = useRef<HTMLDivElement>(null);
  const last = ask.messages[ask.messages.length - 1];
  useEffect(() => { end.current?.scrollIntoView?.({ block: 'end' }); }, [ask.messages.length, last?.content]);
  const send = (text: string) => {
    const t = text.trim().slice(0, ASK_MAX_CHARS);
    if (!t || ask.busy) return;
    onAsk(t);
    setQ('');
  };
  const starters = ['Summarize the last few minutes', 'What should I reply?', 'Any action items so far?'];
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {suggestions.length > 0 && (
          <div className="mb-5">
            <p className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-ink-subtle">Suggested now</p>
            <ul className="flex flex-col gap-2">
              {suggestions.slice(0, 4).map((x) => (
                <li key={x.id}>
                  <button
                    type="button"
                    onClick={() => send(`Help me respond to this: ${x.content}`)}
                    disabled={ask.busy}
                    className="w-full rounded-xl border border-hairline bg-surface-2/50 px-3 py-2 text-left text-[13px] leading-relaxed transition hover:border-hairline-strong hover:bg-surface-2 disabled:opacity-60"
                  >
                    {x.content}
                  </button>
                  {x.actions && x.actions.length > 0 && (
                    <div className="mt-1.5 flex flex-wrap gap-1.5">
                      {x.actions.map((a, i) => (
                        <button
                          key={i}
                          type="button"
                          onClick={() => send(a.prompt)}
                          disabled={ask.busy}
                          className="rounded-full border border-hairline px-2.5 py-1 text-[12px] text-ink-muted transition hover:border-hairline-strong hover:text-ink disabled:opacity-60"
                        >
                          {a.label}
                        </button>
                      ))}
                    </div>
                  )}
                </li>
              ))}
            </ul>
          </div>
        )}
        {ask.messages.length === 0 ? (
          <div className="flex flex-col gap-3">
            <Empty>Ask WarpBot about the call: what was just said, a reply in the other language, or anything in your documents.</Empty>
            <div className="flex flex-wrap gap-1.5">
              {starters.map((s) => (
                <button key={s} type="button" onClick={() => send(s)} disabled={ask.busy} className="rounded-full border border-hairline px-3 py-1 text-[12px] text-ink-muted transition hover:border-hairline-strong hover:text-ink">{s}</button>
              ))}
            </div>
          </div>
        ) : (
          <ol className="flex flex-col gap-3">
            {ask.messages.map((m) => (
              <li key={m.key} className={m.role === 'user' ? 'max-w-[85%] self-end rounded-2xl rounded-br-md bg-ink-btn px-3.5 py-2 text-[14px] text-on-ink-btn' : 'max-w-full text-[14px] leading-relaxed'}>
                {m.role === 'assistant' && !m.content && m.status === 'streaming' ? <span className="text-ink-subtle">Thinking…</span> : <p className="whitespace-pre-wrap">{m.content}</p>}
                {m.error && <p className="mt-1 text-[12px] text-danger-text">{m.error}</p>}
              </li>
            ))}
          </ol>
        )}
        <div ref={end} />
      </div>
      <form className="border-t border-hairline p-3" onSubmit={(e) => { e.preventDefault(); send(q); }}>
        <div className="flex items-end gap-2 rounded-xl border border-hairline bg-surface-1 p-1.5 focus-within:border-hairline-strong focus-within:ring-2 focus-within:ring-focus/20">
          <textarea
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); send(q); } }}
            rows={1}
            maxLength={ASK_MAX_CHARS}
            placeholder="Ask WarpBot…"
            aria-label="Ask WarpBot"
            className="max-h-32 min-h-8 flex-1 resize-none bg-transparent px-2 py-1.5 text-[14px] outline-none"
          />
          {ask.busy
            ? <Button type="button" size="sm" variant="secondary" onClick={onAskStop}>Stop</Button>
            : <Button type="submit" size="sm" variant="primary" disabled={!q.trim()}>Ask</Button>}
        </div>
      </form>
    </div>
  );
}

/* ------------------------------------------------------------------ bits */

function IconButton({ label, onClick, children, active, disabled }: { label: string; onClick: () => void; children: ReactNode; active?: boolean; disabled?: boolean }) {
  return (
    <button
      type="button"
      aria-label={label}
      title={label}
      onClick={onClick}
      disabled={disabled}
      className={cn('grid size-8 place-items-center rounded-lg border border-hairline transition-colors disabled:opacity-40', active ? 'bg-chip-rose text-chip-rose-text' : 'bg-surface-1 text-ink-muted hover:bg-surface-2 hover:text-ink')}
    >
      {children}
    </button>
  );
}

function Tab({ active, onClick, children }: { active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={cn('-mb-px inline-flex items-center gap-1.5 border-b-2 px-3 py-2.5 text-[13px] font-medium transition', active ? 'border-ink text-ink' : 'border-transparent text-ink-subtle hover:text-ink')}
    >
      {children}
    </button>
  );
}

function Count({ n }: { n: number }) {
  return <span className="rounded-full bg-chip-gray px-1.5 text-[11px] text-chip-gray-text">{n}</span>;
}

function Block({ title, meta, action, children }: { title: string; meta?: string; action?: ReactNode; children: ReactNode }) {
  return (
    <div>
      <div className="mb-2 flex items-center justify-between gap-3">
        <div className="min-w-0">
          <h3 className="text-[14px] font-semibold">{title}</h3>
          {meta && <p className="text-[12px] text-ink-subtle">{meta}</p>}
        </div>
        {action}
      </div>
      {children}
    </div>
  );
}

function Empty({ children }: { children: ReactNode }) {
  return <p className="text-[13px] text-ink-subtle">{children}</p>;
}

function Section({ title, items }: { title: string; items: string[] }) {
  if (items.length === 0) return null;
  return (
    <div>
      <p className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-ink-subtle">{title}</p>
      <ul className="list-disc space-y-1 pl-5">{items.map((d, i) => <li key={i}>{d}</li>)}</ul>
    </div>
  );
}
