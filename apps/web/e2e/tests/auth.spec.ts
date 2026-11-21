import { call, createUser, createWorkspace, expect, loginViaUi, rnd, test as base, seedSession, login } from '../fixtures';

const test = base;

test.describe('Auth', () => {
  test('AUTH-01 register -> verify instructions -> login lands on rooms', async ({ page }) => {
    const id = rnd();
    const email = `e2e-reg-${id}@example.com`;
    await page.goto('/register');
    await page.getByLabel('Full name').fill(`Reg User ${id}`);
    await page.getByLabel('Email').fill(email);
    await page.getByLabel('Password').fill('Passw0rd!e2e');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page).toHaveURL(/\/verify-email/);
    await expect(page.getByText('Check your inbox')).toBeVisible();
    await expect(page.getByText(email)).toBeVisible();

    // Test-only bypass: the token is never exposed, mark verified in Postgres.
    const { markEmailVerified } = await import('../fixtures/db');
    markEmailVerified(email);

    await loginViaUi(page, { email, password: 'Passw0rd!e2e' });
    await expect(page).toHaveURL(/\/rooms$/);
    await expect(page.getByTestId('account-name')).toHaveText(`Reg User ${id}`);
  });

  test('AUTH-02 wrong password shows an error', async ({ page }) => {
    const u = await createUser('wrongpw');
    await loginViaUi(page, { email: u.email, password: 'definitely-wrong' });
    await expect(page.getByRole('alert')).toContainText(/invalid email or password/i);
    await expect(page).toHaveURL(/\/login/);
  });

  test('AUTH-03 unverified user is told to verify', async ({ page }) => {
    const u = await createUser('unverified', false);
    await loginViaUi(page, u);
    await expect(page.getByRole('alert')).toContainText(/not verified/i);
    await page.getByRole('link', { name: 'Verify your email' }).click();
    await expect(page).toHaveURL(/\/verify-email\?email=/);
  });

  test('AUTH-04 protected route redirects to login and back', async ({ page }) => {
    const u = await createUser('guard');
    await page.goto('/settings');
    await expect(page).toHaveURL(/\/login/);
    await page.getByLabel('Email').fill(u.email);
    await page.getByLabel('Password').fill(u.password);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page).toHaveURL(/\/settings/);
    await expect(page.getByRole('heading', { name: 'Settings', level: 1 })).toBeVisible();
  });

  test('AUTH-05 logout clears the session and revokes the refresh token', async ({ page }) => {
    const u = await createUser('logout');
    await loginViaUi(page, u);
    await expect(page).toHaveURL(/\/rooms/);
    const rt = await page.evaluate(() => localStorage.getItem('wt.refresh'));
    expect(rt).toBeTruthy();
    await page.getByTestId('account-menu').click();
    const logoutDone = page.waitForResponse((r) => r.url().includes('/auth/logout'));
    await page.getByRole('menuitem', { name: 'Sign out' }).click();
    expect((await logoutDone).status()).toBe(204);
    await expect(page).toHaveURL(/\/login/);
    expect(await page.evaluate(() => localStorage.getItem('wt.refresh'))).toBeNull();
    await page.reload();
    await expect(page).toHaveURL(/\/login/);
    const r = await call('/auth/refresh', { body: { refreshToken: rt } });
    expect(r.status).toBe(401);
  });

  test('AUTH-06 silent refresh on reload rotates the refresh token', async ({ page }) => {
    const u = await createUser('silent');
    await loginViaUi(page, u);
    await expect(page).toHaveURL(/\/rooms/);
    const t0 = await page.evaluate(() => localStorage.getItem('wt.refresh'));
    await page.reload();
    await expect(page.getByTestId('account-name')).toBeVisible();
    const t1 = await page.evaluate(() => localStorage.getItem('wt.refresh'));
    await page.reload();
    await expect(page.getByTestId('account-name')).toBeVisible();
    const t2 = await page.evaluate(() => localStorage.getItem('wt.refresh'));
    expect(new Set([t0, t1, t2]).size).toBe(3);
    const stale = await call('/auth/refresh', { body: { refreshToken: t0 } });
    expect(stale.status).toBe(401);
  });

  test('AUTH-07 concurrent 401s trigger a single refresh', async ({ page }) => {
    const u = await createUser('single');
    const tokens = await login(u);
    await createWorkspace(tokens.accessToken);
    await seedSession(page, tokens);
    await page.goto('/rooms');
    await expect(page.getByTestId('account-name')).toBeVisible();
    let refreshes = 0;
    await page.route('**/api/v1/auth/refresh', async (route) => { refreshes += 1; await route.continue(); });
    // "Expire" the access token: every authed call is refused until the first refresh has happened.
    await page.route(/\/api\/v1\/(workspaces|me)/, async (route) => {
      if (refreshes === 0) {
        await route.fulfill({ status: 401, contentType: 'application/json', body: JSON.stringify({ code: 'UNAUTHORIZED', message: 'expired' }) });
      } else {
        await route.continue();
      }
    });
    // Client-side navigation fires several authed queries at once (settings + members).
    await page.getByRole('link', { name: 'Settings' }).click();
    await page.getByRole('tab', { name: 'Members' }).click();
    await expect(page.getByRole('table', { name: 'Workspace members' })).toBeVisible();
    expect(refreshes).toBe(1);
  });

  test('AUTH-08 bogus verification token shows an error', async ({ page }) => {
    await page.goto('/verify-email');
    await page.getByLabel('Verification token').fill('not-a-real-token');
    await page.getByRole('button', { name: 'Verify email' }).click();
    await expect(page.getByRole('alert')).toBeVisible();
  });

  test('AUTH-09 register validation', async ({ page }) => {
    const existing = await createUser('dup');
    await page.goto('/register');
    await page.getByLabel('Full name').fill('Short Pw');
    await page.getByLabel('Email').fill(`e2e-short-${rnd()}@example.com`);
    await page.getByLabel('Password').fill('short');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.getByRole('alert')).toContainText('at least 8 characters');

    await page.getByLabel('Email').fill(existing.email);
    await page.getByLabel('Password').fill('Passw0rd!e2e');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.getByRole('alert')).toBeVisible();
    await expect(page).toHaveURL(/\/register/);
  });
});
