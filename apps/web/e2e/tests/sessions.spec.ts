// Copilot sessions (ADR 0001): list, start dialog, detail (live transcript, suggestions, summary, action items).
// The copilot endpoints are mocked (fixtures/copilot.ts): the pages are coded against the planned backend shapes.
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import {
  FLAGS_DEFAULT, call, createWorkspace, expect, installNativeMock, login, mockCopilot, nativeCalls, ok, rnd, seedSession, test, verifyEmail,
  type MockActionItem, type MockSegment, type MockSession,
} from '../fixtures';

test.use({ flags: FLAGS_DEFAULT });

const WHEN = '2026-11-20T09:00:00Z';
const S_LIVE: MockSession = { id: 'sess-live-1', title: 'Weekly product sync', status: 'IN_PROGRESS', platform: 'google_meet', meetingUrl: 'https://meet.google.com/abc-defg-hij', languages: { me: 'en', others: 'vi' }, startedAt: WHEN, createdAt: WHEN };
const S_DONE: MockSession = { id: 'sess-done-1', title: 'Board review', status: 'ENDED', platform: 'zoom', languages: { me: 'en', others: 'en' }, startedAt: '2026-11-18T14:00:00Z', createdAt: '2026-11-18T14:00:00Z' };
const S_NEW: MockSession = { id: 'sess-new-1', title: 'Customer call', status: 'WAITING', platform: 'teams', createdAt: '2026-11-19T10:00:00Z' };
const SEGS: MockSegment[] = [
  { id: 'g1', speakerName: 'Me', originalText: 'Let us start with the launch plan.', startTimeMs: 1000, sequenceOrder: 1 },
  { id: 'g2', speakerName: 'Others', originalText: 'We need the pricing page before Friday.', startTimeMs: 65000, sequenceOrder: 2 },
];
const ITEMS = (): MockActionItem[] => [
  { id: 'ai-1', task: 'Send the pricing draft to legal', owner: 'Linh', due: 'Friday', status: 'open' },
  { id: 'ai-2', task: 'Book the launch review', status: 'open' },
  { id: 'ai-3', task: 'Share the deck', owner: 'Sam', status: 'done' },
];

const rows = (p: Page) => p.getByTestId('session-row');

test.describe('Sessions list and start dialog', () => {
  test('SESS-01 list shows the workspace sessions and opens one', async ({ app, workspace }) => {
    const m = await mockCopilot(app, { sessions: [S_LIVE, S_DONE, S_NEW] });
    await app.goto(`/${workspace.slug}/sessions`);
    await expect(rows(app)).toHaveCount(3);
    await expect(app.getByTestId('sessions-count')).toHaveText('3');
    const live = rows(app).filter({ hasText: 'Weekly product sync' });
    await expect(live.locator('[data-status="IN_PROGRESS"]')).toHaveText('Live');
    await expect(live).toContainText('Google Meet');
    await expect(rows(app).filter({ hasText: 'Board review' }).locator('[data-status="ENDED"]')).toBeVisible();
    expect(m.calls.some((c) => c === `GET /copilot/sessions?workspaceId=${workspace.id}`)).toBe(true);
    await app.getByRole('link', { name: 'Board review', exact: true }).click();
    await expect(app).toHaveURL(/\/sessions\/sess-done-1$/);
    await expect(app.getByRole('heading', { name: 'Board review', level: 1 })).toBeVisible();
    await expect(app.getByRole('navigation', { name: 'Breadcrumb' })).toContainText('Sessions');
  });

  test('SESS-02 empty state offers to start a session; "Start session" opens the dialog and ?new=1 deep-links it', async ({ app, workspace }) => {
    await mockCopilot(app);
    await app.goto(`/${workspace.slug}/sessions`);
    await expect(app.getByRole('heading', { name: 'No sessions yet' })).toBeVisible();
    await app.getByRole('button', { name: 'Start session' }).first().click();
    await expect(app.getByRole('dialog', { name: 'Start copilot session' })).toBeVisible();
    await expect(app).toHaveURL(/new=1/);
    await app.keyboard.press('Escape');
    await expect(app.getByRole('dialog')).toHaveCount(0);
    await expect(app).not.toHaveURL(/new=1/);
    await app.goto(`/${workspace.slug}/sessions?new=1`);
    await expect(app.getByRole('dialog', { name: 'Start copilot session' })).toBeVisible();
  });

  test('SESS-03 browser: the dialog explains the desktop app is required, creates the session with the documented body', async ({ app, workspace }) => {
    const m = await mockCopilot(app);
    await app.goto(`/${workspace.slug}/sessions?new=1`);
    const d = app.getByRole('dialog', { name: 'Start copilot session' });
    await expect(d.getByText('Audio capture needs the WarpTalk desktop app')).toBeVisible();
    await expect(d.getByRole('button', { name: 'Start session' })).toHaveCount(0);
    await d.getByRole('button', { name: 'Create session' }).click();
    await expect(d.getByRole('alert').filter({ hasText: 'Give the session a title.' })).toBeVisible();
    expect(m.created).toHaveLength(0);
    await d.getByLabel('Title').fill('Design review');
    await d.getByLabel('Meeting link (optional)').fill('https://zoom.us/j/123456');
    await d.getByRole('combobox', { name: 'Meeting app' }).click();
    await app.getByRole('option', { name: 'Zoom' }).click();
    await d.getByRole('combobox', { name: 'Others speak' }).click();
    await app.getByRole('option', { name: 'Vietnamese' }).click();
    await d.getByRole('button', { name: 'Create session' }).click();
    await expect(app).toHaveURL(/\/sessions\/sess-1-new$/);
    expect(m.created[0]).toEqual({ workspaceId: workspace.id, title: 'Design review', platform: 'zoom', meetingUrl: 'https://zoom.us/j/123456', languages: { me: 'en', others: 'vi' } });
    await expect(app.getByRole('heading', { name: 'Design review', level: 1 })).toBeVisible();
    // The detail repeats the explanation: nothing can capture in a browser.
    await expect(app.getByText('Audio capture needs the WarpTalk desktop app')).toBeVisible();
    await expect(app.getByRole('button', { name: /Start capture|Stop capture/ })).toHaveCount(0);
  });

  test('SESS-04 native host: no browser notice, "Start session" calls warptalkNative.startCopilotSession', async ({ app, workspace }) => {
    await installNativeMock(app);
    const m = await mockCopilot(app);
    await app.goto(`/${workspace.slug}/sessions?new=1`);
    const d = app.getByRole('dialog', { name: 'Start copilot session' });
    await expect(d.getByRole('button', { name: 'Start session' })).toBeVisible();
    await expect(d.getByText('Audio capture needs the WarpTalk desktop app')).toHaveCount(0);
    await d.getByLabel('Title').fill('Native session');
    await d.getByRole('button', { name: 'Start session' }).click();
    await expect(app).toHaveURL(/\/sessions\/sess-1-new$/);
    expect(m.created).toHaveLength(1);
    const [call1] = await nativeCalls(app, 'startCopilotSession');
    const arg = call1!.args[0] as { sessionId: string; accessToken: string; apiBaseUrl: string };
    expect(arg.sessionId).toBe('sess-1-new');
    expect(arg.accessToken.length).toBeGreaterThan(20);
    expect(arg.apiBaseUrl).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
    // Live session in the host: Stop capture calls stopCopilotSession.
    await app.getByRole('button', { name: 'Stop capture' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'stopCopilotSession')).length).toBe(1);
    await expect(app.getByText('Audio capture needs the WarpTalk desktop app')).toHaveCount(0);
  });

  test('SESS-05 a missing backend (404) reads as an empty list, not an error', async ({ app, workspace }) => {
    await app.route(/\/api\/v1\/copilot\//, (r) => r.fulfill({ status: 404, contentType: 'application/json', body: '{"code":"NOT_FOUND","message":"not found"}' }));
    await app.goto(`/${workspace.slug}/sessions`);
    await expect(app.getByRole('heading', { name: 'No sessions yet' })).toBeVisible();
    await expect(app.getByRole('alert')).toHaveCount(0);
  });

  test('SESS-06 a failing list shows the API message', async ({ app, workspace }) => {
    await app.route(/\/api\/v1\/copilot\//, (r) => r.fulfill({ status: 500, contentType: 'application/json', body: '{"code":"INTERNAL","message":"sessions exploded"}' }));
    await app.goto(`/${workspace.slug}/sessions`);
    await expect(app.getByRole('alert').filter({ hasText: 'sessions exploded' })).toBeVisible();
  });
});

test.describe('Session detail', () => {
  test('SESS-10 live: transcript polls while IN_PROGRESS and shows speakers and times', async ({ app }) => {
    const m = await mockCopilot(app, { sessions: [S_LIVE], segments: { [S_LIVE.id]: [...SEGS] }, followups: { [S_LIVE.id]: [{ id: 'f1', text: 'Confirm the pricing page owner' }] } });
    await app.goto(`/sessions/${S_LIVE.id}`);
    await expect(app.getByRole('heading', { name: 'Weekly product sync', level: 1 })).toBeVisible();
    await expect(app.getByTestId('transcript-line')).toHaveCount(2);
    const second = app.getByTestId('transcript-line').nth(1);
    await expect(second).toContainText('Others');
    await expect(second).toContainText('1:05');
    await expect(second).toContainText('We need the pricing page before Friday.');
    await expect(app.getByTestId('transcript-line').first()).toHaveAttribute('data-mine', 'true');
    await expect(app.getByTestId('transcript-live')).toBeVisible();
    // A new segment arrives on the server: the next poll shows it without a reload.
    m.segments[S_LIVE.id]!.push({ id: 'g3', speakerName: 'Others', originalText: 'Agreed, ship it on Friday.', startTimeMs: 90000, sequenceOrder: 3 });
    await expect(app.getByTestId('transcript-line')).toHaveCount(3, { timeout: 12_000 });
    await expect(app.getByTestId('transcript-count')).toHaveText('3');
    expect(m.calls.filter((c) => c === `GET /transcripts/${S_LIVE.id}`).length).toBeGreaterThan(1);
    // follow-ups from GET /meetings/{id}/followups; live suggestions stream is optional (missing stream = empty)
    await expect(app.getByTestId('followup')).toContainText('Confirm the pricing page owner');
    await expect(app.getByTestId('suggestions-empty')).toBeVisible();
    await expect(app.getByText('The summary and minutes are generated when the session ends.')).toBeVisible();
    await expect(app.getByRole('link', { name: 'Meeting link' })).toHaveAttribute('href', S_LIVE.meetingUrl!);
  });

  test('SESS-11 ended: no polling; summary, decisions and action items render', async ({ app }) => {
    const m = await mockCopilot(app, {
      sessions: [S_DONE], segments: { [S_DONE.id]: SEGS },
      summary: { [S_DONE.id]: { short: 'The board approved the Q4 budget.', decisions: ['Hire two engineers'], generatedAt: WHEN } },
      actionItems: { [S_DONE.id]: ITEMS() },
    });
    await app.goto(`/sessions/${S_DONE.id}`);
    await expect(app.getByTestId('summary')).toContainText('The board approved the Q4 budget.');
    await expect(app.getByTestId('summary')).toContainText('Hire two engineers');
    const items = app.getByTestId('action-item');
    await expect(items).toHaveCount(3);
    await expect(items.filter({ hasText: 'Send the pricing draft to legal' })).toContainText('(Linh, Friday)');
    await expect(items.filter({ hasText: 'Share the deck' })).toHaveAttribute('data-done', 'true');
    await expect(app.getByTestId('action-items-open')).toHaveText('2 open');
    await expect(app.getByTestId('transcript-live')).toHaveCount(0);
    const before = m.calls.filter((c) => c.startsWith('GET /transcripts/')).length;
    await app.waitForTimeout(3500);
    expect(m.calls.filter((c) => c.startsWith('GET /transcripts/')).length).toBe(before);
    await expect(app.getByRole('button', { name: 'Regenerate summary' })).toBeVisible();
    await expect(app.getByRole('button', { name: 'End session' })).toHaveCount(0);
    await expect(app.getByText('Audio capture needs the WarpTalk desktop app')).toHaveCount(0);
  });

  test('SESS-12 mark an action item done (and reopen it)', async ({ app }) => {
    const m = await mockCopilot(app, { sessions: [S_DONE], actionItems: { [S_DONE.id]: ITEMS() } });
    await app.goto(`/sessions/${S_DONE.id}`);
    const item = app.getByTestId('action-item').filter({ hasText: 'Book the launch review' });
    await item.getByRole('button', { name: 'Mark done: Book the launch review' }).click();
    await expect(item).toHaveAttribute('data-done', 'true');
    await expect(item.getByText('Done', { exact: true })).toBeVisible();
    await expect(app.getByTestId('action-items-open')).toHaveText('1 open');
    expect(m.actionItems[S_DONE.id]!.find((a) => a.id === 'ai-2')!.status).toBe('done');
    expect(m.calls).toContain(`PATCH /meetings/${S_DONE.id}/action-items/ai-2`);
    await item.getByRole('button', { name: 'Reopen: Book the launch review' }).click();
    await expect(item).toHaveAttribute('data-done', 'false');
    await expect(app.getByTestId('action-items-open')).toHaveText('2 open');
  });

  test('SESS-13 regenerate the summary: generates when missing, replaces it when present', async ({ app }) => {
    const m = await mockCopilot(app, { sessions: [S_DONE] });
    await app.goto(`/sessions/${S_DONE.id}`);
    await expect(app.getByTestId('summary-empty')).toHaveText('No summary yet.');
    await app.getByRole('button', { name: 'Generate summary' }).click();
    await expect(app.getByTestId('summary')).toContainText('Regenerated summary', { timeout: 12_000 });
    await expect(app.getByTestId('summary')).toContainText('Ship on Friday');
    expect(m.calls).toContain(`POST /meetings/${S_DONE.id}/summary/regenerate`);
    await expect(app.getByRole('button', { name: 'Regenerate summary' })).toBeVisible();
  });

  test('SESS-14 unknown session shows "Session not found" with a way back', async ({ app, workspace }) => {
    await mockCopilot(app);
    await app.route(/\/api\/v1\/translation-rooms\/missing-1$/, (r) => r.fulfill({ status: 404, contentType: 'application/json', body: '{"code":"NOT_FOUND","message":"room not found"}' }));
    await app.goto('/sessions/missing-1');
    await expect(app.getByRole('heading', { name: 'Session not found' })).toBeVisible();
    await app.getByRole('link', { name: 'All sessions' }).click();
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/sessions$`));
  });

  test('SESS-15 the session page falls back to the room when GET /copilot/sessions/{id} is not deployed', async ({ app }) => {
    await app.route(/\/api\/v1\/copilot\//, (r) => r.fulfill({ status: 404, contentType: 'application/json', body: '{"code":"NOT_FOUND","message":"nf"}' }));
    await app.route(/\/api\/v1\/translation-rooms\/room-9$/, (r) => r.fulfill({
      status: 200, contentType: 'application/json',
      body: JSON.stringify({ id: 'room-9', workspaceId: 'w', hostId: 'h', effectiveHostId: 'h', title: 'Room as session', roomCode: 'abcdef123456', status: 'ENDED', sourceLanguage: 'en', targetLanguages: ['vi'], requiresApproval: false, createdAt: WHEN, translationActive: false, roomType: 'COPILOT_SESSION' }),
    }));
    await mockCopilot(app); // transcripts/summary/action items still mocked (empty)
    await app.goto('/sessions/room-9');
    await expect(app.getByRole('heading', { name: 'Room as session', level: 1 })).toBeVisible();
  });
});

test.describe('Home and action items', () => {
  test('SESS-20 Home quick actions and recent sessions', async ({ app, workspace }) => {
    await mockCopilot(app, { sessions: [S_LIVE, S_DONE] });
    await app.goto(`/${workspace.slug}/home`);
    const quick = app.getByRole('region', { name: 'Quick actions' });
    await expect(quick.getByRole('link', { name: 'Start copilot session' })).toBeVisible();
    await expect(quick.getByRole('link', { name: 'Recent sessions' })).toBeVisible();
    await expect(quick.getByRole('link', { name: 'Open action items' })).toBeVisible();
    await expect(quick.getByRole('link', { name: 'New meeting' })).toHaveCount(0);
    await expect(app.getByRole('region', { name: 'Recent sessions' }).getByRole('link', { name: 'Weekly product sync' })).toBeVisible();
    await quick.getByRole('link', { name: 'Start copilot session' }).click();
    await expect(app.getByRole('dialog', { name: 'Start copilot session' })).toBeVisible();
  });

  test('SESS-21 Home empty state points to the copilot, not translation', async ({ app, workspace }) => {
    await mockCopilot(app);
    await app.goto(`/${workspace.slug}/home`);
    await expect(app.getByRole('heading', { name: 'No sessions yet' })).toBeVisible();
    await expect(app.getByText('live transcript, suggestions, follow-ups and minutes')).toBeVisible();
    await expect(app.locator('main')).not.toContainText(/translat/i);
    await app.getByRole('region', { name: 'Recent sessions' }).getByRole('button', { name: 'Start copilot session' }).click();
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/sessions\\?new=1`));
  });

  test('SESS-22 "Open action items" lists my open items across sessions and marks them done', async ({ app, workspace }) => {
    const m = await mockCopilot(app, { sessions: [S_DONE], actionItems: { [S_DONE.id]: ITEMS() } });
    await app.goto(`/${workspace.slug}/home`);
    await app.getByRole('link', { name: 'Open action items' }).click();
    await expect(app).toHaveURL(/tab=actions/);
    const items = app.getByTestId('action-item');
    await expect(items).toHaveCount(2);
    await expect(items.first().getByRole('link', { name: 'Send the pricing draft to legal' })).toHaveAttribute('href', `/sessions/${S_DONE.id}`);
    await items.filter({ hasText: 'Book the launch review' }).getByRole('button', { name: /Mark done/ }).click();
    await expect(items).toHaveCount(1);
    expect(m.calls).toContain(`PATCH /meetings/${S_DONE.id}/action-items/ai-2`);
  });
});

test.describe('Sessions against the real backend', () => {
  test.use({ flags: 'real' });

  test('SESS-30 real features endpoint: platform defaults hide translation, show the copilot', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    const nav = app.getByRole('navigation', { name: 'Primary' });
    await expect(nav.getByRole('link', { name: 'Copilot', exact: true })).toBeVisible();
    await expect(nav.getByRole('link', { name: 'Meetings' })).toHaveCount(0);
    await expect(nav.getByRole('link', { name: 'External meeting' })).toHaveCount(0);
  });

  test('SESS-31 create from the dialog, see it in the list, open it, end it', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/sessions`);
    await expect(app.getByRole('heading', { name: 'No sessions yet' })).toBeVisible();
    await app.getByRole('button', { name: 'Start session' }).first().click();
    const d = app.getByRole('dialog', { name: 'Start copilot session' });
    const title = `Real session ${rnd()}`;
    await d.getByLabel('Title').fill(title);
    await d.getByRole('button', { name: 'Create session' }).click();
    await expect(app).toHaveURL(/\/sessions\/[0-9a-f-]{36}$/);
    await expect(app.getByRole('heading', { name: title, level: 1 })).toBeVisible();
    await expect(app.locator('[data-status="IN_PROGRESS"]').first()).toHaveText('Live');
    await expect(app.getByTestId('transcript-empty')).toBeVisible();
    await expect(app.getByText('The summary and minutes are generated when the session ends.')).toBeVisible();
    await app.getByRole('button', { name: 'End session' }).click();
    await expect(app.locator('[data-status="ENDED"]').first()).toBeVisible();
    await expect(app.getByRole('button', { name: 'End session' })).toHaveCount(0);
    await expect(app.getByRole('button', { name: /generate summary|regenerate summary/i })).toBeVisible();
    await app.getByRole('link', { name: 'Sessions', exact: true }).first().click();
    await expect(app.getByRole('link', { name: title, exact: true })).toBeVisible();
  });

  test('SESS-32 a session created through the API shows in the list and on Home; other workspace members only see their own', async ({ app, tokens, workspace }) => {
    const created = await ok(call<{ id: string; roomId: string; status: string }>('/copilot/sessions', {
      token: tokens.accessToken, body: { workspaceId: workspace.id, title: 'API session', platform: 'zoom', languages: { me: 'en', others: 'vi' } },
    }));
    expect(created.id).toBe(created.roomId);
    await app.goto(`/${workspace.slug}/home`);
    await expect(app.getByRole('region', { name: 'Recent sessions' }).getByRole('link', { name: 'API session' })).toBeVisible();
    await app.goto(`/${workspace.slug}/sessions`);
    const row = app.getByTestId('session-row').filter({ hasText: 'API session' });
    await expect(row).toContainText('Zoom');
    await row.getByRole('link', { name: 'API session' }).click();
    await expect(app).toHaveURL(new RegExp(`/sessions/${created.id}$`));
    await expect(app.getByText('Me English · Others Vietnamese')).toBeVisible();
    // the end endpoint is what the button calls: ending twice is fine
    await app.getByRole('button', { name: 'End session' }).click();
    await expect(app.locator('[data-status="ENDED"]').first()).toBeVisible();
  });
});

async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${v.id} (${v.impact}): ${v.nodes.slice(0, 3).map((n) => `${n.target.join(' ')} -> ${n.any.map((a) => a.message).join('; ')}`).join(' | ')}`), `axe violations on ${label}`).toEqual([]);
}

test.describe('Sessions a11y', () => {
  test('A11Y-12 sessions list, start dialog (browser + native), detail live and ended, dark + light', async ({ app, workspace }) => {
    await mockCopilot(app, {
      sessions: [S_LIVE, S_DONE, S_NEW], segments: { [S_LIVE.id]: SEGS, [S_DONE.id]: SEGS },
      summary: { [S_DONE.id]: { short: 'The board approved the Q4 budget.', decisions: ['Hire two engineers'] } },
      actionItems: { [S_DONE.id]: ITEMS() }, followups: { [S_LIVE.id]: [{ id: 'f1', text: 'Confirm the pricing page owner' }] },
    });
    for (const theme of ['dark', 'light'] as const) {
      await app.goto(`/${workspace.slug}/sessions`);
      await app.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      await expect(rows(app)).toHaveCount(3);
      await scan(app, `sessions list ${theme}`);
      await app.getByRole('tab', { name: 'Action items' }).click();
      await expect(app.getByTestId('action-item').first()).toBeVisible();
      await scan(app, `action items tab ${theme}`);
      await app.goto(`/${workspace.slug}/sessions?new=1`);
      await app.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      await expect(app.getByRole('dialog', { name: 'Start copilot session' })).toBeVisible();
      await scan(app, `start dialog ${theme}`);
      await app.goto(`/sessions/${S_LIVE.id}`);
      await app.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      await expect(app.getByTestId('transcript-line')).toHaveCount(2);
      await scan(app, `detail live ${theme}`);
      await app.goto(`/sessions/${S_DONE.id}`);
      await app.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      await expect(app.getByTestId('summary')).toBeVisible();
      await expect(app.getByTestId('action-item')).toHaveCount(3);
      await scan(app, `detail ended ${theme}`);
    }
  });
});

// Visual snapshots: fixed data, masked account/workspace chrome (random per run).
test.describe('Sessions visual', () => {
  async function seeded(page: Page) {
    const id = rnd();
    const u = { email: `e2e-sv-${id}@example.com`, password: 'Passw0rd!e2e', fullName: 'Ada Visual' };
    await ok(call('/auth/register', { body: u }));
    await verifyEmail(u.email);
    const tokens = await login({ ...u, userId: '' });
    const ws = await createWorkspace(tokens.accessToken, 'Visual HQ');
    await seedSession(page, tokens);
    return ws;
  }
  const masks = (p: Page) => [p.getByTestId('account-menu'), p.getByTestId('workspace-switcher')];
  const sizes = [{ width: 1280, height: 800 }, { width: 390, height: 844 }];

  for (const theme of ['dark', 'light'] as const) {
    for (const vp of sizes) {
      test(`VR-20 sessions list + detail ${vp.width} (${theme})`, async ({ page }) => {
        const ws = await seeded(page);
        await mockCopilot(page, {
          sessions: [S_LIVE, S_DONE, S_NEW], segments: { [S_LIVE.id]: SEGS, [S_DONE.id]: SEGS },
          summary: { [S_DONE.id]: { short: 'The board approved the Q4 budget and agreed to review hiring monthly.', decisions: ['Hire two engineers', 'Freeze travel until Q1'], generatedAt: WHEN } },
          actionItems: { [S_DONE.id]: ITEMS() }, followups: { [S_LIVE.id]: [{ id: 'f1', text: 'Confirm the pricing page owner' }] },
        });
        await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await page.setViewportSize(vp);
        await page.goto(`/${ws.slug}/sessions`);
        await expect(rows(page)).toHaveCount(3);
        await page.evaluate(() => document.fonts.ready);
        await expect(page).toHaveScreenshot(`sessions-list-${vp.width}-${theme}.png`, { mask: masks(page) });
        await page.goto(`/sessions/${S_DONE.id}`);
        await expect(page.getByTestId('summary')).toBeVisible();
        await expect(page.getByTestId('action-item')).toHaveCount(3);
        await page.evaluate(() => document.fonts.ready);
        await expect(page).toHaveScreenshot(`session-detail-ended-${vp.width}-${theme}.png`, { mask: masks(page), fullPage: true });
        await page.goto(`/sessions/${S_LIVE.id}`);
        await expect(page.getByTestId('transcript-line')).toHaveCount(2);
        await page.evaluate(() => document.fonts.ready);
        await expect(page).toHaveScreenshot(`session-detail-live-${vp.width}-${theme}.png`, { mask: [...masks(page), page.getByTestId('transcript-live')], fullPage: true });
      });
    }
  }
  test('VR-21 start dialog (browser) 1280 + 390', async ({ page }) => {
    const ws = await seeded(page);
    await mockCopilot(page);
    for (const vp of sizes) {
      await page.setViewportSize(vp);
      await page.goto(`/${ws.slug}/sessions?new=1`);
      await expect(page.getByRole('dialog', { name: 'Start copilot session' })).toBeVisible();
      await page.evaluate(() => document.fonts.ready);
      await expect(page).toHaveScreenshot(`start-dialog-${vp.width}.png`, { mask: masks(page) });
    }
  });
});

