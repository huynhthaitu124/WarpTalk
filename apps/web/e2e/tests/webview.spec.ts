import { createRoom, expect, test } from '../fixtures';

test.describe('macOS WebView shell', () => {
  test('WV-01 shell fits 1280x800 without horizontal scroll', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id, { title: 'Fits' });
    await app.goto('/rooms');
    await expect(app.getByRole('link', { name: 'Fits', exact: true })).toBeVisible();
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

  test('WV-03 narrow viewport uses a navigation drawer', async ({ app, workspace }) => {
    await app.setViewportSize({ width: 390, height: 800 });
    await app.goto(`/${workspace.slug}/meetings`);
    await expect(app.getByRole('complementary', { name: 'Sidebar' })).toBeHidden();
    await app.getByRole('button', { name: 'Open navigation' }).click();
    const drawer = app.getByRole('dialog', { name: 'Navigation' });
    await expect(drawer.getByRole('link', { name: 'Settings' })).toBeVisible();
    await drawer.getByRole('link', { name: 'Settings' }).click();
    await expect(app).toHaveURL(/\/settings/);
    await expect(drawer).toHaveCount(0);
  });

  test('WV-04 mobile drawer is a real dialog: focus trap, Escape, focus return (A11Y-04)', async ({ app, workspace }) => {
    await app.setViewportSize({ width: 390, height: 800 });
    await app.goto(`/${workspace.slug}/home`);
    await expect(app.getByTestId('current-workspace')).toBeHidden();
    const opener = app.getByRole('button', { name: 'Open navigation' });
    await opener.focus();
    await app.keyboard.press('Enter');
    const drawer = app.getByRole('dialog', { name: 'Navigation' });
    await expect(drawer).toBeVisible();
    // let the sidebar finish loading (its rows re-render when the workspace list arrives)
    await expect(drawer.getByTestId('current-workspace')).not.toHaveText('No workspace');
    await drawer.getByRole('button', { name: 'Close menu' }).focus();
    await expect(drawer.getByRole('button', { name: 'Close menu' })).toBeFocused();
    for (let i = 0; i < 14; i++) {
      await app.keyboard.press('Tab');
      expect(await app.evaluate(() => !!document.activeElement?.closest('[role="dialog"]')), `tab ${i} stays in the drawer: ${await app.evaluate(() => document.activeElement?.outerHTML.slice(0, 120))}`).toBe(true);
    }
    await app.keyboard.press('Escape');
    await expect(drawer).toHaveCount(0);
    await expect(opener).toBeFocused();
  });

  test('WV-05 mobile drawer closes from its button and on the overlay', async ({ app, workspace }) => {
    await app.setViewportSize({ width: 390, height: 800 });
    await app.goto(`/${workspace.slug}/home`);
    await app.getByRole('button', { name: 'Open navigation' }).click();
    await app.getByRole('button', { name: 'Close menu' }).click();
    await expect(app.getByRole('dialog', { name: 'Navigation' })).toHaveCount(0);
    await app.getByRole('button', { name: 'Open navigation' }).click();
    await app.mouse.click(380, 400);
    await expect(app.getByRole('dialog', { name: 'Navigation' })).toHaveCount(0);
  });
});
