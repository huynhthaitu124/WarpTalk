// "Ask AI" panel: question box + scope, quick recaps, the streamed conversation with citation chips, paged history.
// Used on the session detail page and as a tab of the meeting side panel. Data layer: src/lib/ask.ts.
import { useEffect, useMemo, useRef, useState, type FormEvent, type KeyboardEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { ArrowClockwise, FileText, PaperPlaneRight, Quotes, Sparkle, Stop } from '@phosphor-icons/react';
import { Alert, Badge, Button, Select, Skeleton, Textarea } from '@v2/ui';
import { cn } from '@v2/lib/cn';
import {
  ASK_MAX_CHARS, ASK_SCOPES, RECAP_WINDOWS, friendlyAskError, useAskHistory, useAssistant,
  type AskMessage, type AskScope, type Citation,
} from '@v2/lib/ask';
import { Markdown, tagCitations } from '@v2/lib/markdown';
import { ASK } from '@v2/lib/strings';
import { useWorkspaceOptional } from '@v2/lib/workspace';

function CitationChip({ c, tag, onOpen }: { c: Citation; tag?: string; onOpen(c: Citation): void }) {
  const doc = c.kind === 'document';
  const label = doc && c.page ? `${c.label}, p. ${c.page}` : c.label;
  return (
    <button
      type="button"
      data-testid="citation"
      data-kind={c.kind}
      data-ref={c.ref}
      onClick={() => onOpen(c)}
      aria-label={doc ? ASK.openDocument(label) : ASK.jumpToLine(label)}
      className="inline-flex max-w-full items-center gap-1 rounded-pill border border-hairline bg-surface-2 px-2 py-0.5 align-baseline text-caption font-medium text-ink-muted hover:bg-surface-3 hover:text-ink focus-ring"
    >
      {doc ? <FileText size={12} aria-hidden className="shrink-0" /> : <Quotes size={12} aria-hidden className="shrink-0" />}
      {tag && <span className="font-mono text-ink-subtle" aria-hidden>{tag}</span>}
      <span className="truncate">{label}</span>
    </button>
  );
}

function Bubble({ m, onCitation }: { m: AskMessage; onCitation(c: Citation): void }) {
  if (m.role === 'user') {
    return (
      <div data-testid="ask-message" data-role="user" data-kind={m.kind} className="flex flex-col items-end gap-0.5">
        <span className="text-caption text-ink-subtle">{ASK.you}</span>
        <p className="max-w-[90%] whitespace-pre-wrap break-words rounded-lg bg-primary px-3 py-1.5 text-body text-on-primary">{m.content}</p>
      </div>
    );
  }
  const tags = tagCitations(m.content, m.citations);
  const streaming = m.status === 'streaming';
  return (
    <div data-testid="ask-message" data-role="assistant" data-kind={m.kind} data-status={m.status ?? 'done'} className="flex flex-col gap-1">
      <span className="flex items-center gap-1.5 text-caption text-ink-subtle">
        <Sparkle size={12} weight="fill" aria-hidden />{ASK.assistant}
        {m.cached && <Badge size="sm">{ASK.cached}</Badge>}
      </span>
      {m.content ? (
        <div data-testid="ask-answer" className="max-w-full rounded-lg bg-surface-2 px-3 py-2 text-body text-ink">
          <Markdown
            text={m.content}
            renderTag={(tag, kind, n) => {
              const c = tags.get(`${kind}${n}`);
              return c ? <CitationChip c={c} tag={tag} onOpen={onCitation} /> : <span className="text-ink-subtle">{tag}</span>;
            }}
          />
        </div>
      ) : streaming ? <p className="px-1 text-body-sm text-ink-subtle" role="status">{ASK.thinking}</p> : null}
      {m.incomplete && !m.error && <p className="px-1 text-caption text-ink-subtle">{ASK.incomplete}</p>}
      {m.error && <Alert tone="danger"><span data-testid="ask-error">{m.error}</span></Alert>}
      {m.citations.length > 0 && !streaming && (
        <div data-testid="ask-sources" className="flex flex-wrap items-center gap-1.5" role="group" aria-label={ASK.sources}>
          {m.citations.map((c) => <CitationChip key={`${c.kind}-${c.ref}`} c={c} onOpen={onCitation} />)}
        </div>
      )}
    </div>
  );
}

export function historyToMessages(pages: { items: { id: string; role: string; kind: string; content: string; citations: { kind: string; ref: string; label: string; tMs?: number | null; page?: number | null }[]; model?: string | null; createdAt: string }[] }[]): AskMessage[] {
  const seen = new Set<string>();
  const out: AskMessage[] = [];
  for (const p of pages) for (const it of p.items) {
    if (seen.has(it.id)) continue;
    seen.add(it.id);
    out.push({
      key: it.id,
      role: it.role === 'user' ? 'user' : 'assistant',
      kind: it.kind === 'recap' ? 'recap' : 'ask',
      content: it.content,
      createdAt: it.createdAt,
      model: it.model,
      status: 'done',
      citations: it.citations.flatMap((c): Citation[] => (c.kind === 'transcript' || c.kind === 'document'
        ? [{ kind: c.kind, ref: c.ref, label: c.label, ...(typeof c.tMs === 'number' ? { tMs: c.tMs } : {}), ...(typeof c.page === 'number' ? { page: c.page } : {}) }]
        : [])),
    });
  }
  return out.sort((a, b) => (a.createdAt ?? '').localeCompare(b.createdAt ?? ''));
}

/**
 * `onCitation` is called for transcript citations (jump to the line); document citations open the Documents page.
 * `className` lets the host decide the height (side panel: fills; page band: a fixed max height).
 */
export function AskPanel({ roomId, onCitation, className, canRecap = true }: { roomId: string; onCitation?: (c: Citation) => void; className?: string; canRecap?: boolean }) {
  const nav = useNavigate();
  const ws = useWorkspaceOptional();
  const history = useAskHistory(roomId);
  const a = useAssistant(roomId);
  const [text, setText] = useState('');
  const [scope, setScope] = useState<AskScope>('meeting+docs');
  const log = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  const past = useMemo(() => historyToMessages(history.data?.pages ?? []), [history.data]);
  const messages = [...past, ...a.local];
  const last = a.local[a.local.length - 1];

  // Follow new local output; loading older history must not move the view.
  useEffect(() => {
    const el = log.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [last?.key, last?.content, last?.status, past.length]);

  function open(c: Citation) {
    if (c.kind === 'document') {
      nav(ws ? `${ws.wsPath('/documents')}?search=${encodeURIComponent(c.label)}` : '/');
      return;
    }
    onCitation?.(c);
  }
  function submit(e?: FormEvent) {
    e?.preventDefault();
    const q = text.trim();
    if (!q || a.busy) return;
    stick.current = true;
    setText('');
    void a.ask(q, scope);
  }
  function onKey(e: KeyboardEvent<HTMLTextAreaElement>) {
    if (e.key === 'Enter' && !e.shiftKey && !e.nativeEvent.isComposing) { e.preventDefault(); submit(); }
  }
  const tooLong = text.length > ASK_MAX_CHARS;

  return (
    <div role="region" aria-label={`${ASK.title} panel`} data-testid="ask-panel" className={cn('flex min-h-0 flex-col', className)}>
      {canRecap && (
        <div role="group" aria-label={ASK.recapGroup} className="flex flex-wrap items-center gap-1.5 border-b border-row px-3 py-2">
          <span className="mr-1 text-caption text-ink-subtle">{ASK.recapGroup}</span>
          {[...RECAP_WINDOWS, null].map((w) => (
            <Button key={String(w)} size="sm" disabled={a.busy} data-testid={`recap-${w ?? 'all'}`} onClick={() => { stick.current = true; void a.recap(w); }}>{ASK.recap(w)}</Button>
          ))}
        </div>
      )}
      <div
        ref={log}
        role="log"
        aria-live="polite"
        aria-label="Assistant conversation"
        tabIndex={0}
        data-testid="ask-log"
        onScroll={(e) => { const el = e.currentTarget; stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 40; }}
        className="focus-ring min-h-40 flex-1 space-y-3 overflow-y-auto p-3"
      >
        {history.hasNextPage && (
          <div className="flex justify-center">
            <Button size="sm" variant="ghost" loading={history.isFetchingNextPage} data-testid="ask-load-earlier" onClick={() => { stick.current = false; void history.fetchNextPage(); }}>
              <ArrowClockwise size={14} aria-hidden />{ASK.loadEarlier}
            </Button>
          </div>
        )}
        {history.isLoading && <div aria-hidden><Skeleton className="h-10" /></div>}
        {history.isError && <p role="alert" className="text-body-sm text-danger-text">{ASK.historyError} {friendlyAskError(history.error)}</p>}
        {messages.length === 0 && !history.isLoading && !history.isError && <p data-testid="ask-empty" className="px-1 py-2 text-body-sm text-ink-subtle">{ASK.empty}</p>}
        {messages.map((m) => <Bubble key={m.key} m={m} onCitation={open} />)}
      </div>
      <form onSubmit={submit} className="flex flex-col gap-2 border-t border-hairline p-3" aria-label="Ask a question">
        <Textarea
          aria-label={ASK.inputLabel}
          placeholder={ASK.placeholder}
          rows={2}
          value={text}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={onKey}
          aria-invalid={tooLong || undefined}
          className="min-h-14 resize-none"
        />
        {tooLong && <p role="alert" className="text-caption text-danger-text">{ASK.tooLong(ASK_MAX_CHARS)}</p>}
        <div className="flex flex-wrap items-center gap-2">
          <Select
            className="h-7 w-52 max-w-full rounded-pill text-body-sm"
            aria-label={ASK.scopeLabel}
            testId="ask-scope"
            value={scope}
            onValueChange={(v) => setScope(v as AskScope)}
            options={ASK_SCOPES.map((s) => ({ value: s, label: ASK.scopes[s] }))}
          />
          <div className="ml-auto flex items-center gap-2">
            {a.busy && <Button size="sm" onClick={a.stop} data-testid="ask-stop"><Stop size={14} weight="fill" aria-hidden />{ASK.stop}</Button>}
            <Button type="submit" size="sm" variant="primary" disabled={a.busy || !text.trim() || tooLong} data-testid="ask-send"><PaperPlaneRight size={14} aria-hidden />{ASK.send}</Button>
          </div>
        </div>
      </form>
    </div>
  );
}
