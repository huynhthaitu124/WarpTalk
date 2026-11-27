// Live AI suggestions: SSE stream `GET /meetings/{roomId}/suggestions/stream` (spec 09 §9) and
// `GET /meetings/{roomId}/followups`. EventSource cannot send an Authorization header, so the stream is read with
// fetch. When the endpoint answers 404/501/503 (backend not deployed) the feature reports `unavailable` and the UI hides it.
import { useCallback, useEffect, useRef, useState } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api, ApiError } from './api';
import { z } from 'zod';
import { session } from './session';
import { apiUrl } from '@v2/lib/env';

export interface Suggestion {
  suggestionId: string;
  category: string;
  content: string;
  detail?: string;
  confidence?: number;
  language?: string;
  severity?: string;
  ttlSeconds?: number;
  followUp?: string;
  actions?: { label: string; prompt?: string; [k: string]: unknown }[];
  sources?: { title?: string; url?: string; [k: string]: unknown }[];
  /** Client clock when it arrived (for ttl). */
  receivedAt: number;
}

export type StreamStatus = 'connecting' | 'open' | 'unavailable';

const str = (v: unknown) => (typeof v === 'string' && v ? v : undefined);

/** Lists may arrive as arrays or as JSON-encoded strings (`*_json` columns). */
function asList(v: unknown): unknown[] | undefined {
  if (typeof v === 'string') { try { v = JSON.parse(v); } catch { return undefined; } }
  return Array.isArray(v) ? v : undefined;
}
function followUpText(v: unknown): string | undefined {
  if (typeof v === 'string') {
    try { const j: unknown = JSON.parse(v); if (typeof j === 'object' && j !== null) return followUpText(j); } catch { /* plain text */ }
    return v || undefined;
  }
  if (typeof v === 'object' && v !== null) {
    const o = v as Record<string, unknown>;
    const t = str(o.task) ?? str(o.content) ?? str(o.text);
    return t && (str(o.owner) ? `${t} (${str(o.owner)})` : t);
  }
  return undefined;
}

export function parseSuggestion(data: string, now = Date.now()): Suggestion | null {
  let j: unknown;
  try { j = JSON.parse(data); } catch { return null; }
  if (typeof j !== 'object' || j === null) return null;
  const o = j as Record<string, unknown>;
  const id = str(o.suggestionId) ?? str(o.suggestion_id) ?? str(o.id);
  const content = str(o.content) ?? str(o.text);
  if (!id || !content) return null;
  const ttl = o.ttlSeconds ?? o.ttl_seconds;
  const conf = o.confidence;
  return {
    suggestionId: id,
    category: str(o.category) ?? 'general',
    content,
    detail: str(o.detail),
    confidence: typeof conf === 'number' ? conf : undefined,
    language: str(o.language),
    severity: str(o.severity),
    ttlSeconds: typeof ttl === 'number' && ttl > 0 ? ttl : undefined,
    followUp: followUpText(o.followUp ?? o.follow_up ?? o.follow_up_json),
    actions: asList(o.actions ?? o.actions_json)?.filter((a) => typeof a === 'object' && a !== null && typeof (a as { label?: unknown }).label === 'string') as Suggestion['actions'],
    sources: asList(o.sources ?? o.sources_json)?.filter((a) => typeof a === 'object' && a !== null) as Suggestion['sources'],
    receivedAt: now,
  };
}

/** Incremental SSE parser: feed text chunks, get `{event, data}` messages. */
export function createSseParser(onMessage: (m: { event: string; data: string; id?: string }) => void) {
  let buf = '';
  let lastId: string | undefined;
  let event = '';
  let data: string[] = [];
  return (chunk: string) => {
    buf += chunk;
    let nl: number;
    while ((nl = buf.search(/\r\n|\n|\r/)) >= 0) {
      const line = buf.slice(0, nl);
      const m = /^\r\n/.exec(buf.slice(nl));
      buf = buf.slice(nl + (m ? 2 : 1));
      if (line === '') {
        if (data.length) onMessage({ event: event || 'message', data: data.join('\n'), ...(lastId ? { id: lastId } : {}) });
        event = ''; data = [];
      } else if (line.startsWith(':')) {
        // comment / keep-alive
      } else {
        const k = line.indexOf(':');
        const field = k < 0 ? line : line.slice(0, k);
        const value = k < 0 ? '' : line.slice(k + 1).replace(/^ /, '');
        if (field === 'event') event = value;
        else if (field === 'data') data.push(value);
        else if (field === 'id') lastId = value;
      }
    }
  };
}

const GONE = new Set([404, 405, 501, 503]);

async function open(path: string, signal: AbortSignal, lastId?: string): Promise<Response> {
  const go = (t: string | null) => fetch(apiUrl(path), { signal, headers: { accept: 'text/event-stream', ...(lastId ? { 'last-event-id': lastId } : {}), ...(t ? { authorization: `Bearer ${t}` } : {}) } });
  let res = await go(session.accessToken);
  if (res.status === 401 && session.refreshToken) {
    const t = (await session.refresh())?.accessToken;
    if (t) res = await go(t);
  }
  return res;
}

export function useSuggestionStream(roomId: string | undefined, lang?: string, enabled = true) {
  const [status, setStatus] = useState<StreamStatus>('connecting');
  const [items, setItems] = useState<Suggestion[]>([]);
  useEffect(() => {
    if (!roomId || !enabled) return;
    const ctl = new AbortController();
    let delay = 1500;
    let lastId: string | undefined;
    setStatus('connecting');
    const run = async () => {
      while (!ctl.signal.aborted) {
        try {
          const res = await open(`/meetings/${roomId}/suggestions/stream${lang && lang !== 'original' ? `?lang=${encodeURIComponent(lang)}` : ''}`, ctl.signal, lastId);
          if (GONE.has(res.status)) { setStatus('unavailable'); return; }
          if (res.ok && res.body) {
            setStatus('open');
            delay = 1500;
            const feed = createSseParser((m) => {
              if (m.id) lastId = m.id;
              const s = parseSuggestion(m.data);
              if (s) setItems((cur) => (cur.some((c) => c.suggestionId === s.suggestionId) ? cur.map((c) => (c.suggestionId === s.suggestionId ? { ...s, receivedAt: c.receivedAt } : c)) : [...cur, s].slice(-50)));
            });
            const rd = res.body.getReader();
            const dec = new TextDecoder();
            for (;;) {
              const { done, value } = await rd.read();
              if (done) break;
              feed(dec.decode(value, { stream: true }));
            }
          } else if (res.status === 401 || res.status === 403) { setStatus('unavailable'); return; }
        } catch { if (ctl.signal.aborted) return; }
        await new Promise((r) => setTimeout(r, delay));
        delay = Math.min(delay * 2, 30_000);
      }
    };
    void run();
    return () => ctl.abort();
  }, [roomId, lang, enabled]);
  return { status, items };
}

/** Dismiss (forever) and snooze (hide for a while) state, per suggestion id. */
export function useSuggestionTriage(items: Suggestion[], snoozeMs = 5 * 60_000) {
  const [dismissed, setDismissed] = useState<Set<string>>(() => new Set());
  const [snoozed, setSnoozed] = useState<Record<string, number>>({});
  const [now, setNow] = useState(() => Date.now());
  const itemsRef = useRef(items);
  itemsRef.current = items;
  useEffect(() => { const t = setInterval(() => setNow(Date.now()), 5000); return () => clearInterval(t); }, []);
  const dismiss = useCallback((id: string) => setDismissed((s) => new Set(s).add(id)), []);
  const snooze = useCallback((id: string) => setSnoozed((s) => ({ ...s, [id]: Date.now() + snoozeMs })), [snoozeMs]);
  const visible = items.filter((s) => !dismissed.has(s.suggestionId)
    && (snoozed[s.suggestionId] ?? 0) <= now
    && (!s.ttlSeconds || s.receivedAt + s.ttlSeconds * 1000 > now));
  return { visible, dismiss, snooze };
}

export interface FollowUp { id: string; text: string; owner?: string; due?: string }

/** Accepts `[..]`, `{items:[..]}` or `{followups:[..]}` whose entries are strings or objects. */
export function parseFollowUps(json: unknown): FollowUp[] {
  const list = Array.isArray(json) ? json : typeof json === 'object' && json !== null
    ? ((json as Record<string, unknown>).items ?? (json as Record<string, unknown>).followups ?? (json as Record<string, unknown>).followUps)
    : undefined;
  if (!Array.isArray(list)) return [];
  const out: FollowUp[] = [];
  list.forEach((e, n) => {
    if (typeof e === 'string' && e) { out.push({ id: `f${n}`, text: e }); return; }
    if (typeof e !== 'object' || e === null) return;
    const o = e as Record<string, unknown>;
    const text = str(o.task) ?? str(o.content) ?? str(o.text) ?? str(o.followUp) ?? str(o.title) ?? str(o.description);
    if (text) out.push({ id: str(o.suggestionId) ?? str(o.id) ?? str(o.followUpId) ?? `f${n}`, text, owner: str(o.owner) ?? str(o.assignee), due: str(o.due) ?? str(o.dueDate) });
  });
  return out;
}

export function useFollowUps(roomId: string | undefined, enabled: boolean) {
  return useQuery({
    queryKey: ['followups', roomId],
    enabled: !!roomId && enabled,
    refetchInterval: 15_000,
    retry: false,
    queryFn: async () => {
      try { return parseFollowUps(await api(`/meetings/${roomId}/followups`, { schema: z.unknown() })); } catch (e) {
        if (e instanceof ApiError && GONE.has(e.status)) return [];
        throw e;
      }
    },
  });
}

// ---- Dev test mode (scripted suggestions). The API serves these routes only when it runs with
// `SUGGESTION_TEST_SCRIPT` in a debug build; anywhere else the probe answers false and nothing shows.

const testModeSchema = z.object({ suggestionScript: z.boolean() });
const testLineSchema = z.object({
  segmentId: z.string(), speakerId: z.string(), speakerName: z.string().nullish(),
  text: z.string(), language: z.string(), timestampMs: z.number(),
});

export function useSuggestionTestMode(): boolean {
  const q = useQuery({
    queryKey: ['dev', 'test-mode'],
    queryFn: () => api('/dev/test-mode', { schema: testModeSchema }),
    staleTime: Infinity,
    retry: false,
  });
  return q.data?.suggestionScript === true;
}

/** Publishes one typed line as a final transcript segment of the meeting (dev test mode). */
export function sendTestLine(roomId: string, text: string, language?: string) {
  return api(`/meetings/${roomId}/dev/transcript`, {
    method: 'POST',
    body: { text, ...(language && language !== 'original' ? { language } : {}) },
    schema: testLineSchema,
  });
}

// ---- feedback (`POST /suggestions/{id}/feedback`, 204)
export type FeedbackAction = 'used' | 'dismissed' | 'helpful' | 'not_helpful' | 'wrong';

export interface FeedbackState {
  /** Actions already sent (or being sent) per suggestion. */
  marks: Record<string, FeedbackAction[]>;
  /** Last failed action per suggestion (cleared by the next success). */
  failed: Record<string, FeedbackAction | undefined>;
  send(id: string, action: FeedbackAction, comment?: string): Promise<boolean>;
}

/**
 * Optimistic feedback: the action is marked at once and rolled back when the request fails. Repeating an action
 * with a comment updates the comment on the server (one verdict per user and action).
 */
export function useSuggestionFeedback(): FeedbackState {
  const [marks, setMarks] = useState<Record<string, FeedbackAction[]>>({});
  const [failed, setFailed] = useState<Record<string, FeedbackAction | undefined>>({});
  const send = useCallback(async (id: string, action: FeedbackAction, comment?: string) => {
    const had = (m: Record<string, FeedbackAction[]>) => m[id]?.includes(action) ?? false;
    let already = false;
    setMarks((m) => { already = had(m); return already ? m : { ...m, [id]: [...(m[id] ?? []), action] }; });
    setFailed((f) => ({ ...f, [id]: undefined }));
    try {
      await api(`/suggestions/${id}/feedback`, { method: 'POST', body: { action, ...(comment?.trim() ? { comment: comment.trim() } : {}) } });
      return true;
    } catch {
      if (!already) setMarks((m) => ({ ...m, [id]: (m[id] ?? []).filter((a) => a !== action) }));
      setFailed((f) => ({ ...f, [id]: action }));
      return false;
    }
  }, []);
  return { marks, failed, send };
}
