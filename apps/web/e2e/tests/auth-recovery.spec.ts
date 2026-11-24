import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { call, createUser, devMail, expect, login, loginViaUi, ok, test } from '../fixtures';

// Password recovery, change password, resend verification and Google sign-in. Runs against the real e2e backend
// (DEV_MAIL_CAPTURE=1: reset tokens are readable from Redis `dev:mail:reset:{email}`). Google Identity Services is
// replaced by a route-mocked script, so no network call to Google is made.

const NEW_PW = 'N3w-Passw0rd!e2e';

async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${v.id}: ${v.nodes.slice(0, 2).map((n) => n.target.join(' ')).join(' | ')}`), `axe on ${label}`).toEqual([]);
}

/** Stand-in for https://accounts.google.com/gsi/client: renders a plain button that "returns" a credential. */
async function mockGis(page: Page, credential = 'fake-google-id-token') {
  let loads = 0;
  await page.addInitScript(() => { (window as unknown as Record<string, unknown>).__E2E_GOOGLE_CLIENT_ID__ = 'e2e-client.apps.googleusercontent.com'; });
  await page.route('https://accounts.google.com/gsi/client', async (route) => {
    loads += 1;
    await route.fulfill({
      contentType: 'text/javascript',
      body: `window.google = { accounts: { id: {
        initialize(c) { window.__gis = c; },
        renderButton(el) { const b = document.createElement('button'); b.type = 'button'; b.textContent = 'Sign in with Google'; b.onclick = () => window.__gis.callback({ credential: ${JSON.stringify(credential)} }); el.appendChild(b); },
      } } };`,
    });
  });
  return { loads: () => loads };
}

test.describe('Forgot / reset password', () => {
  test('PWR-01 forgot -> reset -> sign in with the new password; the token cannot be reused', async ({ page }) => {
    const u = await createUser('forgot');
    await page.goto('/login');
    await page.getByRole('link', { name: 'Forgot password?' }).click();
    await expect(page).toHaveURL(/\/forgot-password/);
    await page.getByLabel('Email').fill(u.email);
    await page.getByRole('button', { name: 'Send reset link' }).click();
    await expect(page.getByText(/If an account exists for/)).toBeVisible();

    const token = await devMail('reset', u.email);
    await page.goto(`/reset-password?token=${encodeURIComponent(token)}`);
    await page.getByLabel('New password', { exact: true }).fill(NEW_PW);
    await page.getByLabel('Confirm new password').fill(NEW_PW);
    await page.getByRole('button', { name: 'Reset password' }).click();
    await expect(page).toHaveURL(/\/login\?reset=1/);
    await expect(page.getByText('Password updated')).toBeVisible();

    await loginViaUi(page, { email: u.email, password: NEW_PW });
    await expect(page).toHaveURL(/\/workspaces$/);
    expect((await call('/auth/login', { body: { email: u.email, password: u.password } })).status).toBe(401);

    // Same link again: used.
    await page.context().clearCookies();
    await page.evaluate(() => localStorage.clear());
    await page.goto(`/reset-password?token=${encodeURIComponent(token)}`);
    await page.getByLabel('New password', { exact: true }).fill('An0ther-Passw0rd!');
    await page.getByLabel('Confirm new password').fill('An0ther-Passw0rd!');
    await page.getByRole('button', { name: 'Reset password' }).click();
    await expect(page.getByRole('heading', { name: 'Reset link not valid' })).toBeVisible();
    await page.getByRole('link', { name: 'Request a new link' }).click();
    await expect(page).toHaveURL(/\/forgot-password/);
  });

  test('PWR-02 unknown email gets the same neutral message (no account enumeration)', async ({ page }) => {
    await page.goto('/forgot-password');
    await page.getByLabel('Email').fill(`nobody-${Date.now()}@example.com`);
    await page.getByRole('button', { name: 'Send reset link' }).click();
    await expect(page.getByRole('heading', { name: 'Check your inbox' }).or(page.getByText('Check your inbox'))).toBeVisible();
    await expect(page.getByText(/If an account exists for/)).toBeVisible();
  });

  test('PWR-03 form validation: malformed email, short password, mismatch; missing token is the invalid state', async ({ page }) => {
    await page.goto('/forgot-password');
    await page.getByRole('button', { name: 'Send reset link' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Enter your email.' })).toBeVisible();
    await page.getByLabel('Email').fill('nope');
    await page.getByRole('button', { name: 'Send reset link' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Enter a valid email address.' })).toBeVisible();

    await page.goto('/reset-password?token=bogus-token');
    await page.getByLabel('New password', { exact: true }).fill('short');
    await page.getByLabel('Confirm new password').fill('short');
    await page.getByRole('button', { name: 'Reset password' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'at least 8 characters' })).toBeVisible();
    await page.getByLabel('New password', { exact: true }).fill(NEW_PW);
    await page.getByLabel('Confirm new password').fill('different-Pw1');
    await page.getByRole('button', { name: 'Reset password' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'do not match' })).toBeVisible();
    await page.getByLabel('Confirm new password').fill(NEW_PW);
    await page.getByRole('button', { name: 'Reset password' }).click();
    await expect(page.getByRole('heading', { name: 'Reset link not valid' })).toBeVisible(); // bogus token -> INVALID_TOKEN

    await page.goto('/reset-password');
    await expect(page.getByRole('heading', { name: 'Reset link not valid' })).toBeVisible();
  });

  test('PWR-04 axe on /forgot-password and /reset-password (both states), dark + light', async ({ page }) => {
    for (const theme of ['dark', 'light']) {
      await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
      await page.goto('/forgot-password');
      await expect(page.getByRole('heading', { name: 'Reset your password' })).toBeVisible();
      await scan(page, `forgot ${theme}`);
      await page.goto('/reset-password?token=x');
      await expect(page.getByRole('heading', { name: 'Choose a new password' })).toBeVisible();
      await scan(page, `reset ${theme}`);
      await page.goto('/reset-password');
      await expect(page.getByRole('heading', { name: 'Reset link not valid' })).toBeVisible();
      await scan(page, `reset invalid ${theme}`);
    }
  });

  for (const [w, h] of [[1280, 800], [390, 844]] as const) {
    for (const theme of ['dark', 'light'] as const) {
      test(`PWR-05 visual: forgot + reset ${w} ${theme}`, async ({ page }) => {
        await page.setViewportSize({ width: w, height: h });
        await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await page.goto('/forgot-password');
        await expect(page.getByRole('heading', { name: 'Reset your password' })).toBeVisible();
        await expect(page).toHaveScreenshot(`forgot-${w}-${theme}.png`);
        await page.goto('/reset-password?token=x');
        await expect(page.getByRole('heading', { name: 'Choose a new password' })).toBeVisible();
        await expect(page).toHaveScreenshot(`reset-${w}-${theme}.png`);
      });
    }
  }
});

test.describe('Resend verification', () => {
  test('PWR-10 login shows "Resend verification email" for an unverified account and the API call is neutral', async ({ page }) => {
    const u = await createUser('resend', false);
    await loginViaUi(page, u);
    await expect(page.getByRole('alert')).toContainText(/not verified/i);
    const resp = page.waitForResponse((r) => r.url().includes('/auth/resend-verification'));
    await page.getByRole('button', { name: 'Resend verification email' }).click();
    expect((await resp).status()).toBe(202);
    await expect(page.getByText(/we sent a new verification link/)).toBeVisible();
    // The new link works.
    await ok(call('/auth/verify-email', { body: { token: await devMail('verify', u.email) } }));
  });

  test('PWR-11 verify-email failure offers resend (asks for the email when the URL has none)', async ({ page }) => {
    await page.goto('/verify-email?token=bad-token');
    await expect(page.getByRole('alert')).toBeVisible();
    await page.getByRole('button', { name: 'Resend verification email' }).click();
    await expect(page.getByRole('alert').filter({ hasText: 'Enter a valid email address.' })).toBeVisible();
    await page.getByLabel('Email for the new link').fill(`nobody-${Date.now()}@example.com`);
    await page.getByRole('button', { name: 'Resend verification email' }).click();
    await expect(page.getByText(/we sent a new verification link/)).toBeVisible();
  });
});

test.describe('Change password (settings -> account)', () => {
  test('PWR-20 wrong current password, mismatch, then success (old password stops working)', async ({ app, user, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/account`);
    await expect(app.getByRole('heading', { name: 'Settings', level: 1 })).toBeVisible();
    const cur = app.getByLabel('Current password');
    const nw = app.getByLabel('New password', { exact: true });
    const cf = app.getByLabel('Confirm new password');
    const submit = app.getByRole('button', { name: 'Change password' });

    await cur.fill('not-my-password');
    await nw.fill(NEW_PW);
    await cf.fill(NEW_PW);
    await submit.click();
    await expect(app.getByRole('alert').filter({ hasText: 'current password is incorrect' })).toBeVisible();
    await expect(cur).toHaveAttribute('aria-invalid', 'true');

    await cur.fill(user.password);
    await cf.fill('mismatch-Pw1');
    await submit.click();
    await expect(app.getByRole('alert').filter({ hasText: 'do not match' })).toBeVisible();

    await cf.fill(NEW_PW);
    await submit.click();
    await expect(app.getByText('Password changed')).toBeVisible();
    await expect(cur).toHaveValue('');
    expect((await call('/auth/login', { body: { email: user.email, password: user.password } })).status).toBe(401);
    expect((await call('/auth/login', { body: { email: user.email, password: NEW_PW } })).status).toBe(200);
  });

  test('PWR-21 account without a password: set-password hint, no form; Google linked -> unlink disabled with the reason', async ({ app, workspace }) => {
    await app.route('**/api/v1/me', async (route) => {
      if (route.request().method() !== 'GET') return route.continue();
      const r = await route.fetch();
      await route.fulfill({ response: r, json: { ...(await r.json()), hasPassword: false, googleLinked: true } });
    });
    await app.goto(`/${workspace.slug}/settings/account`);
    await expect(app.getByText('Set a password', { exact: true })).toBeVisible();
    await expect(app.getByRole('link', { name: 'Forgot password' })).toBeVisible();
    await expect(app.getByLabel('Current password')).toHaveCount(0);
    await expect(app.getByRole('button', { name: 'Unlink Google' })).toBeDisabled();
    await expect(app.getByText(/cannot unlink Google while this account has no password/)).toBeVisible();
  });

  test('PWR-22 link and unlink Google from settings', async ({ app, workspace }) => {
    await mockGis(app);
    let linked = false;
    const bodies: unknown[] = [];
    await app.route('**/api/v1/me', async (route) => {
      if (route.request().method() !== 'GET') return route.continue();
      const r = await route.fetch();
      await route.fulfill({ response: r, json: { ...(await r.json()), hasPassword: true, googleLinked: linked } });
    });
    await app.route('**/api/v1/me/google/link', async (route) => {
      const m = route.request().method();
      if (m === 'POST') { bodies.push(route.request().postDataJSON()); linked = true; }
      else linked = false;
      await route.fulfill({ status: 204 });
    });
    await app.goto(`/${workspace.slug}/settings/account`);
    await app.getByRole('button', { name: 'Sign in with Google' }).click(); // the stubbed GIS button
    await expect(app.getByText('Google is linked')).toBeVisible();
    expect(bodies).toEqual([{ idToken: 'fake-google-id-token' }]);
    await app.getByRole('button', { name: 'Unlink Google' }).click();
    await expect(app.getByText('Link Google', { exact: true })).toBeVisible();
  });
});

test.describe('Google sign-in', () => {
  test('PWR-30 button loads GIS lazily on /login and /register; credential -> POST /auth/google -> signed in', async ({ page }) => {
    const gis = await mockGis(page);
    const u = await createUser('google');
    const tokens = await login(u);
    let posted: unknown;
    await page.route('**/api/v1/auth/google', async (route) => {
      posted = route.request().postDataJSON();
      await route.fulfill({ json: { userId: u.userId, email: u.email, accessToken: tokens.accessToken, accessTokenExpiresAt: new Date(Date.now() + 900_000).toISOString(), refreshToken: tokens.refreshToken } });
    });
    await page.goto('/design');
    expect(gis.loads()).toBe(0); // not loaded on unrelated pages
    await page.goto('/register');
    await expect(page.getByRole('button', { name: 'Sign in with Google' })).toBeVisible();
    await page.goto('/login');
    await expect(page.getByRole('button', { name: 'Sign in with Google' })).toBeVisible();
    await page.getByRole('button', { name: 'Sign in with Google' }).click();
    await expect(page).toHaveURL(/\/workspaces$/);
    expect(posted).toEqual({ idToken: 'fake-google-id-token' });
    await expect(page.getByTestId('account-name')).toBeVisible();
  });

  const errors: [string, number, RegExp][] = [
    ['INVALID_GOOGLE_TOKEN', 401, /could not verify your sign-in/],
    ['GOOGLE_UNAVAILABLE', 503, /temporarily unavailable/],
    ['GOOGLE_LINK_REQUIRES_VERIFIED_EMAIL', 409, /Verify its email address first/],
    ['GOOGLE_ACCOUNT_MISMATCH', 409, /different Google account/],
    ['ACCOUNT_LOCKED', 403, /temporarily locked/],
  ];
  for (const [code, status, re] of errors) {
    test(`PWR-31 ${code} is explained`, async ({ page }) => {
      await mockGis(page);
      await page.route('**/api/v1/auth/google', (route) => route.fulfill({ status, json: { code, message: 'raw server text' } }));
      await page.goto('/login');
      await page.getByRole('button', { name: 'Sign in with Google' }).click();
      await expect(page.getByRole('alert')).toContainText(re);
      await expect(page).toHaveURL(/\/login/);
    });
  }

  test('PWR-35 no client id configured: no Google button and GIS is never requested', async ({ page }) => {
    const requested: string[] = [];
    page.on('request', (r) => { if (r.url().includes('accounts.google.com')) requested.push(r.url()); });
    for (const path of ['/login', '/register']) {
      await page.goto(path);
      await expect(page.getByRole('heading', { level: 1 })).toBeVisible();
      await expect(page.getByTestId('google-signin')).toHaveCount(0);
    }
    expect(requested).toEqual([]);
  });

  test('PWR-32 macOS host without a google sign-in bridge method: button hidden', async ({ page }) => {
    await mockGis(page);
    await page.addInitScript(() => { (window as unknown as Record<string, unknown>).warptalkNative = { version: 1, getCapabilities: () => Promise.resolve({}) }; });
    await page.goto('/login');
    await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
    await expect(page.getByTestId('google-signin')).toHaveCount(0);
  });

  test('PWR-33 axe on login with the Google button', async ({ page }) => {
    await mockGis(page);
    await page.goto('/login');
    await expect(page.getByRole('button', { name: 'Sign in with Google' })).toBeVisible();
    await scan(page, 'login + google');
  });
});
