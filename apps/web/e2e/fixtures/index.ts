import { test as base, expect, type BrowserContext, type Page } from '@playwright/test';
import { createUser, createWorkspace, login, type Tokens, type TestUser, type Ws } from './api';
import { FLAGS_ALL, mockFeatures, type FeatureFlags } from './features';

export { expect };
export * from './api';
export * from './db';
export * from './native-mock';
export * from './files';
export * from './features';
export * from './copilot';

/**
 * Seeds the refresh token into localStorage once per tab (guarded by sessionStorage so a rotated
 * token is never overwritten). The app boots through its real silent-refresh path.
 */
export async function seedSession(target: Page | BrowserContext, tokens: Tokens) {
  await target.addInitScript((rt) => {
    try {
      if (!sessionStorage.getItem('e2e-seeded')) {
        localStorage.setItem('wt.refresh', rt);
        sessionStorage.setItem('e2e-seeded', '1');
      }
    } catch { /* ignore */ }
  }, tokens.refreshToken);
}

/** A second, independent signed-in browser context (e.g. the guest in lobby tests). */
export async function signedInContext(browser: import('@playwright/test').Browser, user: TestUser, baseURL: string | undefined) {
  const tokens = await login(user);
  const context = await browser.newContext({ baseURL, reducedMotion: 'reduce', timezoneId: 'UTC', locale: 'en-US' });
  await mockFeatures(context, FLAGS_ALL);
  await seedSession(context, tokens);
  const page = await context.newPage();
  return { context, page, tokens };
}

interface Fixtures {
  user: TestUser;
  tokens: Tokens;
  workspace: Ws;
  /** Signed in as `user`, workspace pre-created. */
  app: Page;
  /**
   * Feature flags the page sees (`GET /features` is mocked). Default: everything ON, so specs written for the translation
   * product keep exercising it; use `test.use({ flags: 'real' })` to hit the backend, or `{ ...FLAGS_DEFAULT }` for platform defaults.
   */
  flags: Partial<FeatureFlags> | 'real';
}

export const test = base.extend<Fixtures>({
  flags: [FLAGS_ALL, { option: true }],
  page: async ({ page, flags }, use) => {
    if (flags !== 'real') await mockFeatures(page, flags);
    await use(page);
  },
  user: async ({}, use) => { await use(await createUser('owner')); },
  tokens: async ({ user }, use) => { await use(await login(user)); },
  workspace: async ({ tokens }, use) => { await use(await createWorkspace(tokens.accessToken)); },
  app: async ({ page, tokens, workspace: _workspace }, use) => {
    await seedSession(page, tokens);
    await use(page);
  },
});

export async function loginViaUi(page: Page, u: { email: string; password: string }) {
  await page.goto('/login');
  await page.getByLabel('Email').fill(u.email);
  await page.getByLabel('Password').fill(u.password);
  await page.getByRole('button', { name: 'Sign in' }).click();
}
export * from './worker';
export * from './assistant';
