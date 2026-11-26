// "Ask AI" (WarpBot): streamed answers with safe markdown and citation chips, quick recaps, paged history, gate and
// ANSWER_BLOCKED handling, abort, the meeting side-panel tab, a11y and visual snapshots.
// The e2e backend has no LLM provider: streaming is served by `mockAssistant` (fixtures/assistant.ts); the gate error
// LLM_NOT_CONFIGURED is also checked against the real backend (ASK-12).
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import {
  FLAGS_DEFAULT, call, createCopilotSession, createRoom, createWorkspace, expect, hist, login, mockAssistant, mockCopilot, ok, rnd, seedSession, test, verifyEmail,
  type MockHistoryItem, type MockSegment, type MockSession,
} from '../fixtures';
import { mountMeeting } from '../harness/helpers';

test.use({ flags: FLAGS_DEFAULT });

const WHEN = '2026-11-20T09:00:00Z';
const S_END: MockSession = { id: 'sess-ask-1', title: 'Pricing review', status: 'ENDED', platform: 'google_meet', languages: { me: 'en', others: 'en' }, startedAt: WHEN, createdAt: WHEN };
const SEGS: MockSegment[] = [
  { id: 'g1', speakerName: 'Me', originalText: 'Let us start with the launch plan.', startTimeMs: 1000, sequenceOrder: 1 },
  { id: 'g2', speakerName: 'Others', originalText: 'We need the pricing page before Friday.', startTimeMs: 65000, sequenceOrder: 2 },
  { id: 'g3', speakerName: 'Me', originalText: 'Agreed, I will draft it tonight.', startTimeMs: 90000, sequenceOrder: 3 },
];
const input = (p: Page) => p.getByRole('textbox', { name: 'Question for the AI assistant' });
const answers = (p: Page) => p.getByTestId('ask-answer');
const msgs = (p: Page) => p.getByTestId('ask-message');
async function ask(p: Page, q: string) {
  await input(p).fill(q);
  await p.getByTestId('ask-send').click();
}
async function open(app: Page, sess: MockSession = S_END) {
  const m = await mockCopilot(app, { sessions: [{ ...sess }], segments: { [sess.id]: [...SEGS] } });
  await app.goto(`/sessions/${sess.id}`);
  await expect(app.getByTestId('transcript-line')).toHaveCount(3);
  await expect(app.getByTestId('ask-panel')).toBeVisible();
  return m;
}

test.describe('Ask AI: answers', () => {
  test('ASK-01 an answer streams into the conversation; the request carries question and scope', async ({ app }) => {
    const a = await mockAssistant(app, { script: [{ deltas: ['We agreed ', 'to ship on Friday.'] }] });
    await open(app);
    await expect(app.getByTestId('ask-empty')).toBeVisible();
    await ask(app, 'When do we ship?');
    await expect(app.locator('[data-testid="ask-message"][data-role="user"]')).toContainText('When do we ship?');
    await expect(answers(app)).toHaveText('We agreed to ship on Friday.');
    expect(a.requests).toEqual([{ kind: 'ask', body: { question: 'When do we ship?', scope: 'meeting+docs' }, stream: true }]);
    await expect(input(app)).toHaveValue('');
    // The finished turn is handed over to the persisted history: shown once, not twice.
    await expect(app.locator('[data-testid="ask-message"][data-role="assistant"]')).toHaveCount(1);
    await expect(msgs(app)).toHaveCount(2);
  });

  test('ASK-02 the scope selector changes what is sent; Enter sends, Shift+Enter adds a line', async ({ app }) => {
    const a = await mockAssistant(app);
    await open(app);
    await app.getByRole('combobox', { name: 'Search in' }).click();
    await app.getByRole('option', { name: 'Whole workspace' }).click();
    await input(app).fill('line one');
    await input(app).press('Shift+Enter');
    await input(app).type('line two');
    await input(app).press('Enter');
    await expect(answers(app)).toHaveCount(1);
    expect(a.requests[0]!.body).toEqual({ question: 'line one\nline two', scope: 'workspace' });
    await app.getByRole('combobox', { name: 'Search in' }).click();
    await app.getByRole('option', { name: 'This meeting' }).click();
    await ask(app, 'second');
    await expect.poll(() => a.requests.length).toBe(2);
    expect(a.requests[1]!.body.scope).toBe('meeting');
  });

  test('ASK-03 markdown renders safely: formatting yes, raw HTML and javascript: links no', async ({ app }) => {
    await mockAssistant(app, {
      script: [{ deltas: ['## Plan\n**Bold** and *soft* and `code`. <img src=x onerror="window.__pwned=1"> <script>window.__pwned=1</script>\n\n- one\n- two\n\n1. first\n2. second\n\n[docs](https://example.com/a) [evil](javascript:alert(1))'] }],
    });
    await open(app);
    await ask(app, 'format please');
    const a = answers(app).first();
    await expect(a.getByRole('heading', { name: 'Plan' })).toBeVisible();
    await expect(a.locator('strong')).toHaveText('Bold');
    await expect(a.locator('em')).toHaveText('soft');
    await expect(a.locator('code')).toHaveText('code');
    await expect(a.locator('ul li')).toHaveCount(2);
    await expect(a.locator('ol li')).toHaveCount(2);
    const link = a.getByRole('link', { name: 'docs' });
    await expect(link).toHaveAttribute('href', 'https://example.com/a');
    await expect(link).toHaveAttribute('rel', /noopener/);
    await expect(link).toHaveAttribute('target', '_blank');
    await expect(a.locator('img, script')).toHaveCount(0);
    await expect(a.getByRole('link', { name: 'evil' })).toHaveCount(0);
    await expect(a).toContainText('<script>window.__pwned=1</script>'); // shown as text
    expect(await app.evaluate(() => (window as unknown as { __pwned?: number }).__pwned)).toBeUndefined();
  });

  test('ASK-04 citation chips: tags map to sources; a transcript chip highlights and scrolls to the cited line, a document chip opens Documents', async ({ app, workspace }) => {
    await mockAssistant(app, {
      script: [{
        deltas: ['Pricing is due before Friday [T1]. The refund rules are in the handbook [D1]. Also [T9].'],
        citations: [{ kind: 'transcript', ref: 'g2', label: 'Others 1:05', tMs: 65000 }, { kind: 'document', ref: 'chunk-77', label: 'Refund handbook', page: 3 }],
      }],
    });
    await open(app);
    await ask(app, 'What is due and where are refunds covered?');
    const a = answers(app).first();
    await expect(a.getByTestId('citation')).toHaveCount(2); // [T9] points nowhere: plain muted text
    await expect(a).toContainText('[T9]');
    const sources = app.getByTestId('ask-sources');
    await expect(sources.getByTestId('citation')).toHaveCount(2);
    await expect(sources.locator('[data-kind="document"]')).toContainText('Refund handbook, p. 3');
    await sources.locator('[data-kind="transcript"]').click();
    const line = app.locator('[data-testid="transcript-line"][data-line-id="g2"]');
    await expect(line).toHaveAttribute('data-highlight', 'true');
    await expect(app.locator('[data-testid="transcript-line"][data-highlight="true"]')).toHaveCount(1);
    await expect(line).toBeInViewport();
    // A citation without a loaded segment falls back to the nearest line at or before the offset.
    await a.locator('[data-kind="document"]').click();
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/documents\\?search=Refund%20handbook$`));
    await expect(app.getByLabel('Search documents')).toHaveValue('Refund handbook');
  });

  test('ASK-05 a time-only transcript citation highlights the nearest earlier line', async ({ app }) => {
    await mockAssistant(app, { script: [{ deltas: ['Said later [T1]'], citations: [{ kind: 'transcript', ref: 'unknown-segment', label: 'Me 1:40', tMs: 100000 }] }] });
    await open(app);
    await ask(app, 'when?');
    await app.getByTestId('ask-sources').getByTestId('citation').click();
    await expect(app.locator('[data-testid="transcript-line"][data-line-id="g3"]')).toHaveAttribute('data-highlight', 'true');
  });
});

test.describe('Ask AI: errors', () => {
  const GATES: [number, string, RegExp][] = [
    [403, 'EXTERNAL_LLM_NOT_ALLOWED', /does not allow sending meeting content to an external AI provider/],
    [403, 'ASSISTANT_CHAT_DISABLED', /assistant is turned off/],
    [409, 'LLM_NOT_CONFIGURED', /No AI provider is configured/],
    [429, 'RATE_LIMITED', /asking too quickly/],
    [400, 'QUESTION_EMPTY', /Type a question first/],
    [400, 'QUESTION_TOO_LONG', /question is too long/],
    [400, 'INVALID_RECAP_WINDOW', /recap window is not available/],
  ];
  for (const [status, code, text] of GATES) {
    test(`ASK-06 gate error ${code} (${status}) shows a friendly message, nothing streams`, async ({ app }) => {
      const a = await mockAssistant(app, { script: [{ gate: { status, code, message: 'raw backend text' } }] });
      await open(app);
      await ask(app, 'anything');
      await expect(app.getByTestId('ask-error')).toContainText(text);
      await expect(app.getByTestId('ask-error')).not.toContainText('raw backend text');
      await expect(answers(app)).toHaveCount(0);
      expect(a.requests).toHaveLength(1);
      // The panel stays usable.
      await expect(app.getByTestId('ask-send')).toBeDisabled(); // input was cleared
      await input(app).fill('again');
      await expect(app.getByTestId('ask-send')).toBeEnabled();
    });
  }

  test('ASK-07 ANSWER_BLOCKED discards the text that already streamed', async ({ app }) => {
    await mockAssistant(app, {
      script: [{ deltas: ['The secret password is ', 'hunter2'], citations: [{ kind: 'transcript', ref: 'g1', label: 'Me 0:01', tMs: 1000 }], streamError: { code: 'ANSWER_BLOCKED', message: 'blocked' } }],
    });
    await open(app);
    await ask(app, 'leak it');
    await expect(app.getByTestId('ask-error')).toContainText('withheld');
    await expect(app.getByText('hunter2')).toHaveCount(0);
    await expect(app.getByText('secret password')).toHaveCount(0);
    await expect(app.getByTestId('citation')).toHaveCount(0);
    await expect(answers(app)).toHaveCount(0);
  });

  test('ASK-08 an error mid-stream keeps the partial text and says it was cut off', async ({ app }) => {
    await mockAssistant(app, { script: [{ deltas: ['Half an answer'], streamError: { code: 'LLM_FAILED', message: 'upstream 500' } }] });
    await open(app);
    await ask(app, 'q');
    await expect(answers(app)).toHaveText('Half an answer');
    await expect(app.getByTestId('ask-error')).toContainText('failed to answer');
    await expect(app.getByText('The answer was cut off.')).toBeVisible();
  });

  test('ASK-09 Stop aborts the request and keeps what arrived; leaving the page aborts a running answer', async ({ app }) => {
    const a = await mockAssistant(app, { script: [{ hold: true }] });
    await open(app);
    await ask(app, 'slow one');
    await expect(app.getByText('Thinking…')).toBeVisible();
    await expect(app.getByTestId('ask-send')).toBeDisabled();
    await app.getByTestId('ask-stop').click();
    await expect.poll(() => a.aborted).toBe(1);
    await expect(app.getByTestId('ask-stop')).toHaveCount(0);
    await expect(app.getByText('Thinking…')).toHaveCount(0);
    // Unmount (navigate to the list) while a second request is pending.
    await ask(app, 'another slow one');
    await expect.poll(() => a.requests.length).toBe(2);
    await app.getByRole('navigation', { name: 'Breadcrumb' }).getByRole('link', { name: 'Sessions' }).click();
    await expect(app).toHaveURL(/\/sessions$/);
    await expect.poll(() => a.aborted).toBe(2);
  });

  test('ASK-12 real backend without an AI provider answers LLM_NOT_CONFIGURED (gate error before the stream)', async ({ app, tokens, workspace }) => {
    const s = await createCopilotSession(tokens.accessToken, workspace.id, 'Real gate');
    await app.goto(`/sessions/${s.id}`);
    await expect(app.getByTestId('ask-empty')).toBeVisible(); // real, empty history
    await ask(app, 'What was decided?');
    await expect(app.getByTestId('ask-error')).toContainText('No AI provider is configured');
    await expect(answers(app)).toHaveCount(0);
    await app.getByTestId('recap-5').click();
    await expect(app.getByTestId('ask-error')).toHaveCount(2);
  });

  test('ASK-13 question length is capped client-side before any request', async ({ app }) => {
    const a = await mockAssistant(app);
    await open(app);
    await input(app).fill('x'.repeat(1001));
    await expect(app.getByText('Questions are limited to 1000 characters.')).toBeVisible();
    await expect(app.getByTestId('ask-send')).toBeDisabled();
    expect(a.requests).toHaveLength(0);
  });
});

test.describe('Ask AI: recap and history', () => {
  const RECAP = '## Recap\nThe team settled the launch.\n\n## Open questions\n- Who owns pricing?\n\n## Decisions so far\n- Ship Friday [T1]';
  test('ASK-10 quick recap buttons send lastMinutes 5/10/15/null and render the sections; a cached recap is flagged', async ({ app }) => {
    const a = await mockAssistant(app, { script: [{ deltas: [RECAP], citations: [{ kind: 'transcript', ref: 'g2', label: 'Others 1:05', tMs: 65000 }] }, { deltas: [RECAP], done: { cached: true } }, { deltas: ['Fine.'] }, { deltas: ['Fine.'] }] });
    await open(app);
    await app.getByTestId('recap-10').click();
    const first = answers(app).first();
    await expect(first.getByRole('heading', { name: 'Recap' })).toBeVisible();
    await expect(first.getByRole('heading', { name: 'Open questions' })).toBeVisible();
    await expect(first.getByRole('heading', { name: 'Decisions so far' })).toBeVisible();
    await expect(first.getByTestId('citation')).toHaveCount(1);
    await expect(app.locator('[data-testid="ask-message"][data-role="user"]').first()).toContainText('Recap of the last 10 minutes');
    await app.getByTestId('recap-5').click();
    await expect(app.getByText('From a recent recap')).toBeVisible();
    await app.getByTestId('recap-15').click();
    await app.getByTestId('recap-all').click();
    await expect.poll(() => a.requests.length).toBe(4);
    expect(a.requests.map((r) => [r.kind, r.body])).toEqual([['recap', { lastMinutes: 10 }], ['recap', { lastMinutes: 5 }], ['recap', { lastMinutes: 15 }], ['recap', { lastMinutes: null }]]);
  });

  test('ASK-11 history pages back 20 at a time and keeps the view when older items load', async ({ app }) => {
    const items: MockHistoryItem[] = Array.from({ length: 45 }, (_, i) => hist(i + 1, i % 2 === 0
      ? { role: 'user', content: `Question ${i + 1}` }
      : { role: 'assistant', content: `Answer ${i + 1}`, model: 'mock-model', citations: [] }));
    const a = await mockAssistant(app, { history: items });
    await open(app);
    await expect(msgs(app)).toHaveCount(20);
    await expect(app.getByText('Answer 44', { exact: true })).toBeVisible(); // newest at the bottom
    await expect(app.getByText('Answer 26', { exact: true })).toBeAttached();
    await expect(app.getByText('Question 25', { exact: true })).toHaveCount(0);
    expect(a.historyCalls[0]).toContain('limit=20');
    const log = app.getByTestId('ask-log');
    await expect.poll(() => log.evaluate((el) => el.scrollHeight - el.scrollTop - el.clientHeight)).toBeLessThan(5); // opened at the bottom
    await app.getByTestId('ask-load-earlier').click();
    await expect(msgs(app)).toHaveCount(40);
    await expect(app.getByText('Answer 6', { exact: true })).toBeAttached();
    expect(a.historyCalls[1]).toContain('before=');
    await expect(app.getByText('Answer 44', { exact: true })).toBeInViewport(); // the view stayed on the newest messages
    await app.getByTestId('ask-load-earlier').click();
    await expect(msgs(app)).toHaveCount(45);
    await expect(app.getByTestId('ask-load-earlier')).toHaveCount(0);
    await expect(app.getByText('Question 1', { exact: true })).toBeAttached();
  });

  test('ASK-14 a history assistant message keeps its citation chips', async ({ app }) => {
    await mockAssistant(app, { history: [hist(1, { content: 'What about pricing?' }), hist(2, { role: 'assistant', content: 'Before Friday [T1].', citations: [{ kind: 'transcript', ref: 'g2', label: 'Others 1:05', tMs: 65000 }], model: 'm' })] });
    await open(app);
    await expect(answers(app)).toContainText('Before Friday');
    await app.getByTestId('ask-sources').getByTestId('citation').click();
    await expect(app.locator('[data-testid="transcript-line"][data-line-id="g2"]')).toHaveAttribute('data-highlight', 'true');
  });
});

test.describe('Ask AI: meeting side panel', () => {
  test('ASK-15 the meeting panel has an Ask AI tab; a transcript citation switches to the Transcript tab', async ({ app, tokens, user, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Design sync', displayName: 'Host Hana' });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const a = await mockAssistant(app, { script: [{ deltas: ['It was Friday [T1].'], citations: [{ kind: 'transcript', ref: 's1', label: 'Ana 0:10', tMs: 10000 }] }] });
    await mountMeeting(app, 'live', { roomId: room.id, selfUserId: user.userId, selfName: 'Host Hana', participants: [{ identity: user.userId, name: 'Host Hana', isLocal: true }] });
    await expect(app.getByTestId('meeting-room')).toBeVisible();
    const tab = app.getByRole('tab', { name: 'Ask AI' });
    await tab.click();
    await ask(app, 'When?');
    await expect(answers(app)).toContainText('It was Friday');
    expect(a.requests[0]!.body.question).toBe('When?');
    await app.getByTestId('ask-sources').getByTestId('citation').click();
    await expect(app.getByRole('tab', { name: 'Transcript' })).toHaveAttribute('aria-selected', 'true');
  });
});

// ---- a11y and visual
async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${v.id} (${v.impact}): ${v.nodes.slice(0, 3).map((n) => `${n.target.join(' ')} -> ${n.any.map((a) => a.message).join('; ')}`).join(' | ')}`), `axe violations on ${label}`).toEqual([]);
}
const VIEWPORTS = [{ name: '1280', width: 1280, height: 800 }, { name: '390', width: 390, height: 844 }] as const;
const CONVO: MockHistoryItem[] = [
  hist(1, { content: 'What did we decide about pricing?' }),
  hist(2, { role: 'assistant', content: 'The team decided to **ship the pricing page before Friday** [T1]. Refund rules are in the handbook [D1].\n\n- Draft tonight\n- Review tomorrow', citations: [{ kind: 'transcript', ref: 'g2', label: 'Others 1:05', tMs: 65000 }, { kind: 'document', ref: 'c1', label: 'Refund handbook', page: 3 }], model: 'mock-model' }),
  hist(3, { content: 'Recap of the last 10 minutes', kind: 'recap' }),
  hist(4, { role: 'assistant', kind: 'recap', content: '## Recap\nLaunch plan agreed.\n\n## Open questions\n- Who owns pricing?\n\n## Decisions so far\n- Ship Friday', model: 'mock-model' }),
];

test.describe('Ask AI: a11y', () => {
  for (const theme of ['dark', 'light'] as const) {
    for (const vp of VIEWPORTS) {
      test(`ASK-16 axe: session with conversation, error and recap (${theme}, ${vp.name})`, async ({ app }) => {
        await app.setViewportSize(vp);
        await app.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await mockAssistant(app, { history: CONVO, script: [{ gate: { status: 409, code: 'LLM_NOT_CONFIGURED', message: 'x' } }] });
        await open(app);
        await expect(answers(app)).toHaveCount(2);
        await ask(app, 'trigger an error');
        await expect(app.getByTestId('ask-error')).toBeVisible();
        expect(await app.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await scan(app, `ask panel ${theme} ${vp.name}`);
      });
    }
  }
});

test.describe('Ask AI: visual regression', () => {
  for (const theme of ['dark', 'light'] as const) {
    for (const vp of VIEWPORTS) {
      test(`VR-ASK ask panel ${theme} ${vp.name}`, async ({ page }) => {
        const id = rnd();
        const u = { email: `e2e-vrask-${id}@example.com`, password: 'Passw0rd!e2e', fullName: 'Ada Visual' };
        await ok(call('/auth/register', { body: u }));
        await verifyEmail(u.email);
        const tokens = await login({ ...u, userId: '' });
        await createWorkspace(tokens.accessToken, 'Visual HQ');
        await seedSession(page, tokens);
        await page.setViewportSize(vp);
        await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await mockAssistant(page, { history: CONVO });
        await open(page);
        await expect(answers(page)).toHaveCount(2);
        const panel = page.getByTestId('ask-panel');
        await panel.scrollIntoViewIfNeeded();
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(150);
        await expect(panel).toHaveScreenshot(`ask-panel-${theme}-${vp.name}.png`);
      });
    }
  }
});
