import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { createUser, createWorkspace, expect, inviteAndAccept, login, seedSession, test } from '../fixtures';

// Workspace billing UI. Plans/overview/checkout run against the real backend where possible (Stripe is not configured in
// e2e, so checkout answers 503 STRIPE_NOT_CONFIGURED); states the backend cannot be put into cheaply (suspended, paging,
// redirect URL, deterministic pixels) are route-mocked.

async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${v.id}: ${v.nodes.slice(0, 2).map((n) => n.target.join(' ')).join(' | ')}`), `axe on ${label}`).toEqual([]);
}

const PLANS = [
  { id: 'p-free', name: 'Free', slug: 'free', tier: 0, price: 0, currency: 'usd', billingCycle: 'monthly', creditsPerCycle: 100, maxParticipants: 5, maxLanguages: 2, maxActiveRooms: 1, features: {}, purchasable: false },
  { id: 'p-pro', name: 'Pro', slug: 'pro', tier: 1, price: 29, currency: 'usd', billingCycle: 'monthly', creditsPerCycle: 5000, maxParticipants: 50, maxLanguages: 10, maxActiveRooms: 5, features: {}, purchasable: true },
  { id: 'p-team', name: 'Team', slug: 'team', tier: 2, price: 99, currency: 'usd', billingCycle: 'monthly', creditsPerCycle: 25000, maxParticipants: 200, maxLanguages: 30, maxActiveRooms: 20, features: {}, purchasable: true },
];

const overview = (wsId: string, serviceState = 'healthy', suspendedReason: string | null = null) => ({
  workspaceId: wsId,
  subscription: { id: 's1', plan: PLANS[1], status: 'active', currentPeriodStart: '2026-11-01T00:00:00Z', currentPeriodEnd: '2026-12-01T00:00:00Z', autoRenew: true, trialEndsAt: null, cancelledAt: null },
  balance: 3200.5, creditsUsedThisCycle: 1800, serviceState, suspendedReason,
  usage: { from: '2026-11-01T00:00:00Z', to: '2026-11-24T00:00:00Z', creditsConsumed: 1800, audioSeconds: 7200, tokens: 120000, events: 342 },
  entitlements: {},
});

const TX = Array.from({ length: 25 }, (_, i) => ({
  id: `tx-${i + 1}`, amount: i % 3 === 0 ? 5000 : -(10 + i), type: i % 3 === 0 ? 'grant' : 'usage', chargeType: i % 3 === 0 ? 'plan' : 'stt',
  description: i % 3 === 0 ? `Plan credits ${i + 1}` : `Live transcription ${i + 1}`, balanceAfter: 1000 + i, referenceId: null, referenceType: null,
  createdAt: `2026-11-${String(24 - Math.floor(i / 2)).padStart(2, '0')}T10:${String(i).padStart(2, '0')}:00Z`,
}));

/** Deterministic billing data for one workspace. */
async function mockBilling(page: Page, wsId: string, state: { serviceState?: string; reason?: string | null } = {}) {
  await page.route(new RegExp(`/api/v1/workspaces/${wsId}/billing$`), (r) => r.fulfill({ json: overview(wsId, state.serviceState ?? 'healthy', state.reason ?? null) }));
  await page.route(/\/api\/v1\/billing\/plans$/, (r) => r.fulfill({ json: PLANS }));
  await page.route(/\/billing\/usage\?/, (r) => {
    const by = new URL(r.request().url()).searchParams.get('groupBy');
    const groups = by === 'feature'
      ? [{ key: 'stt', credits: 1200, quantity: 7200, events: 200 }, { key: 'suggestions', credits: 600, quantity: 90, events: 142 }]
      : by === 'member' ? [{ key: 'Ada Lovelace', credits: 1800, quantity: 1, events: 342 }]
      : [{ key: '2026-11-22', credits: 300, quantity: 1, events: 50 }, { key: '2026-11-23', credits: 900, quantity: 1, events: 150 }, { key: '2026-11-24', credits: 600, quantity: 1, events: 142 }];
    return r.fulfill({ json: { from: '2026-10-25T00:00:00Z', to: '2026-11-24T00:00:00Z', groupBy: by, groups } });
  });
  await page.route(/\/billing\/transactions\?/, (r) => {
    const u = new URL(r.request().url());
    const pg = Number(u.searchParams.get('page') ?? 1);
    const size = Number(u.searchParams.get('pageSize') ?? 10);
    return r.fulfill({ json: { items: TX.slice((pg - 1) * size, pg * size), page: pg, pageSize: size, total: TX.length } });
  });
}

test.describe('Billing page', () => {
  test('BILL-01 real backend: owner sees the plan, balance and the plans grid; checkout without Stripe explains itself', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/billing`);
    await expect(app.getByRole('heading', { name: 'Billing', level: 1 })).toBeVisible();
    await expect(app.getByRole('region', { name: 'Balance' })).toBeVisible();
    const plans = app.getByRole('region', { name: 'Plans' });
    await expect(plans.getByRole('heading', { level: 3 }).first()).toBeVisible();
    const upgrade = plans.getByRole('button', { name: /^Upgrade to / }).first();
    if (await upgrade.count()) {
      await upgrade.click();
      await expect(app.getByRole('alert').filter({ hasText: /payments are not set up/i })).toBeVisible();
    }
    await expect(app.getByRole('button', { name: 'Buy credits' })).toBeVisible();
    await app.getByRole('button', { name: 'Buy credits' }).click();
    await expect(app.getByRole('alert').filter({ hasText: /payments are not set up/i })).toBeVisible();
  });

  test('BILL-02 real backend: a member sees the plans and an explanation, no upgrade, top-up or portal buttons', async ({ page, tokens, workspace }) => {
    const member = await createUser('billmember');
    await inviteAndAccept(tokens.accessToken, workspace.id, member);
    await seedSession(page, await login(member));
    await page.goto(`/${workspace.slug}/billing`);
    await expect(page.getByRole('heading', { name: 'Billing', level: 1 })).toBeVisible();
    await expect(page.getByText('Only the workspace owner can change the plan')).toBeVisible();
    await expect(page.getByRole('button', { name: /^Upgrade/ })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Buy credits' })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Manage in billing portal' })).toHaveCount(0);
  });

  test('BILL-03 owner: Upgrade posts the plan id and navigates to the returned checkout URL; top-up posts the credits', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id);
    const bodies: unknown[] = [];
    await app.route(new RegExp(`/api/v1/workspaces/${workspace.id}/billing/checkout$`), (r) => { bodies.push(r.request().postDataJSON()); return r.fulfill({ json: { url: `${new URL(app.url()).origin}/__stripe-dummy?s=${bodies.length}` } }); });
    await app.route('**/__stripe-dummy**', (r) => r.fulfill({ contentType: 'text/html', body: '<title>Stripe dummy</title><h1>Checkout</h1>' }));
    await app.goto(`/${workspace.slug}/billing`);
    await expect(app.getByTestId('plan-pro')).toBeVisible();
    await expect(app.getByTestId('plan-free').getByRole('button')).toBeDisabled(); // free: included, nothing to buy
    await expect(app.getByTestId('plan-pro').getByRole('button')).toBeDisabled(); // the current plan
    await Promise.all([
      app.waitForRequest((r) => r.url().includes('/__stripe-dummy?s=1')),
      app.getByRole('button', { name: 'Upgrade to Team' }).click(),
    ]);
    await expect(app.getByRole('heading', { name: 'Checkout' })).toBeVisible();
    expect(bodies[0]).toEqual({ planId: 'p-team' });

    await app.goBack();
    await app.getByRole('spinbutton', { name: 'Credits' }).fill('2500');
    await Promise.all([
      app.waitForRequest((r) => r.url().includes('/__stripe-dummy?s=2')),
      app.getByRole('button', { name: 'Buy credits' }).click(),
    ]);
    expect(bodies[1]).toEqual({ topUpCredits: 2500 });
  });

  test('BILL-04 checkout answers 403 / 503: the message is explained; invalid top-up is caught client-side', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id);
    let mode = 503;
    await app.route(new RegExp(`/billing/checkout$`), (r) => r.fulfill(mode === 503
      ? { status: 503, json: { code: 'STRIPE_NOT_CONFIGURED', message: 'raw' } } : { status: 403, json: { code: 'FORBIDDEN', message: 'Only the owner may do that' } }));
    await app.goto(`/${workspace.slug}/billing`);
    await app.getByRole('button', { name: 'Upgrade to Team' }).click();
    await expect(app.getByRole('alert').filter({ hasText: 'payments are not set up' })).toBeVisible();
    mode = 403;
    await app.getByRole('button', { name: 'Upgrade to Team' }).click();
    await expect(app.getByRole('alert').filter({ hasText: 'Only the workspace owner' })).toBeVisible();
    await app.getByRole('spinbutton', { name: 'Credits' }).fill('0');
    await app.getByRole('button', { name: 'Buy credits' }).click();
    await expect(app.getByRole('alert').filter({ hasText: 'whole number of credits' })).toBeVisible();
  });

  test('BILL-05 portal: opens the returned URL', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id);
    await app.route(new RegExp('/billing/portal$'), (r) => r.fulfill({ json: { url: `${new URL(app.url()).origin}/__portal-dummy` } }));
    await app.route('**/__portal-dummy', (r) => r.fulfill({ contentType: 'text/html', body: '<h1>Portal</h1>' }));
    await app.goto(`/${workspace.slug}/billing`);
    await Promise.all([app.waitForRequest((r) => r.url().endsWith('/__portal-dummy')), app.getByRole('button', { name: 'Manage in billing portal' }).click()]);
  });

  test('BILL-06 return from checkout: ?checkout=success / cancel show a notice', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id);
    await app.goto(`/${workspace.slug}/billing?checkout=success`);
    await expect(app.getByText('Payment received')).toBeVisible();
    await app.goto(`/${workspace.slug}/billing?checkout=cancel`);
    await expect(app.getByText('Checkout cancelled')).toBeVisible();
  });

  test('BILL-07 usage table groups by day / feature / member', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id);
    await app.goto(`/${workspace.slug}/billing`);
    const usage = app.getByRole('region', { name: 'Usage' });
    await expect(usage.getByRole('table', { name: /by day/i })).toContainText('Nov 23, 2026');
    await usage.getByRole('button', { name: 'By feature' }).click();
    await expect(usage.getByRole('button', { name: 'By feature' })).toHaveAttribute('aria-pressed', 'true');
    await expect(usage.getByRole('table', { name: /by feature/i })).toContainText('stt');
    await usage.getByRole('button', { name: 'By member' }).click();
    await expect(usage.getByRole('table', { name: /by member/i })).toContainText('Ada Lovelace');
  });

  test('BILL-08 transactions page through the list', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id);
    await app.goto(`/${workspace.slug}/billing`);
    const tx = app.getByRole('region', { name: 'Transactions' });
    await expect(tx.getByText('Page 1 of 3 · 25 total')).toBeVisible();
    await expect(tx.getByRole('row')).toHaveCount(11);
    await expect(tx.getByRole('button', { name: 'Previous' })).toBeDisabled();
    await tx.getByRole('button', { name: 'Next' }).click();
    await expect(tx.getByText('Page 2 of 3 · 25 total')).toBeVisible();
    await tx.getByRole('button', { name: 'Next' }).click();
    await expect(tx.getByText('Page 3 of 3 · 25 total')).toBeVisible();
    await expect(tx.getByRole('row')).toHaveCount(6);
    await expect(tx.getByRole('button', { name: 'Next' })).toBeDisabled();
  });

  test('BILL-09 service banner: suspended / low balance / overage show on every workspace page with a link to billing; healthy shows none', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id, { serviceState: 'suspended', reason: 'insufficient_credits' });
    await app.goto(`/${workspace.slug}/members`);
    const banner = app.getByTestId('billing-banner');
    await expect(banner).toContainText('Service is suspended');
    await banner.getByRole('link', { name: 'Open billing' }).click();
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/billing$`));
    await expect(app.getByTestId('service-state')).toHaveText('Suspended');
    for (const [state, text] of [['low_balance', 'running low'], ['in_overage', 'overage credits']] as const) {
      await app.unroute(new RegExp(`/api/v1/workspaces/${workspace.id}/billing$`));
      await app.route(new RegExp(`/api/v1/workspaces/${workspace.id}/billing$`), (r) => r.fulfill({ json: overview(workspace.id, state) }));
      await app.goto(`/${workspace.slug}/members`);
      await expect(app.getByTestId('billing-banner')).toContainText(text);
    }
    await app.unroute(new RegExp(`/api/v1/workspaces/${workspace.id}/billing$`));
    await app.route(new RegExp(`/api/v1/workspaces/${workspace.id}/billing$`), (r) => r.fulfill({ json: overview(workspace.id) }));
    await app.goto(`/${workspace.slug}/members`);
    await expect(app.getByRole('table', { name: 'Workspace members' })).toBeVisible();
    await expect(app.getByTestId('billing-banner')).toHaveCount(0);
  });

  test('BILL-10 sidebar has a Billing link', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id);
    await app.goto(`/${workspace.slug}/members`);
    await app.getByRole('link', { name: 'Billing' }).first().click();
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/billing$`));
  });

  test('BILL-11 session start with no credits (402 INSUFFICIENT_CREDITS) explains and links to billing', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id);
    await app.route('**/api/v1/copilot/sessions', (r) => r.request().method() === 'POST'
      ? r.fulfill({ status: 402, json: { code: 'INSUFFICIENT_CREDITS', message: 'Insufficient credits' } }) : r.continue());
    await app.goto(`/${workspace.slug}/sessions?new=1`);
    await app.getByLabel('Title').fill('No credits session');
    await app.getByRole('button', { name: /Create session|Start session/ }).click();
    await expect(app.getByRole('alert').filter({ hasText: 'Not enough credits' })).toBeVisible();
    await expect(app.getByRole('alert').getByRole('link', { name: 'Open billing' })).toHaveAttribute('href', `/${workspace.slug}/billing`);
  });

  test('BILL-20 axe: billing page (owner, with suspended banner), dark + light', async ({ app, workspace }) => {
    await mockBilling(app, workspace.id, { serviceState: 'suspended', reason: 'insufficient_credits' });
    for (const theme of ['dark', 'light']) {
      await app.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
      await app.goto(`/${workspace.slug}/billing?checkout=success`);
      await expect(app.getByRole('region', { name: 'Transactions' }).getByRole('row')).toHaveCount(11);
      await scan(app, `billing ${theme}`);
    }
  });

  for (const s of [{ w: 1280, h: 800 }, { w: 390, h: 844 }]) {
    for (const theme of ['dark', 'light'] as const) {
      test(`BILL-21 visual: billing ${s.w} ${theme}`, async ({ page, tokens }) => {
        const ws = await createWorkspace(tokens.accessToken, 'Acme Billing');
        await page.setViewportSize({ width: s.w, height: s.h });
        await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await seedSession(page, tokens);
        await mockBilling(page, ws.id);
        await page.goto(`/${ws.slug}/billing`);
        await expect(page.getByRole('region', { name: 'Transactions' }).getByRole('row')).toHaveCount(11);
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(150);
        await expect(page).toHaveScreenshot(`billing-${s.w}-${theme}.png`, { mask: [page.getByTestId('account-menu')] });
      });
    }
  }
});
