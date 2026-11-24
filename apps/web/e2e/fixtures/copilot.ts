// In-memory stand-in for the copilot backend shapes (src/lib/copilot.ts). The real endpoints land with the backend agent;
// specs mock them so the pages are tested independently of it.
import type { Page } from '@playwright/test';

export interface MockSession {
  id: string; title: string; status: string; platform?: string; meetingUrl?: string;
  languages?: { me: string; others: string }; startedAt?: string; createdAt?: string;
}
export interface MockSegment { id: string; speakerName: string; originalText: string; startTimeMs?: number; sequenceOrder: number; isFinal?: boolean }
export interface MockActionItem { id: string; task: string; owner?: string; due?: string; status: 'open' | 'done' }
export interface MockSummary { short: string; detailed?: string; decisions?: string[]; openQuestions?: string[]; generatedAt?: string }

export interface CopilotMock {
  sessions: MockSession[];
  segments: Record<string, MockSegment[]>;
  summary: Record<string, MockSummary | undefined>;
  actionItems: Record<string, MockActionItem[]>;
  followups: Record<string, { id: string; text: string }[]>;
  /** Requests seen, "METHOD /path?query" without the origin. */
  calls: string[];
  /** Bodies of POST /copilot/sessions. */
  created: unknown[];
}

const json = (body: unknown, status = 200) => ({ status, contentType: 'application/json', body: JSON.stringify(body) });

export async function mockCopilot(page: Page, init: Partial<Pick<CopilotMock, 'sessions' | 'segments' | 'summary' | 'actionItems' | 'followups'>> = {}): Promise<CopilotMock> {
  const m: CopilotMock = { sessions: [], segments: {}, summary: {}, actionItems: {}, followups: {}, calls: [], created: [], ...init };
  await page.route(/\/api\/v1\/(copilot\/|transcripts\/|meetings\/[^/]+\/(summary|action-items|followups)|workspaces\/[^/]+\/action-items)/, async (route) => {
    const req = route.request();
    const u = new URL(req.url());
    const path = u.pathname.replace('/api/v1', '');
    m.calls.push(`${req.method()} ${path}${u.search}`);
    let x: RegExpMatchArray | null;
    if (path === '/copilot/sessions' && req.method() === 'GET') return route.fulfill(json(m.sessions));
    if (path === '/copilot/sessions' && req.method() === 'POST') {
      const b = req.postDataJSON() as Record<string, unknown>;
      m.created.push(b);
      const s: MockSession = { id: `sess-${m.sessions.length + 1}-new`, title: String(b.title), status: 'IN_PROGRESS', platform: String(b.platform), meetingUrl: b.meetingUrl as string | undefined, languages: b.languages as MockSession['languages'], createdAt: new Date().toISOString() };
      m.sessions.unshift(s);
      return route.fulfill(json(s, 201));
    }
    if ((x = path.match(/^\/copilot\/sessions\/([^/]+)$/))) {
      const s = m.sessions.find((v) => v.id === x![1]);
      return route.fulfill(s ? json(s) : json({ code: 'NOT_FOUND', message: 'session not found' }, 404));
    }
    if ((x = path.match(/^\/transcripts\/([^/]+)$/))) return route.fulfill(json({ status: 'ACTIVE', segments: m.segments[x[1]!] ?? [] }));
    if ((x = path.match(/^\/copilot\/sessions\/([^/]+)\/end$/)) && req.method() === 'POST') {
      const s = m.sessions.find((v) => v.id === x![1]);
      if (s) s.status = 'ENDED';
      return route.fulfill(s ? json(s) : json({ code: 'NOT_FOUND', message: 'session not found' }, 404));
    }
    if ((x = path.match(/^\/meetings\/([^/]+)\/summary\/regenerate$/))) {
      m.summary[x[1]!] = { short: 'Regenerated summary: the team confirmed the launch plan.', decisions: ['Ship on Friday'], generatedAt: new Date().toISOString() };
      return route.fulfill({ status: 202, body: '' });
    }
    if ((x = path.match(/^\/meetings\/([^/]+)\/summary$/))) {
      const sm = m.summary[x[1]!];
      if (!sm) return route.fulfill(json({ status: 'none', latest: null, lastCompleted: null }));
      const content = { summaryShort: sm.short, summaryDetailed: sm.detailed ?? '', decisions: sm.decisions ?? [], actionItems: [], openQuestions: sm.openQuestions ?? [], topics: [], language: 'en', meta: {} };
      const job = { status: 'completed', content, completedAt: sm.generatedAt ?? null };
      return route.fulfill(json({ status: 'completed', latest: job, lastCompleted: job, content }));
    }
    if ((x = path.match(/^\/meetings\/([^/]+)\/action-items\/([^/]+)$/)) && req.method() === 'PATCH') {
      const status = (req.postDataJSON() as { status: 'open' | 'done' }).status;
      for (const a of m.actionItems[x[1]!] ?? []) if (a.id === x[2]) a.status = status;
      return route.fulfill(json({ id: x[2], status }));
    }
    if ((x = path.match(/^\/meetings\/([^/]+)\/action-items$/))) return route.fulfill(json({ items: (m.actionItems[x[1]!] ?? []).map((a) => ({ ...a, roomId: x![1] })) }));
    if ((x = path.match(/^\/meetings\/([^/]+)\/followups$/))) return route.fulfill(json(m.followups[x[1]!] ?? []));
    if (/^\/workspaces\/[^/]+\/action-items$/.test(path)) {
      return route.fulfill(json({ items: Object.entries(m.actionItems).flatMap(([roomId, l]) => l.filter((a) => a.status === 'open').map((a) => ({ ...a, roomId }))) }));
    }
    return route.fulfill(json({ code: 'NOT_FOUND', message: 'unmocked' }, 404));
  });
  return m;
}
