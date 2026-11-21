import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { call, joinByCode, createRoom, createUser, expect, ok, signedInContext, test, seedSession, login, createWorkspace } from '../fixtures';
import { test as raw } from '@playwright/test';

async function setTheme(page: Page, theme: 'dark' | 'light') {
  await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
}

async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(
    bad.map((v) => `${v.id} (${v.impact}): ${v.nodes.slice(0, 3).map((n) => `${n.target.join(' ')} -> ${n.any.map((a) => a.message).join('; ')}`).join(' | ')}`),
    `axe violations on ${label}`,
  ).toEqual([]);
}

raw.describe('A11Y public pages', () => {
  for (const theme of ['dark', 'light'] as const) {
    raw(`A11Y-01 /login (${theme})`, async ({ page }) => {
      await setTheme(page, theme);
      await page.goto('/login');
      await expect(page.getByRole('heading', { name: 'Sign in' })).toBeVisible();
      await scan(page, `login ${theme}`);
    });
    raw(`A11Y-03 /design (${theme})`, async ({ page }) => {
      await setTheme(page, theme);
      await page.goto('/design');
      await expect(page.getByRole('heading', { name: 'Design system', level: 1 })).toBeVisible();
      await scan(page, `design ${theme}`);
    });
  }
  raw('A11Y-02 /register and /verify-email', async ({ page }) => {
    await page.goto('/register');
    await expect(page.getByRole('heading', { name: 'Create your account' })).toBeVisible();
    await scan(page, 'register');
    await page.goto('/verify-email?email=a%40b.co');
    await expect(page.getByRole('heading', { name: 'Verify your email' })).toBeVisible();
    await scan(page, 'verify-email');
  });
});

test.describe('A11Y signed-in pages', () => {
  test('A11Y-04 /rooms with data (dark + light)', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id, { title: 'A11y room' });
    await app.goto('/rooms');
    await expect(app.getByRole('link', { name: 'A11y room' })).toBeVisible();
    await scan(app, 'rooms dark');
    await app.getByRole('button', { name: 'Toggle theme' }).click();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'light');
    await scan(app, 'rooms light');
  });

  test('A11Y-05 room detail with lobby, participants and routes', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { requiresApproval: true });
    const g1 = await createUser('a11yg1');
    const g2 = await createUser('a11yg2');
    const c1 = await signedInContext(browser, g1, baseURL);
    const c2 = await signedInContext(browser, g2, baseURL);
    try {
      await joinByCode(c1.tokens.accessToken, room.roomCode, { displayName: 'Waiting Wendy' });
      await joinByCode(c2.tokens.accessToken, room.roomCode, { displayName: 'Admitted Al', speak: 'ja', listen: 'en' });
      const parts = await ok(call<{ id: string; displayName: string }[]>(`/translation-rooms/${room.id}/participants`, { token: tokens.accessToken }));
      const al = parts.find((p) => p.displayName === 'Admitted Al')!;
      await ok(call(`/translation-rooms/${room.id}/participants/${al.id}/admit`, { token: tokens.accessToken, method: 'PUT' }));
      await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
      await ok(call(`/translation-rooms/${room.id}/audio-routes/generate`, { token: tokens.accessToken, method: 'POST' }));
      await app.goto(`/rooms/${room.id}`);
      await expect(app.getByTestId('lobby')).toBeVisible();
      await expect(app.getByRole('table', { name: 'Participants' })).toBeVisible();
      await scan(app, 'room detail');
    } finally {
      await c1.context.close();
      await c2.context.close();
    }
  });

  test('A11Y-06 settings tabs', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id);
    await app.goto('/settings');
    await expect(app.getByLabel('Max participants')).toBeVisible();
    await scan(app, 'settings workspace');
    await app.getByRole('tab', { name: 'Members' }).click();
    await expect(app.getByRole('table', { name: 'Workspace members' })).toBeVisible();
    await scan(app, 'settings members');
    await app.getByRole('tab', { name: 'Runtime' }).click();
    await expect(app.getByLabel('Minimum words')).toBeVisible();
    await scan(app, 'settings runtime form');
    await app.getByRole('tab', { name: 'Raw JSON' }).click();
    await expect(app.getByLabel('Settings JSON')).toBeVisible();
    await scan(app, 'settings runtime json');
    await app.getByRole('tab', { name: 'Account' }).click();
    await expect(app.getByRole('heading', { name: 'Appearance' })).toBeVisible();
    await scan(app, 'settings account');
  });

  test('A11Y-07 meeting, join, meetings list', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/meetings/${room.id}`);
    await app.getByRole('button', { name: 'Join meeting' }).click();
    await expect(app.getByText('LiveKit is not configured')).toBeVisible();
    await scan(app, 'meeting not configured');
    await app.goto('/join');
    await expect(app.getByLabel('Room code')).toBeVisible();
    await scan(app, 'join');
    await app.goto('/meetings');
    await expect(app.getByRole('list', { name: 'Live meetings' })).toBeVisible();
    await scan(app, 'meetings');
  });

  test('A11Y-08 dialog and command menu', async ({ app }) => {
    await app.goto('/rooms?new=1');
    await expect(app.getByRole('dialog', { name: 'Create room' })).toBeVisible();
    await scan(app, 'create room dialog');
    await app.keyboard.press('Escape');
    await app.keyboard.press('ControlOrMeta+k');
    await expect(app.getByRole('dialog', { name: 'Command menu' })).toBeVisible();
    await scan(app, 'command menu');
  });
});

void seedSession; void login; void createWorkspace;
