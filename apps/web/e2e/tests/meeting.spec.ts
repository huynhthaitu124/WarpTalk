import type { Page } from '@playwright/test';
import { addWorkspaceMember, call, createRoom, createUser, expect, ok, signedInContext, test } from '../fixtures';

const HARNESS = '/e2e/harness/captions.tsx';
interface Harness {
  messages: { payload: string; now: number }[];
  translationLang: string;
  room: { status: string; translationActive: boolean };
  connection: string;
  quality: string;
}
/** Mounts the real caption UI on a public page with scripted data-channel payloads (no LiveKit needed). */
async function mountCaptions(page: Page, input: Harness) {
  await page.goto('/login');
  await page.evaluate(async ({ path, i }) => {
    const m = await import(/* @vite-ignore */ path);
    const el = document.createElement('div');
    el.id = 'harness';
    document.body.append(el);
    m.mount(el, i);
  }, { path: HARNESS, i: input });
}
const NOW = 1_700_000_000_000;
const msg = (o: Record<string, unknown>, ago?: number) => ({
  payload: JSON.stringify({ meeting_id: 'm', speaker_id: 'Ana', is_early: false, ...(ago === undefined ? {} : { emittedAtMs: NOW - ago }), ...o }),
  now: NOW,
});

test.describe('Meeting shell', () => {
  test('MTG-01 join shows "LiveKit not configured" with roster and disabled mic', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Standup', displayName: 'Host Hana' });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/meetings/${room.id}`);
    await expect(app.getByRole('heading', { name: 'Standup', level: 1 })).toBeVisible();
    await expect(app.getByTestId('meeting-prejoin')).toBeVisible();
    await app.getByRole('button', { name: 'Join meeting' }).click();
    await expect(app.getByText('LiveKit is not configured')).toBeVisible();
    await expect(app.getByTestId('livekit-not-configured')).toContainText('LIVEKIT_URL');
    await expect(app.getByRole('button', { name: 'Unmute' })).toBeDisabled();
    await expect(app.getByTestId('captions-log')).toContainText('Captions are unavailable');
    await expect(app.getByTestId('participant-grid').getByText('Host Hana').first()).toBeVisible();
  });

  test('MTG-02 listen-language selector', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/meetings/${room.id}`);
    const sel = app.getByRole('combobox', { name: 'Listen language' });
    await sel.click();
    await expect(app.getByRole('option')).toHaveText(['Original audio', 'Dubbed: Vietnamese', 'Dubbed: English', 'Dubbed: Japanese']);
    await app.getByRole('option', { name: 'Dubbed: Japanese' }).click();
    await expect(sel).toContainText('Dubbed: Japanese');
  });

  test('MTG-03 meetings list shows live rooms only', async ({ app, tokens, workspace }) => {
    const live = await createRoom(tokens.accessToken, workspace.id, { title: 'Live one' });
    await ok(call(`/translation-rooms/${live.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const ended = await createRoom(tokens.accessToken, workspace.id, { title: 'Done one' });
    await ok(call(`/translation-rooms/${ended.id}/end`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto('/meetings');
    const list = app.getByRole('list', { name: 'Live meetings' });
    await expect(list.getByText('Live one')).toBeVisible();
    await expect(list.getByText('Done one')).toHaveCount(0);
    await app.getByRole('link', { name: 'Open meeting Live one' }).click();
    await expect(app).toHaveURL(new RegExp(`/meetings/${live.id}$`));
  });

  test('MTG-04 try again returns to pre-join', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/meetings/${room.id}`);
    await app.getByRole('button', { name: 'Join meeting' }).click();
    await expect(app.getByText('LiveKit is not configured')).toBeVisible();
    await app.getByRole('button', { name: 'Try again' }).click();
    await expect(app.getByTestId('meeting-prejoin')).toBeVisible();
  });

  test('MTG-05 host ends the meeting (LiveKit not configured): confirm dialog, room ends, back on room details', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Wrap up' });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/meetings/${room.id}`);
    await app.getByRole('button', { name: 'End meeting' }).click();
    const dialog = app.getByRole('dialog', { name: 'End meeting for everyone?' });
    await expect(dialog).toBeVisible();
    // Cancel leaves everything untouched.
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    await expect(dialog).toHaveCount(0);
    expect((await ok(call<{ status: string }>(`/translation-rooms/${room.id}`, { token: tokens.accessToken }))).status).toBe('IN_PROGRESS');

    await app.getByRole('button', { name: 'End meeting' }).click();
    // Without LiveKit the meeting-end call answers 404/503; the app still ends the room itself.
    const endMeeting = app.waitForResponse((r) => r.url().includes(`/meetings/${room.id}/end`));
    await app.getByRole('dialog').getByRole('button', { name: 'End meeting' }).click();
    expect([404, 503]).toContain((await endMeeting).status());
    await expect(app).toHaveURL(new RegExp(`/rooms/${room.id}$`));
    await expect(app.getByText('Meeting ended')).toBeVisible();
    await expect(app.locator('[data-status="ENDED"]').first()).toBeVisible();
    expect((await ok(call<{ status: string }>(`/translation-rooms/${room.id}`, { token: tokens.accessToken }))).status).toBe('ENDED');

    // The meeting page of an ended room is closed: no join, no End button.
    await app.goto(`/meetings/${room.id}`);
    await expect(app.getByText('This meeting has ended')).toBeVisible();
    await expect(app.getByRole('button', { name: 'Join meeting' })).toHaveCount(0);
    await expect(app.getByRole('button', { name: 'End meeting' })).toHaveCount(0);
  });

  test('MTG-06 only the host sees End meeting', async ({ tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Hosted' });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const member = await createUser('member');
    addWorkspaceMember(workspace.id, member.userId);
    const m = await signedInContext(browser, member, baseURL);
    try {
      await m.page.goto(`/meetings/${room.id}`);
      await expect(m.page.getByRole('heading', { name: 'Hosted', level: 1 })).toBeVisible();
      await expect(m.page.getByRole('button', { name: 'End meeting' })).toHaveCount(0);
      await expect(m.page.getByRole('button', { name: 'Join meeting' })).toBeVisible();
    } finally {
      await m.context.close();
    }
  });

  test('MTG-07 status indicators: connection, network and translation state with text labels', async ({ app, tokens, workspace }) => {
    const waiting = await createRoom(tokens.accessToken, workspace.id, { title: 'Not live yet' });
    await app.goto(`/meetings/${waiting.id}`);
    const strip = app.getByRole('list', { name: 'Meeting status' });
    await expect(strip.getByTestId('status-connection')).toHaveText('Connection: Not connected');
    await expect(strip.getByTestId('status-quality')).toHaveText('Network: n/a');
    await expect(strip.getByTestId('status-translation')).toHaveText('Translation: Not started');

    const live = await createRoom(tokens.accessToken, workspace.id, { title: 'Live translated' });
    await ok(call(`/translation-rooms/${live.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/meetings/${live.id}`);
    await expect(app.getByTestId('status-translation')).toHaveAttribute('data-state', /^(Active|Stopped)$/);
    await ok(call(`/translation-rooms/${live.id}/pause`, { token: tokens.accessToken, method: 'POST' }));
    await expect(app.getByTestId('status-translation')).toHaveText('Translation: Paused'); // room polls every 3s
  });

  test('MTG-08 captions show original + translation per line with latency; translations merge into the same line', async ({ page }) => {
    await mountCaptions(page, {
      translationLang: 'en',
      room: { status: 'IN_PROGRESS', translationActive: true },
      connection: 'connected',
      quality: 'good',
      messages: [
        // original first (empty translations), then the translated caption of the same segment
        msg({ type: 'stt', segment_id: 's1', text: 'Xin chào mọi người', lang: 'vi', translations: {} }, 300),
        msg({ type: 'translation', segment_id: 's1', text: 'Xin chào mọi người', lang: 'vi', translations: { en: 'Hello everyone' } }, 450),
        // translated only into another language than the listener's
        msg({ type: 'stt', segment_id: 's2', text: 'Cảm ơn', lang: 'vi', translations: {} }, 100),
        msg({ type: 'translation', segment_id: 's2', text: 'Cảm ơn', lang: 'vi', translations: { ja: 'ありがとう' } }, 120),
        // interim sentence, stale emitted time (clock skew) and a plain-text payload
        msg({ type: 'stt', segment_id: 's3', text: 'Tôi nghĩ rằng', lang: 'vi', is_early: true, translations: {} }, 600_000),
        { payload: 'plain words', now: NOW },
      ],
    });
    const lines = page.getByTestId('caption-line');
    await expect(lines).toHaveCount(4); // s1, s2, s3, plain
    const l1 = lines.nth(0);
    await expect(l1.getByTestId('caption-original')).toHaveText('Xin chào mọi người');
    await expect(l1.getByTestId('caption-translation')).toHaveText('Hello everyone');
    await expect(l1.getByTestId('caption-latency')).toHaveText('450 ms');
    await expect(l1).toContainText('Ana');
    const l2 = lines.nth(1);
    await expect(l2.getByTestId('caption-translation')).toHaveCount(0);
    await expect(l2).toContainText('Translating to English…');
    const l3 = lines.nth(2);
    await expect(l3.getByTestId('caption-latency')).toHaveCount(0); // 10 minutes: skew, not latency
    await expect(l3).toContainText('Tôi nghĩ rằng');
    await expect(l3).not.toContainText('Translating'); // not final yet
    await expect(lines.nth(3).getByTestId('caption-original')).toHaveText('plain words');
    await expect(page.getByTestId('status-connection')).toHaveText('Connection: Connected');
    await expect(page.getByTestId('status-quality')).toHaveText('Network: Good');
    await expect(page.getByTestId('status-translation')).toHaveText('Translation: Active');
  });

  test('MTG-09 original audio listeners see the first available translation; connection states map to labels', async ({ page }) => {
    await mountCaptions(page, {
      translationLang: 'original',
      room: { status: 'PAUSED', translationActive: true },
      connection: 'reconnecting',
      quality: 'poor',
      messages: [msg({ type: 'translation', segment_id: 'a', text: 'Hola', lang: 'es', translations: { en: 'Hello' } }, 50)],
    });
    await expect(page.getByTestId('caption-translation')).toHaveText('Hello');
    await expect(page.getByTestId('status-connection')).toHaveText('Connection: Reconnecting');
    await expect(page.getByTestId('status-quality')).toHaveText('Network: Poor');
    await expect(page.getByTestId('status-translation')).toHaveText('Translation: Paused');
  });
});
