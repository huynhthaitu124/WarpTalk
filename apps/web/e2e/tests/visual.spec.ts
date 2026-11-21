import type { Page } from '@playwright/test';
import { test as raw } from '@playwright/test';
import { call, createRoom, createWorkspace, expect, login, markEmailVerified, ok, rnd, seedSession } from '../fixtures';

async function setTheme(page: Page, theme: 'dark' | 'light') {
  await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
}
async function settle(page: Page) {
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(150);
}

/** Deterministic content so only the email / room code need masking. */
async function seededApp() {
  const id = rnd();
  const u = { email: `e2e-vr-${id}@example.com`, password: 'Passw0rd!e2e', fullName: 'Ada Visual' };
  await ok(call('/auth/register', { body: u }));
  markEmailVerified(u.email);
  const tokens = await login({ ...u, userId: '' });
  const ws = await createWorkspace(tokens.accessToken, 'Visual HQ');
  const live = await createRoom(tokens.accessToken, ws.id, { title: 'Quarterly planning', requiresApproval: true });
  await ok(call(`/translation-rooms/${live.id}/start`, { token: tokens.accessToken, method: 'POST' }));
  const waiting = await createRoom(tokens.accessToken, ws.id, { title: 'Customer onboarding' });
  return { tokens, ws, live, waiting };
}
const masks = (page: Page) => [page.getByTestId('account-menu'), page.locator('code'), page.getByTestId('room-code')];

raw.describe('Visual regression', () => {
  for (const theme of ['dark', 'light'] as const) {
    raw(`VR-01/02 /design (${theme})`, async ({ page }) => {
      await setTheme(page, theme);
      await page.goto('/design');
      await expect(page.getByRole('heading', { name: 'Design system', level: 1 })).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`design-${theme}.png`, { fullPage: true });
    });

    raw(`VR-03 /login (${theme})`, async ({ page }) => {
      await setTheme(page, theme);
      await page.goto('/login');
      await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`login-${theme}.png`);
    });

    raw(`VR-04 /rooms populated (${theme})`, async ({ page }) => {
      const s = await seededApp();
      await seedSession(page, s.tokens);
      await setTheme(page, theme);
      await page.goto('/rooms');
      await expect(page.getByRole('link', { name: 'Quarterly planning' })).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`rooms-${theme}.png`, { mask: [page.getByTestId('account-menu'), page.locator('code')] });
    });
  }

  raw('VR-05 room detail host view (dark)', async ({ page }) => {
    const s = await seededApp();
    await seedSession(page, s.tokens);
    await setTheme(page, 'dark');
    await page.goto(`/rooms/${s.live.id}`);
    await expect(page.getByRole('heading', { name: 'Quarterly planning', level: 1 })).toBeVisible();
    await expect(page.getByRole('table', { name: 'Participants' })).toBeVisible();
    await settle(page);
    await expect(page).toHaveScreenshot('room-detail-dark.png', { mask: masks(page) });
  });

  raw('VR-06 settings runtime (dark)', async ({ page }) => {
    const s = await seededApp();
    await seedSession(page, s.tokens);
    await setTheme(page, 'dark');
    await page.goto('/settings?tab=runtime');
    await expect(page.getByLabel('Minimum words')).toBeVisible();
    await settle(page);
    await expect(page).toHaveScreenshot('settings-runtime-dark.png', { mask: masks(page) });
  });
});
