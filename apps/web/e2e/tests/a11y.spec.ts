import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { call, joinByCode, createRoom, createUser, expect, installNativeMock, ok, signedInContext, test, seedSession, login, createWorkspace } from '../fixtures';
import { test as raw } from '@playwright/test';

const randomMeetCode = () => {
  const l = (n: number) => Array.from({ length: n }, () => String.fromCharCode(97 + Math.floor(Math.random() * 26))).join('');
  return `${l(3)}-${l(4)}-${l(3)}`;
};

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
  test('A11Y-04 meetings list with data (dark + light)', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id, { title: 'A11y room' });
    await app.goto(`/${workspace.slug}/meetings`);
    await expect(app.getByRole('link', { name: 'A11y room', exact: true })).toBeVisible();
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
      await app.getByRole('button', { name: 'Show' }).click();
      await expect(app.getByRole('table', { name: 'Audio routes' }).or(app.getByText(/No routes yet/))).toBeVisible();
      await scan(app, 'room detail with routes');
    } finally {
      await c1.context.close();
      await c2.context.close();
    }
  });

  test('A11Y-06 settings sections', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/${workspace.slug}/settings/general`);
    await expect(app.getByLabel('Max participants')).toBeVisible();
    await scan(app, 'settings workspace');
    await app.goto(`/${workspace.slug}/members`);
    await expect(app.getByRole('table', { name: 'Workspace members' })).toBeVisible();
    await scan(app, 'members');
    await app.goto(`/${workspace.slug}/settings/advanced`);
    await expect(app.getByLabel('Minimum words')).toBeVisible();
    await scan(app, 'settings runtime form');
    await app.getByRole('tab', { name: 'Raw JSON' }).click();
    await expect(app.getByLabel('Settings JSON')).toBeVisible();
    await scan(app, 'settings runtime json');
    await app.goto(`/${workspace.slug}/settings/ai`);
    await expect(app.getByRole('heading', { name: 'Provider keys' })).toBeVisible();
    await scan(app, 'settings ai');
    await app.goto(`/${workspace.slug}/settings/account`);
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
    await expect(app.getByRole('list', { name: 'Meetings' })).toBeVisible();
    await scan(app, 'meetings');
  });

  test('A11Y-09 /external in a browser (no native host), dark + light', async ({ app }) => {
    await app.goto('/external');
    await expect(app.getByTestId('external-browser-fallback')).toBeVisible();
    await scan(app, 'external browser dark');
    await app.getByLabel('Meeting link').fill('https://meet.google.com/abc-defg-hij');
    await expect(app.getByTestId('provider-badge')).toBeVisible();
    await app.getByRole('button', { name: 'Toggle theme' }).click();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'light');
    await scan(app, 'external browser light');
    await app.getByLabel('Meeting link').fill('nonsense');
    await expect(app.getByRole('alert').filter({ hasText: 'Not a recognised meeting code' })).toBeVisible();
    await scan(app, 'external invalid link light');
  });

  test('A11Y-10 /external with native host: claimed room, capture error, permissions (dark + light)', async ({ app }) => {
    await installNativeMock(app, {
      permissions: { audioCapture: 'denied', screenRecording: 'notDetermined' },
      startError: { code: 'PERMISSION_DENIED', message: 'Audio capture permission was denied' },
      capabilities: { outputDevices: [{ uid: 'bh', name: 'BlackHole 2ch', isVirtual: true }] },
    });
    await app.goto('/external');
    await app.getByLabel('Meeting link').fill(`https://meet.google.com/${randomMeetCode()}`);
    await app.getByRole('button', { name: 'Claim meeting' }).click();
    await expect(app.getByTestId('bridge-role')).toContainText('Capturer');
    await app.getByRole('combobox', { name: 'Source app' }).click();
    await app.getByRole('option', { name: /Google Chrome/ }).click();
    await app.getByRole('button', { name: 'Start capture' }).click();
    await expect(app.getByTestId('capture-error')).toBeVisible();
    await expect(app.getByTestId('perm-audioCapture')).toHaveAttribute('data-state', 'denied');
    // (primary hover is now AA with white text, so no need to park the pointer)
    await scan(app, 'external capture dark');
    await app.getByRole('button', { name: 'Toggle theme' }).click();
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'light');
    await scan(app, 'external capture light');
  });

  test('A11Y-11 meeting page with the End meeting dialog open', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'A11y end' });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/meetings/${room.id}`);
    await expect(app.getByTestId('status-strip')).toBeVisible();
    await scan(app, 'meeting with status strip');
    await app.getByRole('button', { name: 'End meeting' }).click();
    await expect(app.getByRole('dialog', { name: 'End meeting for everyone?' })).toBeVisible();
    await scan(app, 'end meeting dialog');
  });

  test('A11Y-08 dialog and command menu', async ({ app }) => {
    await app.goto('/rooms?new=1');
    await expect(app.getByRole('dialog', { name: 'Create meeting' })).toBeVisible();
    await scan(app, 'create meeting dialog');
    await app.getByRole('button', { name: 'Create meeting' }).click();
    await expect(app.getByRole('alert').filter({ hasText: 'Give the meeting a title.' })).toBeVisible();
    await scan(app, 'create meeting dialog with errors');
    await app.keyboard.press('Escape');
    await app.keyboard.press('ControlOrMeta+k');
    await expect(app.getByRole('dialog', { name: 'Command menu' })).toBeVisible();
    await scan(app, 'command menu');
  });
});

test.describe('A11Y new pages and states', () => {
  test('A11Y-12 home, knowledge, join dialog, no-workspace page (dark + light)', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id, { title: 'Home room' });
    await app.goto(`/${workspace.slug}/home`);
    await expect(app.getByRole('region', { name: 'Quick actions' })).toBeVisible();
    await expect(app.getByRole('link', { name: 'Home room', exact: true })).toBeVisible();
    await scan(app, 'home dark');
    await app.goto(`/${workspace.slug}/knowledge`);
    await expect(app.getByRole('heading', { name: 'Search your workspace knowledge' })).toBeVisible();
    await scan(app, 'knowledge empty');
    await app.goto(`/${workspace.slug}/meetings?join=1`);
    await expect(app.getByRole('dialog', { name: 'Join a meeting' })).toBeVisible();
    await scan(app, 'join dialog');
    await app.keyboard.press('Escape');
    await app.getByRole('button', { name: 'Toggle theme' }).click();
    await app.goto(`/${workspace.slug}/home`);
    await expect(app.locator('html')).toHaveAttribute('data-theme', 'light');
    await scan(app, 'home light');
  });

  test('A11Y-13 auth forms with field errors (dark + light)', async ({ page }) => {
    await page.goto('/register');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Enter your email.' })).toBeVisible();
    await scan(page, 'register errors dark');
    await setTheme(page, 'light');
    await page.reload();
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Enter your email.' })).toBeVisible();
    await scan(page, 'register errors light');
  });
});

void seedSession; void login; void createWorkspace;
