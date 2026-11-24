// Live copilot session in the native host (COP-xx): start flow, capture status, partial/final segments vs the polled transcript,
// stop/end ordering, permission recovery, overlay mirror, resume. The host is the scriptable mock (fixtures/native-mock.ts).
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import {
  CHROME, FLAGS_DEFAULT, ZOOM, call, createWorkspace, emitNative, expect, installNativeMock, login, mockCopilot, mockSet, nativeCalls, ok, rnd, seedSession, startWorker, test, verifyEmail, workerAvailable,
  type MockSegment, type MockSession,
} from '../fixtures';

test.use({ flags: FLAGS_DEFAULT });

const WHEN = '2026-11-20T09:00:00Z';
const S_LIVE: MockSession = { id: 'sess-live-1', title: 'Weekly product sync', status: 'IN_PROGRESS', platform: 'google_meet', languages: { me: 'en', others: 'vi' }, startedAt: WHEN, createdAt: WHEN };
const SEGS: MockSegment[] = [
  { id: 'g1', speakerName: 'Me', originalText: 'Let us start with the launch plan.', startTimeMs: 1000, sequenceOrder: 1 },
  { id: 'g2', speakerName: 'Others', originalText: 'We need the pricing page before Friday.', startTimeMs: 65000, sequenceOrder: 2 },
];
const seg = (id: string, channel: 'me' | 'others', text: string, startMs = 2000) => ({ clientSegmentId: id, channel, text, language: 'en', startMs, endMs: startMs + 800, confidence: 0.9, isFinal: true });
const lines = (p: Page) => p.getByTestId('transcript-line');
const sse = (events: unknown[]) => events.map((e) => `event: suggestion\ndata: ${JSON.stringify(e)}\n\n`).join('');
const SUG = { suggestionId: 'sg-1', category: 'clarification', content: 'Ask which deadline applies', ttlSeconds: 600, followUp: 'Confirm the date in writing' };

test.describe('Start flow (native)', () => {
  test('COP-01 the dialog lists meeting apps and mics, creates the session and starts the host with a token', async ({ app, workspace }) => {
    await installNativeMock(app, { apps: [ZOOM, CHROME], inputDevices: [{ uid: 'mic-usb', name: 'USB Microphone' }] });
    const m = await mockCopilot(app);
    await app.goto(`/${workspace.slug}/sessions?new=1`);
    const d = app.getByRole('dialog', { name: 'Start copilot session' });
    await d.getByLabel('Title').fill('Native live');
    // The app that is playing audio is preselected.
    await expect(d.getByRole('combobox', { name: 'Capture audio from' })).toContainText('Google Chrome (playing audio)');
    await d.getByRole('combobox', { name: 'Capture audio from' }).click();
    await app.getByRole('option', { name: 'zoom.us' }).click();
    await d.getByRole('combobox', { name: 'Microphone' }).click();
    await app.getByRole('option', { name: 'USB Microphone' }).click();
    await d.getByRole('combobox', { name: 'Others speak' }).click();
    await app.getByRole('option', { name: 'Vietnamese' }).click();
    await d.getByRole('button', { name: 'Start session' }).click();
    await expect(app).toHaveURL(/\/sessions\/sess-1-new$/);
    expect(m.created).toHaveLength(1);
    const [c] = await nativeCalls(app, 'startCopilotSession');
    const arg = c!.args[0] as Record<string, unknown>;
    expect(arg).toMatchObject({ sessionId: 'sess-1-new', appBundleId: 'us.zoom.xos', micDeviceUid: 'mic-usb', languages: { me: 'en', others: 'vi' } });
    expect(String(arg.accessToken).length).toBeGreaterThan(20);
    expect(String(arg.apiBaseUrl)).toMatch(/^http:\/\/127\.0\.0\.1:\d+$/);
    // The detail attaches to the running capture: status per channel and Stop capture.
    await expect(app.getByTestId('live-capture')).toBeVisible();
    await expect(app.getByRole('button', { name: 'Stop capture' })).toBeVisible();
  });

  test('COP-02 "None: microphone only" omits the app; system default mic omits the device', async ({ app, workspace }) => {
    await installNativeMock(app, { apps: [] });
    await mockCopilot(app);
    await app.goto(`/${workspace.slug}/sessions?new=1`);
    const d = app.getByRole('dialog', { name: 'Start copilot session' });
    await d.getByLabel('Title').fill('In person');
    await expect(d.getByRole('combobox', { name: 'Capture audio from' })).toContainText('None: microphone only');
    await d.getByRole('button', { name: 'Start session' }).click();
    await expect(app).toHaveURL(/\/sessions\/sess-1-new$/);
    const [c] = await nativeCalls(app, 'startCopilotSession');
    const arg = c!.args[0] as Record<string, unknown>;
    expect(arg.appBundleId).toBeUndefined();
    expect(arg.micDeviceUid).toBeUndefined();
  });

  test('COP-03 permission error in the dialog: request permission, then try again starts the capture', async ({ app, workspace }) => {
    await installNativeMock(app, {
      permissions: { microphone: 'denied' }, grantOnRequest: true,
      copilotStartError: { code: 'PERMISSION_DENIED', message: 'Microphone access is denied' },
    });
    const m = await mockCopilot(app);
    await app.goto(`/${workspace.slug}/sessions?new=1`);
    const d = app.getByRole('dialog', { name: 'Start copilot session' });
    await d.getByLabel('Title').fill('Needs permission');
    await d.getByRole('button', { name: 'Start session' }).click();
    const err = d.getByTestId('start-error');
    await expect(err.getByTestId('capture-error')).toHaveAttribute('data-code', 'PERMISSION_DENIED');
    await expect(err).toContainText('Microphone access is denied');
    await expect(app).toHaveURL(/new=1/);
    expect(m.created).toHaveLength(1);
    await err.getByRole('button', { name: 'Open System Settings' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'openSystemSettings')).length).toBe(1);
    await err.getByRole('button', { name: 'Request permission' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'requestPermission')).length).toBe(1);
    // Fixed: the retry reuses the session that was already created (no duplicate).
    await mockSet.copilotStartError(app, null);
    await err.getByRole('button', { name: 'Try again' }).click();
    await expect(app).toHaveURL(/\/sessions\/sess-1-new$/);
    expect(m.created).toHaveLength(1);
    expect((await nativeCalls(app, 'startCopilotSession')).length).toBe(2);
  });

  test('COP-04 a failed start can still open the session without capture', async ({ app, workspace }) => {
    await installNativeMock(app, { copilotStartError: { code: 'NO_AUDIO_PROCESS', message: 'The app is not playing audio' } });
    await mockCopilot(app);
    await app.goto(`/${workspace.slug}/sessions?new=1`);
    const d = app.getByRole('dialog', { name: 'Start copilot session' });
    await d.getByLabel('Title').fill('No audio');
    await d.getByRole('button', { name: 'Start session' }).click();
    await expect(d.getByTestId('capture-error')).toHaveAttribute('data-code', 'NO_AUDIO_PROCESS');
    await d.getByRole('button', { name: 'Open the session without capture' }).click();
    await expect(app).toHaveURL(/\/sessions\/sess-1-new$/);
    await expect(app.getByRole('button', { name: 'Start capture' })).toBeVisible();
    await expect(app.getByTestId('live-capture')).toHaveCount(0);
  });
});

test.describe('Live mode on the detail page', () => {
  async function liveDetail(app: Page, opts: Parameters<typeof installNativeMock>[1] = {}) {
    await installNativeMock(app, opts);
    const m = await mockCopilot(app, { sessions: [{ ...S_LIVE }], segments: { [S_LIVE.id]: [...SEGS] } });
    await app.goto(`/sessions/${S_LIVE.id}`);
    await expect(lines(app)).toHaveCount(2);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('live-capture')).toBeVisible();
    return m;
  }

  test('COP-10 Start capture calls the host with the session and languages; status and levels per channel', async ({ app }) => {
    await liveDetail(app);
    const [c] = await nativeCalls(app, 'startCopilotSession');
    expect(c!.args[0]).toMatchObject({ sessionId: S_LIVE.id, languages: { me: 'en', others: 'vi' } });
    await expect(app.getByTestId('channel-me-status')).toHaveText('Connected');
    await emitNative(app, { type: 'copilot.levels', channel: 'me', rms: 0.1 });
    await expect(app.getByTestId('meter-me')).toHaveAttribute('data-db', '-20');
    await emitNative(app, { type: 'copilot.levels', channel: 'others', rms: -35 });
    await expect(app.getByTestId('meter-others')).toHaveAttribute('data-db', '-35');
    await emitNative(app, { type: 'copilot.status', channel: 'others', kind: 'reconnecting', detail: 'Network changed' });
    await expect(app.getByTestId('channel-others-status')).toHaveText('Reconnecting');
    await expect(app.getByTestId('channel-others')).toContainText('Network changed');
    await expect(app.getByTestId('channel-me-status')).toHaveText('Connected');
    await emitNative(app, { type: 'copilot.status', channel: 'others', kind: 'connected' });
    await expect(app.getByTestId('channel-others-status')).toHaveText('Connected');
  });

  test('COP-11 partial lines show above the transcript and are replaced by the final segment, which dedupes against the poll', async ({ app }) => {
    const m = await liveDetail(app);
    await emitNative(app, { type: 'copilot.partial', channel: 'others', text: 'and then we should' });
    await expect(app.getByTestId('partial-line')).toHaveCount(1);
    await expect(app.getByTestId('partial-line')).toContainText('and then we should');
    // The partial sits above the persisted lines.
    const [pBox, tBox] = [await app.getByTestId('partial-line').boundingBox(), await lines(app).first().boundingBox()];
    expect(pBox!.y).toBeLessThan(tBox!.y);
    await emitNative(app, { type: 'copilot.partial', channel: 'me', text: 'yes I agree' });
    await expect(app.getByTestId('partial-line')).toHaveCount(2);
    // Final segment: the partial of that side goes, the line appears right away (pending the poll).
    await emitNative(app, { type: 'copilot.segment', segment: seg('c-1', 'others', 'And then we should ship on Friday.', 90000) });
    await expect(app.getByTestId('partial-line')).toHaveCount(1);
    await expect(lines(app)).toHaveCount(3);
    await expect(lines(app).last()).toHaveAttribute('data-pending', 'true');
    // Same event twice: still one line.
    await emitNative(app, { type: 'copilot.segment', segment: seg('c-1', 'others', 'And then we should ship on Friday.', 90000) });
    await expect(lines(app)).toHaveCount(3);
    // The poll now returns it (server id = clientSegmentId in the mock): the pending line is replaced, not duplicated.
    m.segments[S_LIVE.id]!.push({ id: 'c-1', speakerName: 'Others', originalText: 'And then we should ship on Friday.', startTimeMs: 90000, sequenceOrder: 3 });
    await expect(lines(app).last()).not.toHaveAttribute('data-pending', 'true', { timeout: 12_000 });
    await expect(lines(app)).toHaveCount(3);
    await expect(app.getByTestId('transcript-count')).toHaveText('3');
    // A non-final segment is just a partial.
    await emitNative(app, { type: 'copilot.segment', segment: { ...seg('c-2', 'others', 'interim text'), isFinal: false } });
    await expect(app.getByTestId('partial-line').filter({ hasText: 'interim text' })).toBeVisible();
    await expect(lines(app)).toHaveCount(3);
  });

  test('COP-12 stop capture calls the host and clears the live panel', async ({ app }) => {
    await liveDetail(app);
    await app.getByRole('button', { name: 'Stop capture' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'stopCopilotSession')).length).toBe(1);
    await expect(app.getByTestId('live-capture')).toHaveCount(0);
    await expect(app.getByRole('button', { name: 'Start capture' })).toBeVisible();
    // The session itself keeps running (stop capture is not end session).
    await expect(app.locator('[data-status="IN_PROGRESS"]').first()).toBeVisible();
    await expect(app.getByRole('button', { name: 'End session' })).toBeVisible();
  });

  test('COP-13 End session stops the host first, then ends the session', async ({ app }) => {
    const m = await liveDetail(app);
    await emitNative(app, { type: 'copilot.partial', channel: 'me', text: 'wrapping up' });
    await app.getByRole('button', { name: 'End session' }).click();
    await expect(app.locator('[data-status="ENDED"]').first()).toBeVisible();
    expect((await nativeCalls(app, 'stopCopilotSession')).length).toBe(1);
    expect(m.stoppedBeforeEnd).toEqual([true]);
    expect(m.calls.filter((c) => c.startsWith('POST /copilot/sessions/') && c.endsWith('/end'))).toHaveLength(1);
    await expect(app.getByTestId('live-capture')).toHaveCount(0);
    await expect(app.getByTestId('partial-line')).toHaveCount(0);
  });

  test('COP-14 an error event offers recovery; permission errors request access or open System Settings', async ({ app }) => {
    await liveDetail(app, { permissions: { microphone: 'denied' }, grantOnRequest: true });
    await emitNative(app, { type: 'copilot.error', channel: 'me', code: 'PERMISSION_DENIED', message: 'Microphone access was revoked' });
    const err = app.getByTestId('capture-error');
    await expect(err).toHaveAttribute('data-code', 'PERMISSION_DENIED');
    await expect(err).toContainText('Microphone access was revoked');
    await expect(app.getByTestId('channel-me-status')).toHaveText('Problem');
    await err.getByRole('button', { name: 'Request permission' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'requestPermission')).length).toBe(1);
    await err.getByRole('button', { name: 'Open System Settings' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'openSystemSettings')).length).toBe(1);
    await err.getByRole('button', { name: 'Dismiss' }).click();
    await expect(app.getByTestId('capture-error')).toHaveCount(0);
  });

  test('COP-15 Start capture failing on the detail shows the error with Try again', async ({ app }) => {
    await installNativeMock(app, { copilotStartError: { code: 'PERMISSION_DENIED', message: 'Audio capture is denied' } });
    await mockCopilot(app, { sessions: [{ ...S_LIVE }] });
    await app.goto(`/sessions/${S_LIVE.id}`);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-error')).toContainText('Audio capture is denied');
    await expect(app.getByTestId('live-capture')).toHaveCount(0);
    await mockSet.copilotStartError(app, null);
    await app.getByTestId('capture-error').getByRole('button', { name: 'Try again' }).click();
    await expect(app.getByTestId('live-capture')).toBeVisible();
    await expect(app.getByTestId('capture-error')).toHaveCount(0);
  });

  test('COP-16 resume: a reload attaches to the capture the host is still running', async ({ app }) => {
    await liveDetail(app);
    await app.reload();
    await expect(app.getByTestId('live-capture')).toBeVisible();
    await expect(app.getByRole('button', { name: 'Stop capture' })).toBeVisible();
    await expect(app.getByRole('button', { name: 'Start capture' })).toHaveCount(0);
    await emitNative(app, { type: 'copilot.partial', channel: 'others', text: 'after the reload' });
    await expect(app.getByTestId('partial-line')).toContainText('after the reload');
  });

  test('COP-17 a capture running for another session is announced; Start capture is blocked', async ({ app }) => {
    await installNativeMock(app);
    await mockCopilot(app, { sessions: [{ ...S_LIVE }, { ...S_LIVE, id: 'sess-other-1', title: 'Other call' }] });
    await app.goto(`/sessions/${S_LIVE.id}`);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('live-capture')).toBeVisible();
    await app.goto('/sessions/sess-other-1');
    const alert = app.getByRole('status').filter({ hasText: 'capturing another session' });
    await expect(alert).toBeVisible();
    await expect(alert.getByRole('link', { name: 'Open the session being captured' })).toHaveAttribute('href', `/sessions/${S_LIVE.id}`);
    await expect(app.getByRole('button', { name: 'Start capture' })).toBeDisabled();
    await expect(app.getByTestId('live-capture')).toHaveCount(0);
  });

  test('COP-20 suggestions are mirrored to the overlay and cleared on dismiss', async ({ app }) => {
    await installNativeMock(app);
    await app.route(`**/api/v1/meetings/${S_LIVE.id}/suggestions/stream*`, (r) => r.fulfill({
      status: 200, contentType: 'text/event-stream', body: `: keep-alive\n\n${sse([SUG, { ...SUG, suggestionId: 'sg-2', category: 'risk', content: 'Budget is not confirmed', followUp: undefined }])}`,
    }));
    await mockCopilot(app, { sessions: [{ ...S_LIVE }], followups: { [S_LIVE.id]: [{ id: 'f1', text: 'Confirm the pricing page owner' }] } });
    await app.goto(`/sessions/${S_LIVE.id}`);
    // Not capturing: shown on the page, nothing is sent to the overlay.
    await expect(app.getByTestId('suggestion')).toHaveCount(2);
    expect(await nativeCalls(app, 'pushOverlaySuggestion')).toHaveLength(0);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'pushOverlaySuggestion')).length).toBe(2);
    const pushed = (await nativeCalls(app, 'pushOverlaySuggestion')).map((c) => c.args[0]);
    expect(pushed).toContainEqual({ id: 'sg-1', category: 'clarification', content: 'Ask which deadline applies', followUp: 'Confirm the date in writing' });
    expect(pushed).toContainEqual({ id: 'sg-2', category: 'risk', content: 'Budget is not confirmed' });
    // The follow-ups list stays on the page.
    await expect(app.getByTestId('followup')).toContainText('Confirm the pricing page owner');
    await app.getByRole('button', { name: 'Dismiss suggestion: Ask which deadline applies' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'clearOverlaySuggestion')).map((c) => c.args[0])).toEqual(['sg-1']);
    // The stream re-delivers sg-1 on reconnect: it stays dismissed and is not pushed again.
    await app.waitForTimeout(2500);
    expect(await nativeCalls(app, 'pushOverlaySuggestion')).toHaveLength(2);
    // Stopping capture clears what is left.
    await app.getByRole('button', { name: 'Stop capture' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'clearOverlaySuggestion')).map((c) => c.args[0]).sort()).toEqual(['sg-1', 'sg-2']);
  });

  test('COP-21 browser (no native host): unchanged explanation, no live panel, no capture buttons', async ({ app }) => {
    await mockCopilot(app, { sessions: [{ ...S_LIVE }], segments: { [S_LIVE.id]: [...SEGS] } });
    await app.goto(`/sessions/${S_LIVE.id}`);
    await expect(lines(app)).toHaveCount(2);
    await expect(app.getByText('Audio capture needs the WarpTalk desktop app')).toBeVisible();
    await expect(app.getByTestId('live-capture')).toHaveCount(0);
    await expect(app.getByRole('button', { name: /Start capture|Stop capture/ })).toHaveCount(0);
    await expect(app.getByRole('button', { name: 'End session' })).toBeVisible();
  });

  test('COP-22 a host without the copilot methods behaves like a browser', async ({ app }) => {
    await app.addInitScript(() => {
      const noop = () => Promise.resolve(null);
      (window as unknown as Record<string, unknown>).warptalkNative = { version: 1, listAudioApps: () => Promise.resolve([]), captureStatus: () => Promise.resolve({ state: 'idle' }), updateAuth: noop, permissions: noop };
    });
    await mockCopilot(app, { sessions: [{ ...S_LIVE }], segments: { [S_LIVE.id]: [...SEGS] } });
    await app.goto(`/sessions/${S_LIVE.id}`);
    await expect(app.getByText('Audio capture needs the WarpTalk desktop app')).toBeVisible();
    await expect(app.getByRole('button', { name: /Start capture/ })).toHaveCount(0);
  });
});

test.describe('Live segments against the real backend', () => {
  test.use({ flags: FLAGS_DEFAULT });
  let stopWorker = () => {};
  test.beforeAll(async () => { if (workerAvailable()) stopWorker = await startWorker(); });
  test.afterAll(() => stopWorker());

  test('COP-30 segments posted through the API (what the host sends) appear once: native events and the poll dedupe', async ({ app, tokens, workspace }) => {
    test.skip(!workerAvailable(), 'target/debug/warptalk-worker is not built (cargo build -p warptalk-worker)');
    const created = await ok(call<{ id: string }>('/copilot/sessions', {
      token: tokens.accessToken, body: { workspaceId: workspace.id, title: `Live ${rnd()}`, platform: 'zoom', languages: { me: 'en', others: 'en' } },
    }));
    await installNativeMock(app);
    await app.goto(`/sessions/${created.id}`);
    await expect(app.getByTestId('transcript-empty')).toBeVisible();
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('live-capture')).toBeVisible();
    const a = seg(`cs-${rnd()}`, 'me', 'We should confirm the launch date.', 1000);
    const b = seg(`cs-${rnd()}`, 'others', 'Friday works for the whole team.', 4000);
    // The host emits the segment events and posts the same segments to the API.
    await emitNative(app, { type: 'copilot.segment', segment: a });
    await emitNative(app, { type: 'copilot.segment', segment: b });
    await expect(lines(app)).toHaveCount(2);
    const r = await ok(call<{ segments: { clientSegmentId: string; segmentId: string; duplicate: boolean }[] }>(`/copilot/sessions/${created.id}/segments`, {
      token: tokens.accessToken, body: { segments: [a, b] },
    }));
    expect(r.segments.map((s) => s.duplicate)).toEqual([false, false]);
    // Persisted lines replace the pending ones (matched by the server id = uuid v5 of the client id): never 4 lines.
    await expect(app.getByTestId('transcript-count')).toHaveText('2', { timeout: 20_000 });
    await expect(lines(app)).toHaveCount(2);
    await expect(lines(app).first()).toContainText('We should confirm the launch date.');
    await expect(lines(app).first()).toHaveAttribute('data-mine', 'true');
    await expect(lines(app).last()).toContainText('Others');
    await expect.poll(() => app.locator('[data-testid="transcript-line"][data-pending]').count(), { timeout: 20_000 }).toBe(0);
    // A resent segment from the host (same clientSegmentId) stays one line.
    await emitNative(app, { type: 'copilot.segment', segment: a });
    await app.waitForTimeout(3500);
    await expect(lines(app)).toHaveCount(2);
    await app.getByRole('button', { name: 'End session' }).click();
    await expect(app.locator('[data-status="ENDED"]').first()).toBeVisible();
    expect((await nativeCalls(app, 'stopCopilotSession')).length).toBe(1);
  });
});

async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${v.id} (${v.impact}): ${v.nodes.slice(0, 3).map((n) => `${n.target.join(' ')} -> ${n.any.map((a) => a.message).join('; ')}`).join(' | ')}`), `axe violations on ${label}`).toEqual([]);
}

async function liveState(page: Page) {
  await installNativeMock(page, { apps: [CHROME, ZOOM], inputDevices: [{ uid: 'mic-usb', name: 'USB Microphone' }] });
  await page.route(`**/api/v1/meetings/${S_LIVE.id}/suggestions/stream*`, (r) => r.fulfill({ status: 200, contentType: 'text/event-stream', body: `: keep-alive\n\n${sse([SUG])}` }));
  await mockCopilot(page, { sessions: [{ ...S_LIVE }], segments: { [S_LIVE.id]: [...SEGS] }, followups: { [S_LIVE.id]: [{ id: 'f1', text: 'Confirm the pricing page owner' }] } });
  await page.goto(`/sessions/${S_LIVE.id}`);
  await expect(lines(page)).toHaveCount(2);
  await page.getByRole('button', { name: 'Start capture' }).click();
  await expect(page.getByTestId('live-capture')).toBeVisible();
  await emitNative(page, { type: 'copilot.levels', channel: 'me', rms: -24 });
  await emitNative(page, { type: 'copilot.levels', channel: 'others', rms: -41 });
  await emitNative(page, { type: 'copilot.status', channel: 'others', kind: 'reconnecting', detail: 'Network changed' });
  await emitNative(page, { type: 'copilot.partial', channel: 'others', text: 'and then we should' });
  await emitNative(page, { type: 'copilot.segment', segment: seg('c-9', 'me', 'Sounds good to me.', 80000) });
  await expect(page.getByTestId('suggestion')).toHaveCount(1);
  await expect(page.getByTestId('partial-line')).toHaveCount(1);
}

test.describe('Live mode a11y', () => {
  test('A11Y-13 live detail and the native start dialog (with an error), dark + light', async ({ app, workspace }) => {
    for (const theme of ['dark', 'light'] as const) {
      await liveState(app);
      await app.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      await scan(app, `live detail ${theme}`);
      await emitNative(app, { type: 'copilot.error', channel: 'me', code: 'PERMISSION_DENIED', message: 'Microphone access was revoked' });
      await expect(app.getByTestId('capture-error')).toBeVisible();
      await scan(app, `live detail with error ${theme}`);
      await app.goto(`/${workspace.slug}/sessions?new=1`);
      await app.evaluate((t) => { document.documentElement.dataset.theme = t; }, theme);
      await expect(app.getByRole('dialog', { name: 'Start copilot session' })).toBeVisible();
      await expect(app.getByTestId('capture-sources')).toBeVisible();
      await scan(app, `native start dialog ${theme}`);
      await app.evaluate(() => sessionStorage.removeItem('__mock_copilot'));
    }
  });
});

test.describe('Live mode visual', () => {
  const sizes = [{ width: 1280, height: 800 }, { width: 390, height: 844 }];
  for (const theme of ['dark', 'light'] as const) {
    for (const vp of sizes) {
      test(`VR-22 live detail ${vp.width} (${theme})`, async ({ page }) => {
        const id = rnd();
        const u = { email: `e2e-lv-${id}@example.com`, password: 'Passw0rd!e2e', fullName: 'Ada Visual' };
        await ok(call('/auth/register', { body: u }));
        await verifyEmail(u.email);
        const tokens = await login({ ...u, userId: '' });
        await createWorkspace(tokens.accessToken, 'Visual HQ');
        await seedSession(page, tokens);
        await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await page.setViewportSize(vp);
        await liveState(page);
        await page.evaluate(() => document.fonts.ready);
        await expect(page).toHaveScreenshot(`session-live-capture-${vp.width}-${theme}.png`, {
          mask: [page.getByTestId('account-menu'), page.getByTestId('workspace-switcher'), page.getByTestId('transcript-live')], fullPage: true,
        });
      });
    }
  }
  test('VR-23 native start dialog 1280 + 390', async ({ page }) => {
    const id = rnd();
    const u = { email: `e2e-lv-${id}@example.com`, password: 'Passw0rd!e2e', fullName: 'Ada Visual' };
    await ok(call('/auth/register', { body: u }));
    await verifyEmail(u.email);
    const tokens = await login({ ...u, userId: '' });
    const ws = await createWorkspace(tokens.accessToken, 'Visual HQ');
    await seedSession(page, tokens);
    await installNativeMock(page, { apps: [CHROME, ZOOM], inputDevices: [{ uid: 'mic-usb', name: 'USB Microphone' }] });
    await mockCopilot(page);
    for (const vp of sizes) {
      await page.setViewportSize(vp);
      await page.goto(`/${ws.slug}/sessions?new=1`);
      await expect(page.getByTestId('capture-sources')).toBeVisible();
      await expect(page.getByRole('combobox', { name: 'Capture audio from' })).toContainText('Google Chrome');
      await page.evaluate(() => document.fonts.ready);
      await expect(page).toHaveScreenshot(`start-dialog-native-${vp.width}.png`, { mask: [page.getByTestId('account-menu'), page.getByTestId('workspace-switcher')] });
    }
  });
});
