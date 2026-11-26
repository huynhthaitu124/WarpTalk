import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Button } from '@v2/ui';
import { languageLabel } from '@v2/lib/languages';
import { PROVIDER_LABEL } from '@v2/lib/bridge';
import { summaryBusy, useActionItems, useRegenerateSummary, useSummary } from '@v2/lib/copilot';
import type { Room } from '@v2/lib/schemas';
import type { OverlayAsk } from '@v2/lib/native';
import type { FollowUp } from '@v2/lib/suggestions';
import { ASK_MAX_CHARS } from '@v2/lib/ask';
import { CaptionList, StatusStrip, type CaptionLine, type ConnectionLabel } from './MeetingParts';

type Tone = 'emerald' | 'amber' | 'rose' | 'sky' | 'gray';
const CHIP: Record<Tone, string> = {
  emerald: 'bg-chip-emerald text-chip-emerald-text',
  amber: 'bg-chip-amber text-chip-amber-text',
  rose: 'bg-chip-rose text-chip-rose-text',
  sky: 'bg-chip-sky text-chip-sky-text',
  gray: 'bg-chip-gray text-chip-gray-text',
};

/**
 * Main window while the copilot pill runs, laid out like a Claude Meetings page: the live transcript on the left,
 * WarpBot (chat + suggestions) and the notes (minutes, action items, follow-ups) on the right.
 * Styled like the ported (light) meeting page, so it forces the light theme while mounted; the pill window keeps its own.
 */
export function CopilotDetail({
  room, lines, translationLang, connection, captureState, suggestions, followUps, ask, onAsk, onAskStop, onShowCopilot, onEnd, ending,
}: {
  room: Room;
  lines: CaptionLine[];
  translationLang?: string;
  connection: ConnectionLabel;
  captureState: string;
  suggestions: { id: string; content: string }[];
  followUps: FollowUp[];
  ask: OverlayAsk;
  onAsk: (q: string) => void;
  onAskStop: () => void;
  onShowCopilot: () => void;
  onEnd: () => void;
  ending: boolean;
}) {
  useEffect(() => {
    const root = document.documentElement;
    const prev = root.dataset.theme;
    root.dataset.theme = 'light';
    return () => { if (prev) root.dataset.theme = prev; else delete root.dataset.theme; };
  }, []);

  const summary = useSummary(room.id, true, false);
  const regenerate = useRegenerateSummary(room.id);
  const actions = useActionItems(room.id);
  const s = summary.data;
  const providerKey = room.bridge?.provider as keyof typeof PROVIDER_LABEL | undefined;
  const provider = providerKey && providerKey !== 'desktop' ? PROVIDER_LABEL[providerKey] ?? providerKey : undefined;
  const started = room.startedAt ? new Date(room.startedAt) : null;
  const time = (d: Date) => d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  const capture: [string, Tone] =
    captureState === 'running' ? ['Translating', 'emerald']
      : captureState === 'error' ? ['Capture error', 'rose']
        : captureState === 'starting' ? ['Starting…', 'amber']
          : [captureState || 'Idle', 'gray'];
  const items = actions.data ?? [];
  const [tab, setTab] = useState<'warpbot' | 'notes'>('warpbot');

  return (
    <div className="-m-4 min-h-full bg-panel text-ink" data-testid="copilot-host">
      <div className="mx-auto w-full max-w-[1500px] px-6 py-8 xl:px-10">
        <header className="mb-8 flex flex-col gap-5 border-b border-hairline pb-6 sm:flex-row sm:items-start sm:justify-between">
          <div className="min-w-0">
            <h1 className="truncate text-[30px] font-semibold leading-tight tracking-tight">{room.title || 'Meeting'}</h1>
            <div className="mt-3 flex flex-wrap items-center gap-1.5">
              <Chip tone={capture[1]}>{capture[0]}</Chip>
              {provider && <Chip tone="sky">{provider}</Chip>}
              <Chip>{languageLabel(room.sourceLanguage)} → {room.targetLanguages.map(languageLabel).join(', ') || '—'}</Chip>
              {started && <Chip>Started {time(started)}</Chip>}
            </div>
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Button variant="secondary" onClick={onEnd} disabled={ending}>End meeting</Button>
            <Button variant="primary" onClick={onShowCopilot}>Show copilot</Button>
          </div>
        </header>

        <div className="mb-6"><StatusStrip connection={connection} quality="unknown" room={room} /></div>

        <div className="grid gap-6 xl:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)]">
          <Panel title="Transcript" meta="Live, original and translation">
            <CaptionList lines={lines} placeholder="Waiting for speech…" translationLang={translationLang} className="max-h-[36rem] min-h-72" />
          </Panel>

          <section aria-label="WarpBot and notes" className="flex min-w-0 flex-col rounded-xl border border-hairline bg-surface-1 shadow-[var(--elev-card)] xl:sticky xl:top-4 xl:max-h-[calc(100dvh-2rem)]">
            <div role="tablist" className="flex gap-1 border-b border-hairline px-3 pt-2">
              <Tab active={tab === 'warpbot'} onClick={() => setTab('warpbot')}>WarpBot{suggestions.length ? <Count n={suggestions.length} /> : null}</Tab>
              <Tab active={tab === 'notes'} onClick={() => setTab('notes')}>Notes{items.length + followUps.length ? <Count n={items.length + followUps.length} /> : null}</Tab>
            </div>
            {tab === 'warpbot' ? (
              <WarpBotPanel ask={ask} suggestions={suggestions} onAsk={onAsk} onAskStop={onAskStop} />
            ) : (
              <div className="flex min-h-0 flex-1 flex-col gap-6 overflow-y-auto px-5 py-4 text-[14px] leading-relaxed">
                <Block
                  title="Summary"
                  meta={summaryBusy(s) ? 'Updating…' : s?.generatedAt ? `Updated ${time(new Date(s.generatedAt))}` : undefined}
                  action={<Button size="sm" variant="ghost" onClick={() => regenerate.mutate()} disabled={regenerate.isPending || summaryBusy(s) || lines.length === 0}>Refresh</Button>}
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
                        <li key={a.id} className={a.done ? 'text-ink-subtle line-through' : ''}>
                          {a.text}{a.owner ? <span className="text-ink-subtle"> · {a.owner}</span> : null}
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
            )}
          </section>
        </div>
      </div>
    </div>
  );
}

function WarpBotPanel({ ask, suggestions, onAsk, onAskStop }: {
  ask: OverlayAsk; suggestions: { id: string; content: string }[]; onAsk: (q: string) => void; onAskStop: () => void;
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
  return (
    <div className="flex min-h-[28rem] flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">
        {suggestions.length > 0 && (
          <div className="mb-4">
            <p className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-ink-subtle">Suggested now</p>
            <ul className="flex flex-col gap-2">
              {suggestions.slice(0, 4).map((x) => (
                <li key={x.id}>
                  <button
                    type="button"
                    onClick={() => send(`Help me respond to this: ${x.content}`)}
                    disabled={ask.busy}
                    className="w-full rounded-lg border border-hairline bg-surface-2/60 px-3 py-2 text-left text-[14px] leading-relaxed transition hover:bg-surface-2 disabled:opacity-60"
                  >
                    {x.content}
                  </button>
                </li>
              ))}
            </ul>
          </div>
        )}
        {ask.messages.length === 0 ? (
          <Empty>Ask WarpBot about the call: what was just said, a reply in the other language, or anything in your documents.</Empty>
        ) : (
          <ol className="flex flex-col gap-3">
            {ask.messages.map((m) => (
              <li key={m.key} className={m.role === 'user' ? 'self-end max-w-[85%] rounded-2xl rounded-br-md bg-ink-btn px-3.5 py-2 text-[14px] text-on-ink-btn' : 'max-w-full text-[14px] leading-relaxed'}>
                {m.role === 'assistant' && !m.content && m.status === 'streaming' ? <span className="text-ink-subtle">Thinking…</span> : <p className="whitespace-pre-wrap">{m.content}</p>}
                {m.error && <p className="mt-1 text-[12px] text-danger-text">{m.error}</p>}
              </li>
            ))}
          </ol>
        )}
        <div ref={end} />
      </div>
      <form
        className="flex items-end gap-2 border-t border-hairline p-3"
        onSubmit={(e) => { e.preventDefault(); send(q); }}
      >
        <textarea
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); send(q); } }}
          rows={1}
          maxLength={ASK_MAX_CHARS}
          placeholder="Ask WarpBot…"
          aria-label="Ask WarpBot"
          className="max-h-32 min-h-10 flex-1 resize-none rounded-lg border border-hairline bg-surface-1 px-3 py-2 text-[14px] outline-none focus:border-hairline-strong focus:ring-2 focus:ring-focus/30"
        />
        {ask.busy
          ? <Button type="button" variant="secondary" onClick={onAskStop}>Stop</Button>
          : <Button type="submit" variant="primary" disabled={!q.trim()}>Ask</Button>}
      </form>
    </div>
  );
}

function Tab({ active, onClick, children }: { active: boolean; onClick: () => void; children: ReactNode }) {
  return (
    <button
      type="button"
      role="tab"
      aria-selected={active}
      onClick={onClick}
      className={`-mb-px inline-flex items-center gap-1.5 border-b-2 px-3 py-2 text-[14px] font-medium transition ${active ? 'border-ink text-ink' : 'border-transparent text-ink-subtle hover:text-ink'}`}
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
          <h3 className="text-[15px] font-semibold">{title}</h3>
          {meta && <p className="text-[12px] text-ink-subtle">{meta}</p>}
        </div>
        {action}
      </div>
      {children}
    </div>
  );
}

function Chip({ tone = 'gray', children }: { tone?: Tone; children: ReactNode }) {
  return <span className={`inline-flex items-center rounded-full px-2.5 py-0.5 text-[12px] font-medium ${CHIP[tone]}`}>{children}</span>;
}

function Panel({ title, meta, action, children }: { title: string; meta?: string; action?: ReactNode; children: ReactNode }) {
  return (
    <section aria-label={title} className="min-w-0 rounded-xl border border-hairline bg-surface-1 shadow-[var(--elev-card)]">
      <div className="flex items-center justify-between gap-3 border-b border-hairline px-5 py-3">
        <div className="min-w-0">
          <h2 className="text-[15px] font-semibold">{title}</h2>
          {meta && <p className="text-[12px] text-ink-subtle">{meta}</p>}
        </div>
        {action}
      </div>
      <div className="px-5 py-4">{children}</div>
    </section>
  );
}

function Empty({ children }: { children: ReactNode }) {
  return <p className="text-[14px] text-ink-subtle">{children}</p>;
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
