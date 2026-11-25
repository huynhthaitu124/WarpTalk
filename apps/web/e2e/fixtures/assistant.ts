// Scriptable stand-in for the assistant endpoints (`/meetings/{id}/ask|recap|ask/history|prior-context`).
// The e2e backend has no LLM provider, so streamed answers are served from here; gate errors are also covered against the
// real backend (LLM_NOT_CONFIGURED). Playwright fulfils a route in one piece, so a "stream" is delivered at once.
import type { Page, Route } from '@playwright/test';

export interface MockCitation { kind: 'transcript' | 'document'; ref: string; label: string; tMs?: number; page?: number }
export interface MockHistoryItem { id: string; role: 'user' | 'assistant'; kind: 'ask' | 'recap'; content: string; citations: MockCitation[]; model: string | null; scope: string | null; createdAt: string }

/** What one ask/recap answers with. */
export interface AskScript {
  deltas?: string[];
  citations?: MockCitation[];
  done?: { model?: string; messageId?: string; cached?: boolean };
  /** SSE `error` event after the deltas. */
  streamError?: { code: string; message: string };
  /** Plain JSON gate error before any stream. */
  gate?: { status: number; code: string; message: string };
  /** Never answer (the page must abort the request). */
  hold?: boolean;
}

export interface AssistantMock {
  /** "ask" | "recap" and the JSON body of every request. */
  requests: { kind: 'ask' | 'recap'; body: Record<string, unknown>; stream: boolean }[];
  history: MockHistoryItem[];
  historyCalls: string[];
  prior: { openActionItems: unknown[]; lastSummaries: unknown[] };
  /** Next answers (consumed in order; the last one repeats). */
  script: AskScript[];
  aborted: number;
}

const sse = (events: [string, unknown][]) => events.map(([e, d]) => `event: ${e}\ndata: ${JSON.stringify(d)}\n\n`).join('');

export function hist(n: number, o: Partial<MockHistoryItem> = {}): MockHistoryItem {
  const at = new Date(Date.UTC(2026, 10, 20, 9, 0, n)).toISOString();
  return { id: `00000000-0000-4000-8000-${String(n).padStart(12, '0')}`, role: 'user', kind: 'ask', content: `Question ${n}`, citations: [], model: null, scope: 'meeting', createdAt: at, ...o };
}

export async function mockAssistant(page: Page, init: Partial<Pick<AssistantMock, 'history' | 'prior' | 'script'>> = {}): Promise<AssistantMock> {
  const m: AssistantMock = { requests: [], history: [], historyCalls: [], prior: { openActionItems: [], lastSummaries: [] }, script: [{ deltas: ['Hello.'] }], aborted: 0, ...init };
  let served = 0;
  page.on('requestfailed', (r) => { if (/\/(ask|recap)(\?|$)/.test(r.url()) && r.method() === 'POST') m.aborted += 1; });
  await page.route(/\/api\/v1\/meetings\/[^/]+\/(ask|recap)(\?.*)?$/, async (route: Route) => {
    const req = route.request();
    if (req.method() !== 'POST') return route.fallback();
    const u = new URL(req.url());
    const kind = u.pathname.endsWith('/recap') ? 'recap' : 'ask';
    m.requests.push({ kind, body: (req.postDataJSON() ?? {}) as Record<string, unknown>, stream: u.searchParams.get('stream') !== 'false' });
    const s = m.script[Math.min(served++, m.script.length - 1)]!;
    if (s.hold) return; // left pending: the page aborts it
    if (s.gate) return route.fulfill({ status: s.gate.status, contentType: 'application/json', body: JSON.stringify({ code: s.gate.code, message: s.gate.message }) });
    const events: [string, unknown][] = [...(s.deltas ?? []).map((t): [string, unknown] => ['delta', { text: t }])];
    if (s.citations) events.push(['citations', { citations: s.citations }]);
    if (s.streamError) events.push(['error', s.streamError]);
    else {
      // Like the backend, a finished turn is persisted: the page refetches the history after `done`.
      const n = 1000 + m.history.length;
      const at = (k: number) => new Date(Date.UTC(2026, 11, 1, 0, 0, k)).toISOString();
      m.history.push(
        hist(n, { role: 'user', kind, content: kind === 'recap' ? 'Recap' : String(m.requests.at(-1)?.body.question ?? ''), createdAt: at(n) }),
        hist(n + 1, { role: 'assistant', kind, content: (s.deltas ?? []).join(''), citations: s.citations ?? [], model: 'mock-model', createdAt: at(n + 1) }),
      );
    }
    if (!s.streamError) events.push(['done', { model: 'mock-model', messageId: '00000000-0000-4000-8000-0000000000aa', cached: false, ...s.done }]);
    return route.fulfill({ status: 200, headers: { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' }, body: sse(events) });
  });
  await page.route(/\/api\/v1\/meetings\/[^/]+\/ask\/history(\?.*)?$/, (route) => {
    const u = new URL(route.request().url());
    m.historyCalls.push(u.search);
    const limit = Number(u.searchParams.get('limit') ?? 50);
    const before = u.searchParams.get('before');
    const kind = u.searchParams.get('kind');
    let all = [...m.history].filter((h) => !kind || h.kind === kind).sort((a, b) => a.createdAt.localeCompare(b.createdAt));
    if (before) { const i = all.findIndex((h) => h.id === before); all = i < 0 ? all : all.slice(0, i); }
    const items = all.slice(-limit);
    return route.fulfill({ json: { items, nextBefore: items.length === limit && all.length > limit ? items[0]!.id : null } });
  });
  await page.route(/\/api\/v1\/meetings\/[^/]+\/prior-context$/, (route) => route.fulfill({ json: m.prior }));
  return m;
}
