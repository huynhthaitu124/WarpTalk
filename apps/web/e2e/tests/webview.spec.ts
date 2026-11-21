import { createRoom, expect, test } from '../fixtures';

test.describe('macOS WebView shell', () => {
  test('WV-01 shell fits 1280x800 without horizontal scroll', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id, { title: 'Fits' });
    await app.goto('/rooms');
    await expect(app.getByRole('link', { name: 'Fits' })).toBeVisible();
    await expect(app.getByRole('complementary', { name: 'Sidebar' })).toBeVisible();
    await expect(app.getByRole('banner')).toBeVisible();
    const m = await app.evaluate(() => ({ sw: document.documentElement.scrollWidth, cw: document.documentElement.clientWidth, sh: document.documentElement.scrollHeight, ch: document.documentElement.clientHeight }));
    expect(m.sw).toBeLessThanOrEqual(m.cw);
    expect(m.sh).toBeLessThanOrEqual(m.ch + 1);
  });

  test('WV-02 reduced motion leaves no running animations', async ({ app }) => {
    await app.goto('/rooms');
    await expect(app.getByTestId('current-workspace')).toBeVisible();
    await app.waitForTimeout(300);
    const running = await app.evaluate(() => document.getAnimations().filter((a) => a.playState === 'running').length);
    expect(running).toBe(0);
  });

  test('WV-03 narrow viewport uses a navigation drawer', async ({ app }) => {
    await app.setViewportSize({ width: 390, height: 800 });
    await app.goto('/rooms');
    await expect(app.getByRole('complementary', { name: 'Sidebar' })).toBeHidden();
    await app.getByRole('button', { name: 'Open navigation' }).click();
    await expect(app.getByRole('link', { name: 'Settings' })).toBeVisible();
    await app.getByRole('link', { name: 'Settings' }).click();
    await expect(app).toHaveURL(/\/settings/);
  });
});
