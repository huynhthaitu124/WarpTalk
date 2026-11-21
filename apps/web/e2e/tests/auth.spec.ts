import { call, createUser, createWorkspace, expect, loginViaUi, rnd, test as base, seedSession, login, verifyEmail } from '../fixtures';

const test = base;

test.describe('Auth', () => {
  test('AUTH-01 register -> verify -> login lands on the Workspaces page (no workspace yet)', async ({ page }) => {
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

    // Verify like a person: the test backend captures the "emailed" token in Redis (dev mail capture).
    await verifyEmail(email);

    await loginViaUi(page, { email, password: 'Passw0rd!e2e' });
    await expect(page).toHaveURL(/\/workspaces$/);
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
    const ws = await createWorkspace((await login(u)).accessToken);
    await page.goto(`/${ws.slug}/settings/general`);
    await expect(page).toHaveURL(/\/login/);
    await page.getByLabel('Email').fill(u.email);
    await page.getByLabel('Password').fill(u.password);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page).toHaveURL(new RegExp(`/${ws.slug}/settings/general`));
    await expect(page.getByRole('heading', { name: 'Settings', level: 1 })).toBeVisible();
  });

  test('AUTH-05 logout clears the session and revokes the refresh token', async ({ page }) => {
    const u = await createUser('logout');
    await loginViaUi(page, u);
    await expect(page).toHaveURL(/\/workspaces/);
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
    await expect(page).toHaveURL(/\/workspaces/);
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
    await page.goto('/');
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
    await page.getByRole('link', { name: 'Members' }).click();
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
    await expect(page.getByLabel('Password')).toHaveAttribute('aria-invalid', 'true');
    await expect(page.getByLabel('Password')).toBeFocused();

    await page.getByLabel('Email').fill(existing.email);
    await page.getByLabel('Password').fill('Passw0rd!e2e');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.getByRole('alert')).toBeVisible();
    await expect(page).toHaveURL(/\/register/);
  });

  test('AUTH-10 empty submit reports every empty field, not one banner (FN-03)', async ({ page }) => {
    await page.goto('/register');
    await page.getByRole('button', { name: 'Create account' }).click();
    for (const [label, msg] of [['Full name', 'Enter your full name.'], ['Email', 'Enter your email.'], ['Password', 'at least 8 characters']] as const) {
      await expect(page.getByLabel(label)).toHaveAttribute('aria-invalid', 'true');
      await expect(page.getByRole('alert').filter({ hasText: msg })).toBeVisible();
    }
    await expect(page.getByLabel('Full name')).toBeFocused();
    await page.getByLabel('Email').fill('not-an-email');
    await page.getByRole('button', { name: 'Create account' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Enter a valid email address.' })).toBeVisible();

    await page.goto('/login');
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByLabel('Email')).toHaveAttribute('aria-invalid', 'true');
    await expect(page.getByLabel('Password')).toHaveAttribute('aria-invalid', 'true');
    await expect(page.getByRole('alert').filter({ hasText: 'Enter your email.' })).toBeVisible();
    await expect(page.getByRole('alert').filter({ hasText: 'Enter your password.' })).toBeVisible();
    await expect(page.getByLabel('Email')).toBeFocused();
  });

  test('AUTH-11 password can be revealed and hidden', async ({ page }) => {
    await page.goto('/login');
    const pw = page.getByLabel('Password');
    await expect(pw).toHaveAttribute('type', 'password');
    await page.getByRole('button', { name: 'Show characters' }).click();
    await expect(pw).toHaveAttribute('type', 'text');
    await page.getByRole('button', { name: 'Hide characters' }).click();
    await expect(pw).toHaveAttribute('type', 'password');
  });

  test('AUTH-12 JSON error bodies: a malformed login request is INVALID_REQUEST, shown as a message', async ({ page }) => {
    const r = await call('/auth/login', { body: { email: 'x@example.com' } });
    expect(r.status).toBe(400);
    expect((r.body as { code: string }).code).toBe('INVALID_REQUEST');
    await page.goto('/login');
    await page.route('**/api/v1/auth/login', (route) => route.fulfill({ status: 400, json: { code: 'INVALID_REQUEST', message: 'Invalid request body' } }));
    await page.getByLabel('Email').fill('a@example.com');
    await page.getByLabel('Password').fill('whatever1');
    await page.getByRole('button', { name: 'Sign in' }).click();
    await expect(page.getByRole('alert')).toContainText('Invalid request body');
  });
});
