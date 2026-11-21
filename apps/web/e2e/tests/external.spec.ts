import type { Page } from '@playwright/test';
import {
  addWorkspaceMember, BLACKHOLE, call, createUser, emitNative, expect, expireBridgeLease, installNativeMock, login, mockSet, nativeCalls, ok,
  seedSession, signedInContext, test, ZOOM, CHROME, type TestUser, type Tokens,
} from '../fixtures';

const letters = (n: number) => Array.from({ length: n }, () => String.fromCharCode(97 + Math.floor(Math.random() * 26))).join('');
export const meetCode = () => `${letters(3)}-${letters(4)}-${letters(3)}`;
const meetLink = (code = meetCode()) => `https://meet.google.com/${code}`;

interface BridgeRoom { id: string; status: string; roomType: string; bridge: { capturerUserId: string | null; audioMode: string; meetingCode: string }; title: string; effectiveHostId: string }
const claimBody = (workspaceId: string, code: string) => ({
  workspaceId, provider: 'google_meet', meetingUrl: meetLink(code), sourceLanguage: 'en', targetLanguages: ['vi'], displayName: 'API Host',
});
const apiClaim = (token: string, workspaceId: string, code: string) =>
  call<BridgeRoom & { created: boolean; bridgeRole: string }>('/translation-rooms/bridge/claim', { token, body: claimBody(workspaceId, code) });
const getRoom = (token: string, id: string) => ok(call<BridgeRoom>(`/translation-rooms/${id}`, { token }));

async function claimInUi(page: Page, link: string) {
  await page.getByLabel('Meeting link').fill(link);
  const resp = page.waitForResponse((r) => r.url().includes('/translation-rooms/bridge/claim') && r.request().method() === 'POST');
  await page.getByRole('button', { name: 'Claim meeting' }).click();
  return (await resp).status();
}
async function pickApp(page: Page, name: string) {
  await page.getByRole('combobox', { name: 'Source app' }).click();
  await page.getByRole('option', { name: new RegExp(name) }).click();
}
const roomCard = (page: Page) => page.getByTestId('external-room-card');

/** Owner signed in, native host mocked, claimed in the UI as capturer with Chrome selected. */
async function claimedCapturer(page: Page, opts: Parameters<typeof installNativeMock>[1] = {}) {
  await installNativeMock(page, opts);
  await page.goto('/external');
  const code = meetCode();
  await claimInUi(page, meetLink(code));
  await expect(page.getByTestId('bridge-role')).toContainText('Capturer');
  await pickApp(page, 'Google Chrome');
  return code;
}

test.describe('External meeting: browser (no native host)', () => {
  test('EXT-01 explains that capture needs the desktop app, viewer claim still works', async ({ app }) => {
    await app.goto('/external');
    await expect(app.getByRole('heading', { name: 'External meeting', level: 1 })).toBeVisible();
    const fb = app.getByTestId('external-browser-fallback');
    await expect(fb).toContainText('Capture needs the WarpTalk desktop app');
    await expect(fb).toContainText('Get the macOS app');
    await expect(fb).toContainText('Viewing only');
    await expect(app.getByTestId('native-connected')).toHaveCount(0);
    await expect(app.getByRole('combobox', { name: 'Source app' })).toHaveCount(0);

    // The claim itself works without the bridge; the capturer role then cannot capture here.
    expect(await claimInUi(app, meetLink())).toBe(201);
    await expect(app.getByTestId('bridge-role')).toContainText('Capturer');
    await expect(app.getByText('This browser cannot capture')).toBeVisible();
    await expect(app.getByRole('button', { name: 'Start capture' })).toHaveCount(0);
  });
});

test.describe('External meeting: link parsing', () => {
  const cases: { name: string; input: string; provider?: string; code?: string; error?: RegExp }[] = [
    { name: 'Meet URL', input: 'https://meet.google.com/abc-defg-hij?authuser=1', provider: 'Google Meet', code: 'abc-defg-hij' },
    { name: 'Meet bare code (upper case)', input: 'ABC-DEFG-HIJ', provider: 'Google Meet', code: 'abc-defg-hij' },
    { name: 'Zoom URL with password', input: 'https://us02web.zoom.us/j/12345678901?pwd=abcDEF', provider: 'Zoom', code: '12345678901' },
    { name: 'Zoom number with spaces', input: '123 4567 8901', provider: 'Zoom', code: '12345678901' },
    { name: 'Teams URL', input: 'https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc/0?context=%7b%7d', provider: 'Microsoft Teams', code: 'https://teams.microsoft.com/l/meetup-join/19%3ameeting_abc/0' },
    { name: 'foreign host', input: 'https://example.com/abc-defg-hij', error: /Only Google Meet, Zoom and Microsoft Teams/ },
    { name: 'malformed Meet code', input: 'https://meet.google.com/abc-def', error: /must end with a code like abc-defg-hij/ },
    { name: 'short Zoom id', input: 'https://zoom.us/j/12345', error: /Zoom link must look like/ },
    { name: 'garbage', input: 'hello', error: /Not a recognised meeting code/ },
  ];
  test('EXT-02 parses Meet, Zoom and Teams links with a provider badge and rejects the rest', async ({ app }) => {
    await app.goto('/external');
    const input = app.getByLabel('Meeting link');
    const claim = app.getByRole('button', { name: 'Claim meeting' });
    await expect(claim).toBeDisabled();
    for (const c of cases) {
      await input.fill(c.input);
      if (c.provider) {
        await expect(app.getByTestId('provider-badge'), c.name).toHaveText(c.provider);
        await expect(app.getByTestId('meeting-code'), c.name).toHaveText(c.code!);
        await expect(claim, c.name).toBeEnabled();
      } else {
        await expect(app.getByTestId('provider-badge'), c.name).toHaveCount(0);
        await expect(app.getByRole('alert').filter({ hasText: c.error! }), c.name).toBeVisible();
        await expect(claim, c.name).toBeDisabled();
      }
    }
    await input.fill('');
    await expect(claim).toBeDisabled();
  });

  test('EXT-03 same far-side and listen language is rejected', async ({ app }) => {
    await app.goto('/external');
    await app.getByLabel('Meeting link').fill(meetLink());
    await app.getByRole('combobox', { name: 'Listen language' }).click();
    await app.getByRole('option', { name: 'English' }).click();
    await expect(app.getByText('Pick a different language than the far side.')).toBeVisible();
    await expect(app.getByRole('button', { name: 'Claim meeting' })).toBeDisabled();
  });

  test('EXT-04 Zoom and Teams claims reach the backend with the right provider', async ({ app }) => {
    await app.goto('/external');
    const req = app.waitForRequest((r) => r.url().includes('/bridge/claim'));
    await app.getByLabel('Meeting link').fill(`https://zoom.us/j/${Math.floor(100000000 + Math.random() * 899999999)}`);
    await app.getByRole('button', { name: 'Claim meeting' }).click();
    const body = (await req).postDataJSON() as { provider: string; meetingUrl?: string; targetLanguages: string[]; sourceLanguage: string };
    expect(body.provider).toBe('zoom');
    expect(body.meetingUrl).toContain('zoom.us/j/');
    expect([body.sourceLanguage, body.targetLanguages]).toEqual(['en', ['vi']]);
    await expect(roomCard(app)).toContainText('Zoom');
  });
});

test.describe('External meeting: claim', () => {
  test('EXT-05 claim of a new meeting answers 201 and makes the caller capturer', async ({ app, tokens, workspace }) => {
    await installNativeMock(app);
    await app.goto('/external');
    const code = meetCode();
    expect(await claimInUi(app, meetLink(code))).toBe(201);
    await expect(roomCard(app)).toContainText('New room created for this meeting.');
    await expect(app.getByTestId('bridge-role')).toContainText('Capturer');
    await expect(roomCard(app).locator('[data-status="WAITING"]')).toBeVisible();
    await expect(roomCard(app)).toContainText(code);
    await expect(app.getByTestId('bridge-lease')).toContainText('s ago');
    // Server state agrees.
    const room = await ok(call<{ items: BridgeRoom[] }>(`/translation-rooms?pageSize=100&workspaceId=${workspace.id}`, { token: tokens.accessToken }));
    const mine = room.items.find((r) => r.bridge?.meetingCode === code);
    expect(mine).toMatchObject({ roomType: 'EXTERNAL_BRIDGE', status: 'WAITING', title: `External meeting ${code}` });
  });

  test('EXT-06 claiming an existing meeting answers 200; the same user stays capturer', async ({ app, tokens, workspace }) => {
    const code = meetCode();
    const first = await apiClaim(tokens.accessToken, workspace.id, code);
    expect(first.status).toBe(201);
    await installNativeMock(app);
    await app.goto('/external');
    expect(await claimInUi(app, meetLink(code))).toBe(200);
    await expect(roomCard(app)).toContainText('Joined the existing room for this meeting.');
    await expect(app.getByTestId('bridge-role')).toContainText('Capturer');
    await expect(app.getByRole('link', { name: 'Open meeting page' })).toHaveAttribute('href', `/meetings/${first.body.id}`);
    // "Use a different meeting" resets the form.
    await app.getByRole('button', { name: 'Use a different meeting' }).click();
    await expect(app.getByRole('button', { name: 'Claim meeting' })).toBeVisible();
    await expect(app.getByLabel('Meeting link')).toBeEnabled();
  });

  test('EXT-07 a second workspace member becomes viewer; takeover is refused while the capturer is fresh, allowed once stale', async ({ tokens, workspace, browser, baseURL }) => {
    const code = meetCode();
    const first = await apiClaim(tokens.accessToken, workspace.id, code);
    const viewer: TestUser = await createUser('viewer');
    addWorkspaceMember(workspace.id, viewer.userId);
    const v = await signedInContext(browser, viewer, baseURL);
    try {
      await installNativeMock(v.page);
      await v.page.goto('/external');
      expect(await claimInUi(v.page, meetLink(code))).toBe(200);
      await expect(v.page.getByTestId('bridge-role')).toContainText('Viewer');
      await expect(v.page.getByText('Someone else is capturing this meeting')).toBeVisible();
      await expect(v.page.getByRole('button', { name: 'Start capture' })).toHaveCount(0);

      // Fresh lease: backend answers 409 CAPTURER_ACTIVE and the UI says so.
      await v.page.getByRole('button', { name: 'Take over capture' }).click();
      await expect(v.page.getByRole('alert').filter({ hasText: 'CAPTURER_ACTIVE' })).toContainText('Another participant is currently capturing');
      expect((await getRoom(tokens.accessToken, first.body.id)).bridge.capturerUserId).toBe(first.body.effectiveHostId);

      // The capturer goes silent: lease ages out, the UI flags it as stale and takeover succeeds.
      expireBridgeLease(first.body.id);
      await expect(v.page.getByTestId('lease-stale')).toBeVisible();
      await expect(v.page.getByText('The capturer looks inactive')).toBeVisible();
      await v.page.getByRole('button', { name: 'Take over capture' }).click();
      await expect(v.page.getByTestId('bridge-role')).toContainText('Capturer');
      await expect(v.page.getByRole('heading', { name: '3. Capture' })).toBeVisible();
      expect((await getRoom(tokens.accessToken, first.body.id)).bridge.capturerUserId).toBe(viewer.userId);
    } finally {
      await v.context.close();
    }
  });
});

test.describe('External meeting: capture', () => {
  test('EXT-08 start capture starts the room, then calls the native host with a valid access token', async ({ app, tokens }) => {
    await claimedCapturer(app);
    await expect(app.getByRole('button', { name: 'Start capture' })).toBeEnabled();
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    await expect(roomCard(app).locator('[data-status="IN_PROGRESS"]')).toBeVisible();

    const [start] = await nativeCalls(app, 'startExternalCapture');
    const o = start!.args[0] as { appBundleId: string; roomId: string; mode: string; accessToken: string; heartbeatIntervalSeconds: number; listenLanguage?: string };
    expect(o.appBundleId).toBe('com.google.Chrome');
    expect(o.mode).toBe('text');
    expect(o.heartbeatIntervalSeconds).toBe(15);
    expect(o.listenLanguage).toBeUndefined();
    expect(o.accessToken.split('.')).toHaveLength(3);
    // The token is a live session token the backend accepts, for the room that was claimed.
    const room = await getRoom(o.accessToken, o.roomId);
    expect(room.bridge.capturerUserId).toBe(room.effectiveHostId);
    expect(room.bridge.audioMode).toBe('text');
    expect(room.status).toBe('IN_PROGRESS');
    void tokens;
    await expect(app.getByRole('button', { name: 'Stop capture' })).toBeVisible();
    await expect(app.getByRole('combobox', { name: 'Source app' })).toBeDisabled();
  });

  test('EXT-09 level, audioFlowing, heartbeat and status events render live', async ({ app }) => {
    await claimedCapturer(app);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    await expect(app.getByTestId('capture-backend')).toHaveText('Process tap');

    await emitNative(app, { type: 'level', rmsDbfs: -20, audioFlowing: true });
    await expect(app.getByTestId('level-text')).toHaveText('-20 dBFS');
    await expect(app.getByTestId('level-meter')).toHaveAttribute('aria-valuenow', '-20');
    await expect(app.getByTestId('level-meter')).toHaveAttribute('aria-valuetext', '-20 dBFS');
    await emitNative(app, { type: 'level', rmsDbfs: -120, audioFlowing: true });
    await expect(app.getByTestId('level-text')).toHaveText('Silent');

    await expect(app.getByTestId('audio-flowing')).toContainText('Audio flowing');
    await emitNative(app, { type: 'audioFlowing', audioFlowing: false, lastAudioAt: new Date(Date.now() - 30_000).toISOString() });
    await expect(app.getByTestId('audio-flowing')).toContainText('No audio buffers');
    await expect(app.getByTestId('last-audio')).toContainText(/\d+s ago/);
    await emitNative(app, { type: 'audioFlowing', audioFlowing: true, lastAudioAt: new Date().toISOString() });
    await expect(app.getByTestId('audio-flowing')).toContainText('Audio flowing');

    await expect(app.getByTestId('last-heartbeat')).toContainText('none yet');
    await emitNative(app, { type: 'heartbeat', ok: true, at: new Date().toISOString() });
    await expect(app.getByTestId('last-heartbeat')).toContainText('OK');
    await emitNative(app, { type: 'heartbeat', ok: false, message: 'network unreachable' });
    await expect(app.getByTestId('last-heartbeat')).toContainText('Failed');
    await expect(app.getByTestId('last-heartbeat')).toContainText('network unreachable');

    await emitNative(app, { type: 'log', message: 'Falling back to ScreenCaptureKit' });
    await expect(app.getByRole('list', { name: 'Native log' })).toContainText('Falling back to ScreenCaptureKit');
    await emitNative(app, { type: 'status', state: 'running', backend: 'screenCaptureKit', roomId: null, rmsDbfs: -30, audioFlowing: true });
    await expect(app.getByTestId('capture-backend')).toHaveText('ScreenCaptureKit');
  });

  test('EXT-10 permission denied: the error offers request + open System Settings', async ({ app }) => {
    await claimedCapturer(app, {
      permissions: { audioCapture: 'denied', screenRecording: 'notDetermined' },
      startError: { code: 'PERMISSION_DENIED', message: 'Audio capture permission was denied' },
    });
    await expect(app.getByTestId('perm-audioCapture')).toHaveAttribute('data-state', 'denied');
    await app.getByRole('button', { name: 'Start capture' }).click();
    const err = app.getByTestId('capture-error');
    await expect(err).toHaveAttribute('data-code', 'PERMISSION_DENIED');
    await expect(err).toContainText('Audio capture permission was denied');
    await expect(app.getByTestId('capture-state')).toHaveText('Idle');

    await err.getByRole('button', { name: 'Request permission' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'requestPermission')).map((c) => c.args[0])).toEqual(['screenRecording']);
    await err.getByRole('button', { name: 'Open System Settings' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'openSystemSettings')).map((c) => c.args[0])).toEqual(['audioCapture']);

    await err.getByRole('button', { name: 'Dismiss' }).click();
    await expect(app.getByTestId('capture-error')).toHaveCount(0);
  });

  test('EXT-11 no audio yet: error recovers with "Try again"', async ({ app }) => {
    await claimedCapturer(app, { startError: { code: 'NO_AUDIO_PROCESS', message: 'Chrome is not playing audio yet' } });
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-error')).toHaveAttribute('data-code', 'NO_AUDIO_PROCESS');
    await mockSet.startError(app, null);
    await app.getByRole('button', { name: 'Try again' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    await expect(app.getByTestId('capture-error')).toHaveCount(0);
    expect(await nativeCalls(app, 'startExternalCapture')).toHaveLength(2);
  });

  test('EXT-12 fatal native error event shows a recovery action', async ({ app }) => {
    await claimedCapturer(app);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    await emitNative(app, { type: 'error', code: 'LEASE_LOST', message: 'Another machine took over the capturer role', fatal: true });
    const err = app.getByTestId('capture-error');
    await expect(err).toContainText('Capture stopped');
    await expect(err).toContainText('Another machine took over');
    await expect(err.getByRole('button', { name: 'Refresh room' })).toBeVisible();
    await emitNative(app, { type: 'status', state: 'idle', roomId: null, rmsDbfs: -120, audioFlowing: false });
    await expect(app.getByTestId('capture-state')).toHaveText('Idle');
    await expect(app.getByRole('button', { name: 'Start capture' })).toBeVisible();
  });

  test('EXT-13 auth-required refreshes the session and hands the new token to native', async ({ app }) => {
    await claimedCapturer(app);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    const startToken = ((await nativeCalls(app, 'startExternalCapture'))[0]!.args[0] as { accessToken: string }).accessToken;

    await app.waitForTimeout(1100); // tokens minted in the same second are identical
    const refresh = app.waitForRequest((r) => r.url().endsWith('/auth/refresh'));
    await emitNative(app, { type: 'auth-required', reason: 'heartbeat' });
    await refresh;
    await expect.poll(async () => (await nativeCalls(app, 'updateAuth')).length).toBe(1);
    const [upd] = await nativeCalls(app, 'updateAuth');
    const newToken = (upd!.args[0] as { accessToken: string }).accessToken;
    expect(newToken.split('.')).toHaveLength(3);
    expect(newToken).not.toBe(startToken);
    await expect(app.getByText('Session refreshed')).toBeVisible();
    await expect(app.getByTestId('capture-error')).toHaveCount(0);
    expect((await call('/me/profile', { token: newToken })).status).toBe(200);
  });

  test('EXT-14 auth-required while the refresh fails surfaces an error with a working "Refresh session" recovery', async ({ app }) => {
    await claimedCapturer(app);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    // Refresh requests fail like a backend outage (the stored refresh token stays valid).
    await app.route('**/api/v1/auth/refresh', (route) => route.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ code: 'INTERNAL_ERROR', message: 'boom' }) }));
    await emitNative(app, { type: 'auth-required', reason: 'heartbeat' });
    const err = app.getByTestId('capture-error');
    await expect(err).toHaveAttribute('data-code', 'AUTH_REFRESH_FAILED');
    await expect(err).toContainText('Could not refresh the session');
    expect(await nativeCalls(app, 'updateAuth')).toHaveLength(0);
    await app.unroute('**/api/v1/auth/refresh');
    await err.getByRole('button', { name: 'Refresh session' }).click();
    await expect.poll(async () => (await nativeCalls(app, 'updateAuth')).length).toBe(1);
    await expect(app.getByTestId('capture-error')).toHaveCount(0);
  });

  test('EXT-25 auth-required after the session was revoked ends the session instead of looping', async ({ app }) => {
    await claimedCapturer(app);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    await app.route('**/api/v1/auth/refresh', (route) => route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify({ code: 'UNAUTHORIZED', message: 'invalid refresh token' }) }));
    await emitNative(app, { type: 'auth-required', reason: 'heartbeat' });
    await expect(app).toHaveURL(/\/login/);
    expect(await nativeCalls(app, 'updateAuth')).toHaveLength(0);
  });

  test('EXT-15 stop capture calls native and returns to idle', async ({ app }) => {
    await claimedCapturer(app);
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    await app.getByRole('button', { name: 'Stop capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Idle');
    expect(await nativeCalls(app, 'stopExternalCapture')).toHaveLength(1);
    await expect(app.getByRole('button', { name: 'Start capture' })).toBeEnabled();
    await expect(app.getByRole('combobox', { name: 'Source app' })).toBeEnabled();
    await expect(app.getByTestId('level-text')).toHaveText('Silent');
  });

  test('EXT-16 audio mode: text-only cannot go back to voice while translation runs (409 shown)', async ({ app }) => {
    await claimedCapturer(app, { capabilities: { outputDevices: [BLACKHOLE] } });
    await app.getByRole('button', { name: 'Start capture' }).click(); // text: starts the room + translation
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    await expect(app.getByTestId('bridge-audio-mode')).toContainText('Text only');
    await app.getByRole('button', { name: 'Stop capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Idle');

    await app.getByRole('combobox', { name: 'Mode' }).click();
    await app.getByRole('option', { name: 'Voice dub' }).click();
    await app.getByRole('button', { name: 'Start capture' }).click();
    const alert = app.getByRole('alert').filter({ hasText: 'BRIDGE_AUDIO_MODE_LOCKED' });
    await expect(alert).toContainText('Voice mode is locked');
    await expect(alert).toContainText('Text-only mode cannot return to voice while translation is running.');
    await expect(app.getByRole('combobox', { name: 'Mode' })).toContainText('Text only');
    expect(await nativeCalls(app, 'startExternalCapture')).toHaveLength(1);
  });

  test('EXT-17 voice mode needs a dub output device; with one, native gets listen language and device', async ({ app }) => {
    // Without a device the option is disabled and the page says why.
    await claimedCapturer(app);
    await app.getByRole('combobox', { name: 'Mode' }).click();
    await expect(app.getByRole('option', { name: 'Voice dub' })).toHaveAttribute('aria-disabled', 'true');
    await app.keyboard.press('Escape');
    await expect(app.getByText('Voice needs an audio output device such as BlackHole 2ch')).toBeVisible();
  });

  test('EXT-18 voice mode with BlackHole starts native in voice mode', async ({ app }) => {
    await claimedCapturer(app, { capabilities: { outputDevices: [{ uid: 'Speakers_UID', name: 'MacBook Speakers', isVirtual: false }, BLACKHOLE] } });
    await app.getByRole('combobox', { name: 'Mode' }).click();
    await app.getByRole('option', { name: 'Voice dub' }).click();
    await expect(app.getByRole('combobox', { name: 'Dub output device' })).toContainText('BlackHole 2ch (virtual)');
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    const o = (await nativeCalls(app, 'startExternalCapture'))[0]!.args[0] as Record<string, unknown>;
    expect(o).toMatchObject({ mode: 'voice', listenLanguage: 'vi', dubOutputDeviceUid: BLACKHOLE.uid });
    await expect(app.getByTestId('bridge-audio-mode')).toContainText('Voice');
  });

  test('EXT-19 source app list shows which apps are producing audio and refreshes', async ({ app }) => {
    await claimedCapturer(app, { apps: [CHROME, { ...ZOOM }] });
    await app.getByRole('combobox', { name: 'Source app' }).click();
    await expect(app.getByRole('option')).toHaveText(['Google Chrome · playing audio', 'zoom.us · silent']);
    await app.getByRole('option', { name: /zoom\.us/ }).click();
    await expect(app.getByTestId('external-capture-card').getByText('Silent', { exact: true })).toBeVisible();
    await mockSet.apps(app, [CHROME, { ...ZOOM, producingAudio: true }]);
    await app.getByRole('button', { name: 'Refresh apps' }).click();
    await expect(app.getByTestId('external-capture-card').getByText('Playing audio', { exact: true })).toBeVisible();
    await expect(app.getByRole('button', { name: 'Start capture' })).toBeEnabled();
  });

  test('EXT-20 start is blocked until a source app is chosen', async ({ app }) => {
    await installNativeMock(app);
    await app.goto('/external');
    await claimInUi(app, meetLink());
    await expect(app.getByRole('button', { name: 'Start capture' })).toBeDisabled();
    await expect(app.getByText('Choose a source app to start.')).toBeVisible();
    expect(await nativeCalls(app, 'startExternalCapture')).toHaveLength(0);
  });
});

test.describe('External meeting: permissions, overlay, captions', () => {
  test('EXT-21 permissions panel reflects native state; request and open settings call the host', async ({ app }) => {
    await installNativeMock(app, { permissions: { microphone: 'notDetermined', screenRecording: 'denied', audioCapture: 'granted' }, grantOnRequest: true });
    await app.goto('/external');
    await expect(app.getByTestId('native-connected')).toContainText('macOS 14.5.0');
    await expect(app.getByTestId('perm-microphone')).toContainText('Not requested');
    await expect(app.getByTestId('perm-screenRecording')).toContainText('Denied');
    await expect(app.getByTestId('perm-audioCapture')).toContainText('Granted');
    await expect(app.getByTestId('perm-audioCapture').getByRole('button')).toHaveCount(0);
    // audioCapture is not requestable (the OS prompts on first capture): only microphone/screenRecording get Request.
    await app.getByRole('button', { name: 'Request microphone access' }).click();
    await expect(app.getByTestId('perm-microphone')).toContainText('Granted');
    expect((await nativeCalls(app, 'requestPermission')).map((c) => c.args[0])).toEqual(['microphone']);
    await app.getByRole('button', { name: 'Open settings for screen recording' }).click();
    expect((await nativeCalls(app, 'openSystemSettings')).map((c) => c.args[0])).toEqual(['screenRecording']);
    // Re-check picks up a change made in System Settings.
    await mockSet.perms(app, { screenRecording: 'granted' });
    await app.getByRole('button', { name: 'Refresh', exact: true }).click();
    await expect(app.getByTestId('perm-screenRecording')).toContainText('Granted');
  });

  test('EXT-22 overlay switch drives showOverlay / clearOverlay / setOverlayOptions', async ({ app }) => {
    await installNativeMock(app);
    await app.goto('/external');
    const sw = app.getByRole('switch', { name: /Show caption overlay/ });
    await sw.click();
    await expect(sw).toBeChecked();
    expect((await nativeCalls(app, 'showOverlay')).map((c) => c.args[0])).toEqual([true]);
    await app.getByRole('switch', { name: /Click-through/ }).click();
    await app.getByRole('switch', { name: /Hide from screen sharing/ }).click();
    const opts = (await nativeCalls(app, 'setOverlayOptions')).map((c) => c.args[0]);
    expect(opts).toEqual([{ clickThrough: true, hideFromScreenShare: false }, { clickThrough: true, hideFromScreenShare: true }]);
    await sw.click();
    await expect(sw).not.toBeChecked();
    expect((await nativeCalls(app, 'clearOverlay'))).toHaveLength(1);
    expect((await nativeCalls(app, 'showOverlay')).map((c) => c.args[0])).toEqual([true, false]);
  });

  test('EXT-23 captions explain themselves when LiveKit is not configured', async ({ app }) => {
    await claimedCapturer(app);
    await expect(app.getByTestId('captions-log')).toContainText('Captions start when the room is live.');
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-state')).toHaveText('Running');
    await expect(app.getByTestId('captions-log')).toContainText('Captions are unavailable: the server has no LiveKit credentials');
    await expect(app.getByTestId('status-translation')).toHaveAttribute('data-state', /Active|Stopped/);
    await expect(app.getByRole('button', { name: 'Retry captions' })).toBeVisible();
  });

  test('EXT-24 native host that announces itself late (warptalk-native-ready) switches out of browser mode', async ({ app }) => {
    await app.goto('/external');
    await expect(app.getByTestId('external-browser-fallback')).toBeVisible();
    await app.evaluate(() => {
      (window as unknown as Record<string, unknown>).warptalkNative = {
        version: 1,
        getCapabilities: async () => ({ version: 1, platform: 'macos', osVersion: '15.0.0', processTap: true, screenCaptureKit: true, voiceMode: false, overlay: true, supportedApps: [], outputDevices: [], blackHoleInstalled: false }),
        listAudioApps: async () => [], captureStatus: async () => ({ state: 'idle' }),
        permissions: async () => ({ microphone: 'granted', screenRecording: 'granted', audioCapture: 'granted' }),
      };
      window.dispatchEvent(new Event('warptalk-native-ready'));
    });
    await expect(app.getByTestId('external-browser-fallback')).toHaveCount(0);
    await expect(app.getByTestId('native-connected')).toContainText('macOS 15.0.0');
  });
});

void seedSession; void login; void ({} as Tokens);
