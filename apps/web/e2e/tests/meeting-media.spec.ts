// Pre-join card with Chromium's fake camera/microphone (project "meeting-media", see playwright.config.ts).
import type { Page } from '@playwright/test';
import { call, createRoom, expect, ok, test } from '../fixtures';

test.beforeEach(({}, testInfo) => {
  test.skip(testInfo.project.name !== 'meeting-media', 'needs the fake-media Chromium flags of the meeting-media project');
});

async function openPreJoin(page: Page, token: string, workspaceId: string, o: { title?: string } = {}) {
  const room = await createRoom(token, workspaceId, { title: o.title ?? 'Customer call', displayName: 'Host Hana' });
  await ok(call(`/translation-rooms/${room.id}/start`, { token, method: 'POST' }));
  await page.goto(`/meetings/${room.id}`);
  await expect(page.getByTestId('meeting-prejoin')).toBeVisible();
  return room;
}

test.describe('Pre-join', () => {
  test('MTG-30 preview: camera video, mic meter moves, device pickers list the fake devices', async ({ app, tokens, workspace }) => {
    await openPreJoin(app, tokens.accessToken, workspace.id);
    const preview = app.getByTestId('camera-preview');
    await expect(preview).toHaveAttribute('data-camera', 'on');
    await expect(app.getByRole('combobox', { name: 'Microphone' })).not.toHaveText(/No microphone/);
    await expect(app.getByRole('combobox', { name: 'Camera' })).not.toHaveText(/No camera/);
    // The fake audio device beeps periodically: the meter must rise above zero at some point.
    const meter = app.getByTestId('mic-meter');
    await expect(meter).toHaveAttribute('data-active', 'true');
    await expect.poll(async () => Number(await meter.getAttribute('aria-valuenow')), { timeout: 15_000, intervals: [100] }).toBeGreaterThan(0);
    // The video element really plays the stream.
    expect(await app.locator('video[aria-label="Camera preview"]').evaluate((v: HTMLVideoElement) => v.videoWidth)).toBeGreaterThan(0);
    // Device lists are populated.
    await app.getByRole('combobox', { name: 'Camera' }).click();
    expect(await app.getByRole('option').count()).toBeGreaterThan(0);
    await app.keyboard.press('Escape');
  });

  test('MTG-31 camera toggle stops the preview track; join muted by default', async ({ app, tokens, workspace }) => {
    await openPreJoin(app, tokens.accessToken, workspace.id);
    await expect(app.getByTestId('camera-preview')).toHaveAttribute('data-camera', 'on');
    await app.getByRole('button', { name: 'Turn camera off' }).click();
    await expect(app.getByTestId('camera-preview')).toHaveAttribute('data-camera', 'off');
    await expect(app.getByText('Camera is off')).toBeVisible();
    await app.getByRole('button', { name: 'Turn camera on' }).click();
    await expect(app.getByTestId('camera-preview')).toHaveAttribute('data-camera', 'on');
    await app.getByRole('button', { name: 'Unmute microphone' }).click();
    await expect(app.getByRole('button', { name: 'Mute microphone' })).toHaveAttribute('aria-pressed', 'true');
  });

  test('MTG-32 selecting another microphone re-opens the preview on that device', async ({ app, tokens, workspace }) => {
    await openPreJoin(app, tokens.accessToken, workspace.id);
    const mic = app.getByRole('combobox', { name: 'Microphone' });
    await expect(mic).not.toHaveText(/Looking for/);
    await mic.click();
    const options = app.getByRole('option');
    const n = await options.count();
    test.skip(n < 2, 'the fake device list has a single microphone in this Chromium build');
    const label = (await options.nth(1).innerText()).trim();
    await options.nth(1).click();
    await expect(mic).toContainText(label);
    await expect(app.getByTestId('mic-meter')).toHaveAttribute('data-active', 'true');
  });

  test('MTG-33 join sends names, languages and reaches the LiveKit-not-configured state with the preview released', async ({ app, tokens, workspace }) => {
    const room = await openPreJoin(app, tokens.accessToken, workspace.id);
    await app.getByLabel('Display name (optional)').fill('Hana H.');
    const req = app.waitForRequest((r) => r.url().endsWith(`/translation-rooms/${room.id}/join`));
    await app.getByRole('button', { name: 'Join meeting' }).click();
    expect((await req).postDataJSON()).toMatchObject({ displayName: 'Hana H.', speakLanguage: 'vi' });
    await expect(app.getByText('LiveKit is not configured')).toBeVisible();
    // Tracks opened by the preview are stopped once the card is gone.
    await expect(app.locator('video[aria-label="Camera preview"]')).toHaveCount(0);
  });

  for (const theme of ['dark', 'light'] as const) {
    for (const vp of [{ n: '1440', w: 1440, h: 900 }, { n: '1280', w: 1280, h: 800 }, { n: '390', w: 390, h: 844 }]) {
      test(`MTG-34 pre-join layout ${vp.n} (${theme})`, async ({ app, tokens, workspace }) => {
        await app.setViewportSize({ width: vp.w, height: vp.h });
        await app.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await openPreJoin(app, tokens.accessToken, workspace.id, { title: 'Customer call' });
        await expect(app.getByTestId('camera-preview')).toHaveAttribute('data-camera', 'on');
        expect(await app.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        const card = (await app.getByTestId('meeting-prejoin').boundingBox())!;
        expect(card.x).toBeGreaterThanOrEqual(0);
        expect(card.x + card.width).toBeLessThanOrEqual(vp.w);
        if (vp.w >= 1024) expect(card.width).toBeGreaterThan(700); // two columns, no island
        // Fake camera pixels/clock vary: snapshot everything but the video and the level.
        await app.evaluate(() => document.fonts.ready);
        await expect(app).toHaveScreenshot(`meeting-prejoin-${vp.n}-${theme}.png`, {
          mask: [app.getByTestId('camera-preview'), app.getByTestId('mic-meter'), app.getByTestId('account-menu')],
          maxDiffPixelRatio: 0.02,
        });
      });
    }
  }
});
