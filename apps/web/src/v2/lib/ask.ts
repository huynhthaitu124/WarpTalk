// "Ask AI" (WarpBot): `POST /meetings/{roomId}/ask` and `/recap` stream Server-Sent Events (delta, citations, done, error);
// `GET /meetings/{roomId}/ask/history` pages the persisted conversation. EventSource cannot POST or send a bearer token,
// so the stream is read with fetch + ReadableStream and the SSE parser shared with the suggestion stream.
import { useCallback, useEffect, useRef, useState } from 'react';
import { useInfiniteQuery, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { api, ApiError } from './api';
import { apiUrl } from './env';
import { session } from './session';
import { createSseParser } from './suggestions';
import { ASK } from './strings';

export type AskScope = 'meeting' | 'meeting+docs' | 'workspace';
export type AskKind = 'ask' | 'recap';
export const ASK_SCOPES: AskScope[] = ['meeting', 'meeting+docs', 'workspace'];
export const RECAP_WINDOWS: (number | null)[] = [5, 10, 15];
export const ASK_MAX_CHARS = 1000;

export interface Citation { kind: 'transcript' | 'document'; ref: string; label: string; tMs?: number; page?: number }
export interface AskDone { model?: string; messageId?: string; cached?: boolean }

const citationSchema = z.object({
  kind: z.string(),
  ref: z.string(),
  label: z.string(),
  tMs: z.number().nullable().optional(),
  page: z.number().nullable().optional(),
});
const historyItem = z.object({
  id: z.string(),
  role: z.string(),
  kind: z.string(),
  content: z.string(),
  citations: z.array(citationSchema).default([]),
  model: z.string().nullable().optional(),
  scope: z.string().nullable().optional(),
  createdAt: z.string(),
});
const historyPage = z.object({ items: z.array(historyItem), nextBefore: z.string().nullable().optional() });

export function normalizeCitations(list: z.infer<typeof citationSchema>[]): Citation[] {
  return list.flatMap((c): Citation[] => (c.kind === 'transcript' || c.kind === 'document'
    ? [{ kind: c.kind, ref: c.ref, label: c.label, ...(typeof c.tMs === 'number' ? { tMs: c.tMs } : {}), ...(typeof c.page === 'number' ? { page: c.page } : {}) }]
    : []));
}

/** Error with the backend code (`ANSWER_BLOCKED`, `RATE_LIMITED`, ...) so the UI can map it to a friendly sentence. */
export class AssistantError extends Error {
  constructor(public code: string, message: string, public status = 0) { super(message); this.name = 'AssistantError'; }
}

export function friendlyAskError(e: unknown): string {
  if (e instanceof AssistantError || e instanceof ApiError) return ASK.errors[e.code] ?? (e.status === 403 ? ASK.errors.FORBIDDEN! : e.status === 404 ? ASK.errors.NOT_FOUND! : e.message);
  return ASK.errors.NETWORK!;
}

interface StreamHandlers { onDelta(text: string): void; onCitations(c: Citation[]): void }

/**
 * Runs one streaming request. Gate errors (403/409/429/400) arrive as plain JSON before the stream and are thrown as
 * `AssistantError`; so is an `error` event inside the stream. Resolves with the `done` payload.
 */
export async function streamAssistant(path: string, body: unknown, h: StreamHandlers, signal: AbortSignal): Promise<AskDone> {
  const go = (token: string | null) => fetch(apiUrl(path), {
    method: 'POST',
    signal,
    headers: { 'content-type': 'application/json', accept: 'text/event-stream', ...(token ? { authorization: `Bearer ${token}` } : {}) },
    body: JSON.stringify(body),
  });
  let res = await go(session.accessToken);
  if (res.status === 401 && session.refreshToken) {
    const t = (await session.refresh())?.accessToken;
    if (t) res = await go(t);
  }
  if (!res.ok) {
    let code = 'HTTP_ERROR';
    let message = `Request failed (${res.status})`;
    try {
      const j = (await res.json()) as { code?: unknown; message?: unknown };
      if (typeof j.code === 'string') code = j.code;
      if (typeof j.message === 'string') message = j.message;
    } catch { /* non-JSON body */ }
    throw new AssistantError(code, message, res.status);
  }
  if (!res.body) throw new AssistantError('NETWORK', 'The server sent no stream.');
  let done: AskDone | null = null;
  let failure: AssistantError | null = null;
  const feed = createSseParser((m) => {
    let j: Record<string, unknown>;
    try { j = JSON.parse(m.data) as Record<string, unknown>; } catch { return; }
    if (m.event === 'delta' && typeof j.text === 'string') h.onDelta(j.text);
    else if (m.event === 'citations' && Array.isArray(j.citations)) h.onCitations(normalizeCitations(z.array(citationSchema).catch([]).parse(j.citations)));
    else if (m.event === 'done') done = { model: typeof j.model === 'string' ? j.model : undefined, messageId: typeof j.messageId === 'string' ? j.messageId : undefined, cached: j.cached === true };
    else if (m.event === 'error') failure = new AssistantError(typeof j.code === 'string' ? j.code : 'LLM_FAILED', typeof j.message === 'string' ? j.message : 'The assistant failed.');
  });
  const rd = res.body.getReader();
  const dec = new TextDecoder();
  for (;;) {
    const { done: end, value } = await rd.read();
    if (end) break;
    feed(dec.decode(value, { stream: true }));
  }
  if (failure) throw failure;
  if (!done) throw new AssistantError('NETWORK', ASK.errors.NETWORK!);
  return done;
}

/** One rendered message of the conversation (persisted or local). */
export interface AskMessage {
  key: string;
  /** Local messages: id of the question/answer pair. */
  turn?: string;
  role: 'user' | 'assistant';
  kind: AskKind;
  content: string;
  citations: Citation[];
  createdAt?: string;
  model?: string | null;
  cached?: boolean;
  /** Local assistant messages only. */
  status?: 'streaming' | 'done' | 'error';
  error?: string;
  /** The answer ended early (error after some text); the text is kept, flagged. */
  incomplete?: boolean;
}

const HISTORY_PAGE = 20;
export const askHistoryKey = (roomId: string | undefined) => ['ask-history', roomId] as const;

/** Persisted conversation, newest page first; `nextBefore` pages further back. */
export function useAskHistory(roomId: string | undefined) {
  return useInfiniteQuery({
    queryKey: askHistoryKey(roomId),
    enabled: !!roomId,
    retry: false,
    staleTime: 60_000,
    initialPageParam: undefined as string | undefined,
    queryFn: ({ pageParam }) => api(`/meetings/${roomId}/ask/history`, { query: { limit: HISTORY_PAGE, before: pageParam }, schema: historyPage }),
    getNextPageParam: (last) => last.nextBefore ?? undefined,
  });
}

let seq = 0;
const nextKey = () => `local-${Date.now()}-${++seq}`;

/**
 * Local conversation state: sends questions/recaps, streams the answer into a message and aborts on unmount.
 * Finished turns are handed over to the history query (refetched) so they are not shown twice.
 */
export function useAssistant(roomId: string) {
  const qc = useQueryClient();
  const [local, setLocal] = useState<AskMessage[]>([]);
  const [busy, setBusy] = useState(false);
  const ctl = useRef<AbortController | null>(null);

  useEffect(() => () => ctl.current?.abort(), []);
  useEffect(() => { ctl.current?.abort(); setLocal([]); setBusy(false); }, [roomId]);

  const run = useCallback(async (kind: AskKind, userText: string, path: string, body: unknown) => {
    if (ctl.current) return;
    const c = new AbortController();
    ctl.current = c;
    setBusy(true);
    const turn = nextKey();
    const aKey = `${turn}-a`;
    const base = { kind, turn, citations: [] as Citation[] };
    setLocal((l) => [
      ...l,
      { key: `${turn}-q`, role: 'user', ...base, content: userText },
      { key: aKey, role: 'assistant', ...base, content: '', status: 'streaming' },
    ]);
    const patch = (f: (m: AskMessage) => AskMessage) => setLocal((l) => l.map((m) => (m.key === aKey ? f(m) : m)));
    let text = '';
    try {
      const d = await streamAssistant(path, body, {
        onDelta: (t) => { text += t; patch((m) => ({ ...m, content: text })); },
        onCitations: (cs) => patch((m) => ({ ...m, citations: cs })),
      }, c.signal);
      patch((m) => ({ ...m, status: 'done', model: d.model, cached: d.cached }));
      // The refetched history now contains this turn: drop the local copy once it arrived.
      try {
        await qc.invalidateQueries({ queryKey: askHistoryKey(roomId) });
        if (!c.signal.aborted) setLocal((l) => l.filter((m) => m.turn !== turn));
      } catch { /* keep the local copy */ }
    } catch (e) {
      if (c.signal.aborted) {
        // Stopped by the person (or unmounted): keep what arrived, flagged.
        patch((m) => ({ ...m, status: 'done', incomplete: m.content.length > 0 }));
      } else if (e instanceof AssistantError && e.code === 'ANSWER_BLOCKED') {
        patch((m) => ({ ...m, content: '', citations: [], status: 'error', error: friendlyAskError(e) }));
      } else {
        patch((m) => ({ ...m, status: 'error', error: friendlyAskError(e), incomplete: m.content.length > 0 }));
      }
    } finally {
      if (ctl.current === c) ctl.current = null;
      setBusy(false);
    }
  }, [qc, roomId]);

  const ask = useCallback((question: string, scope: AskScope, sinceMs?: number) =>
    run('ask', question, `/meetings/${roomId}/ask`, { question, scope, ...(sinceMs !== undefined ? { sinceMs } : {}) }), [run, roomId]);
  const recap = useCallback((lastMinutes: number | null) =>
    run('recap', ASK.recapUser(lastMinutes), `/meetings/${roomId}/recap`, { lastMinutes }), [run, roomId]);
  const stop = useCallback(() => ctl.current?.abort(), []);
  return { local, busy, ask, recap, stop };
}
