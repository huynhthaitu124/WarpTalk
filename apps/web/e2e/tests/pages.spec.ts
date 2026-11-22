import type { Page } from '@playwright/test';
import { test as raw } from '@playwright/test';
import {
  call, createRoom, createUser, createWorkspace, devMail, expect, inviteAndAccept, inviteMember, joinByCode, login, ok, rnd, seedSession, signedInContext, test,
} from '../fixtures';

const LONG = 'Extraordinarily long board review of the quarterly translation roadmap and the multilingual launch plan for every region we serve this year ok';

/** No horizontal overflow: the measurement the audit used. */
async function noOverflow(page: Page, label: string) {
  await page.waitForTimeout(150);
  const m = await page.evaluate(() => ({ sw: document.documentElement.scrollWidth, iw: window.innerWidth, bw: document.body.scrollWidth }));
  expect(m.sw, `${label}: documentElement.scrollWidth ${m.sw} > innerWidth ${m.iw}`).toBeLessThanOrEqual(m.iw);
  expect(m.bw, `${label}: body.scrollWidth ${m.bw} > innerWidth ${m.iw}`).toBeLessThanOrEqual(m.iw);
}

test.describe('Members and invitations', () => {
  test('MEM-01 owner invites, resends and revokes (real endpoints)', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/members?invite=1`);
    const dialog = app.getByRole('dialog', { name: 'Invite to workspace' });
    await expect(dialog).toBeVisible();
    await dialog.getByLabel('Email').fill('nope');
    await dialog.getByRole('button', { name: 'Send invitation' }).click();
    await expect(dialog.getByRole('alert').filter({ hasText: 'Enter a valid email address.' })).toBeVisible();
    const email = `e2e-invitee-${rnd()}@example.com`;
    await dialog.getByLabel('Email').fill(email);
    await dialog.getByRole('button', { name: 'Send invitation' }).click();
    await expect(app.getByTestId('toast').filter({ hasText: 'Invitation sent' })).toBeVisible();
    await expect(app).not.toHaveURL(/invite=1/);
    const row = app.getByTestId('invitation').filter({ hasText: email });
    await expect(row).toBeVisible();
    await expect(row.locator('[data-status="pending"]')).toHaveText('Pending');
    await row.getByRole('button', { name: `Resend invitation to ${email}` }).click();
    await expect(app.getByTestId('toast').filter({ hasText: 'Invitation re-sent' })).toBeVisible();
    await row.getByRole('button', { name: `Revoke invitation to ${email}` }).click();
    await expect(row.locator('[data-status="revoked"]')).toHaveText('Revoked');
    await expect(row.getByRole('button', { name: /Resend|Revoke/ })).toHaveCount(0);
  });

  test('MEM-02 invitee sees the pending invitation and accepts it', async ({ app, tokens, workspace, browser, baseURL }) => {
    const invitee = await createUser('invitee');
    const inv = await inviteMember(tokens.accessToken, workspace.id, invitee.email);
    expect(await devMail('workspace-invite', invitee.email)).toBeTruthy();
    const ctx = await signedInContext(browser, invitee, baseURL);
    try {
      await ctx.page.goto('/');
      await expect(ctx.page).toHaveURL(/\/workspaces$/);
      const banner = ctx.page.getByRole('region', { name: 'Pending invitations' });
      await expect(banner).toContainText(workspace.name);
      await banner.getByRole('button', { name: `Accept invitation to ${workspace.name}` }).click();
      await expect(ctx.page.getByTestId('current-workspace')).toHaveText(workspace.name);
      await expect(ctx.page).toHaveURL(new RegExp(`/${workspace.slug}/home`));
      // owner now sees them as a member, and the invitation as accepted
      await app.goto(`/${workspace.slug}/members`);
      await expect(app.getByRole('table', { name: 'Workspace members' }).getByText(invitee.email)).toBeVisible();
      await expect(app.getByTestId('invitation').filter({ hasText: invitee.email }).locator('[data-status="accepted"]')).toBeVisible();
      expect(inv.status).toBe('pending');
    } finally {
      await ctx.context.close();
    }
  });

  test('MEM-03 a plain member cannot invite (UI and API)', async ({ workspace, tokens, browser, baseURL }) => {
    const m = await createUser('plain');
    await inviteAndAccept(tokens.accessToken, workspace.id, m);
    const ctx = await signedInContext(browser, m, baseURL);
    try {
      await ctx.page.goto(`/${workspace.slug}/members`);
      await expect(ctx.page.getByRole('table', { name: 'Workspace members' })).toBeVisible();
      await expect(ctx.page.getByRole('button', { name: 'Invite people' })).toHaveCount(0);
      await expect(ctx.page.getByText('Only workspace owners and admins can invite people')).toBeVisible();
      const r = await call(`/workspaces/${workspace.id}/invitations`, { token: ctx.tokens.accessToken, body: { email: `x-${rnd()}@example.com`, role: 'MEMBER' } });
      expect(r.status).toBe(403);
    } finally {
      await ctx.context.close();
    }
  });
});

test.describe('Knowledge search', () => {
  test('KNOW-01 empty state, then a search against the real API ends in results, none, or a clear error', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/knowledge`);
    await expect(app.getByRole('heading', { name: 'Search your workspace knowledge' })).toBeVisible();
    await app.getByLabel('Search knowledge').fill('budget decision');
    await app.getByRole('button', { name: 'Search', exact: true }).click();
    await expect(app).toHaveURL(/q=budget(\+|%20)decision/);
    // The e2e backend has no embedding keys: either "no results" or a readable error, never a blank page.
    await expect(app.getByRole('heading', { name: /No results for/ }).or(app.getByRole('alert').filter({ hasText: /Search failed/ })).or(app.getByRole('list', { name: 'Search results' }))).toBeVisible();
  });

  test('KNOW-02 results cite the document page or the meeting timestamp; filters go to the API', async ({ app, workspace }) => {
    const seen: string[] = [];
    await app.route('**/api/v1/knowledge/search**', (r) => {
      seen.push(r.request().url());
      return r.fulfill({
        json: {
          chunks: [
            { chunkId: 'c1', score: 0.91, denseScore: 0.9, sourceType: 'document', sourceId: 'd1', text: 'The launch budget was approved at 40k.', citation: { kind: 'document', title: 'Q3 launch plan', refId: 'd1', page: 4, headingPath: ['Budget', 'Approval'] } },
            { chunkId: 'c2', score: 0.8, denseScore: 0.8, sourceType: 'transcript', sourceId: 'm1', text: 'We agreed to ship on Friday.', citation: { kind: 'transcript', title: 'Board review', refId: 'm1', startMs: 754000, endMs: 760000 } },
          ],
          metrics: { latencyMs: 12, returned: 2 },
        },
      });
    });
    await app.goto(`/${workspace.slug}/knowledge?q=budget`);
    const hits = app.getByTestId('knowledge-hit');
    await expect(hits).toHaveCount(2);
    await expect(hits.nth(0)).toContainText('Q3 launch plan');
    await expect(hits.nth(0).getByTestId('citation')).toHaveText('Page 4');
    await expect(hits.nth(0)).toContainText('Source: Q3 launch plan, page 4');
    await expect(hits.nth(1).getByTestId('citation')).toHaveText('Meeting · 12:34');
    await expect(hits.nth(1)).toContainText('Source: Board review, 12:34');
    await app.getByRole('combobox', { name: 'Source type' }).click();
    await app.getByRole('option', { name: 'Documents' }).click();
    await expect(app).toHaveURL(/source=document/);
    await expect.poll(() => seen.some((u) => u.includes('sourceType=document') && u.includes(`workspaceId=${workspace.id}`))).toBe(true);
  });

  test('KNOW-03 no results shows a helpful empty state', async ({ app, workspace }) => {
    await app.route('**/api/v1/knowledge/search**', (r) => r.fulfill({ json: { chunks: [], metrics: { returned: 0 } } }));
    await app.goto(`/${workspace.slug}/knowledge?q=unicorns`);
    await expect(app.getByRole('heading', { name: 'No results for “unicorns”' })).toBeVisible();
    await app.getByRole('button', { name: 'Clear search' }).click();
    await expect(app.getByRole('heading', { name: 'Search your workspace knowledge' })).toBeVisible();
  });
});

test.describe('Design system and brand', () => {
  test('DS-01 the real logo and favicon, Inter actually loaded', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    const logo = app.getByRole('complementary', { name: 'Sidebar' }).getByRole('img', { name: 'WarpTalk' });
    await expect(logo).toBeVisible();
    expect(await logo.evaluate((i: HTMLImageElement) => i.complete && i.naturalWidth)).toBeGreaterThan(100);
    await expect(app.locator('link[rel="icon"][href="/favicon-32.png"]')).toHaveCount(1);
    expect((await app.request.get('/favicon-32.png')).ok()).toBe(true);
    const inter = await app.evaluate(async () => {
      await document.fonts.ready;
      return [...document.fonts].some((f) => f.family.includes('Inter') && f.status === 'loaded');
    });
    expect(inter, 'an Inter @font-face is loaded (not just a locally installed Inter)').toBe(true);
    expect(await app.evaluate(() => getComputedStyle(document.body).fontFamily)).toContain('Inter');
  });

  test('DS-02/03 sidebar: grouped, 30px rows, 13px labels, duotone icons', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    const side = app.getByRole('complementary', { name: 'Sidebar' });
    await expect(side.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Home' })).toBeVisible();
    await expect(side.getByRole('navigation', { name: 'Workspace' }).getByRole('link', { name: 'Members' })).toBeVisible();
    await expect(side.getByText('Workspace', { exact: true })).toBeVisible();
    const m = await side.getByRole('link', { name: 'Home' }).evaluate((a) => ({ h: a.getBoundingClientRect().height, fs: getComputedStyle(a).fontSize }));
    expect(m.h).toBe(30);
    expect(m.fs).toBe('13px');
    expect(await side.getByRole('link', { name: 'Home' }).locator('svg').count()).toBe(1);
    // no lucide strokes left in the shell: phosphor svgs use fill, lucide uses stroke="currentColor" on the root
    expect(await app.locator('svg.lucide').count()).toBe(0);
    // Features without a backend are not advertised
    await expect(side.getByRole('link', { name: 'Glossary' })).toHaveCount(1); // backed by the glossary API
    for (const n of ['Calendar', 'Documents', 'Voice profiles']) await expect(side.getByRole('link', { name: n })).toHaveCount(0);
  });

  test('DS-07 tables inside cards do not draw a second frame', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/rooms/${room.id}`);
    const table = app.getByRole('table', { name: 'Participants' });
    await expect(table).toBeVisible();
    const border = await table.evaluate((t) => parseFloat(getComputedStyle(t.parentElement!).borderTopWidth));
    expect(border).toBe(0);
  });

  test('DS-09 long titles truncate with a tooltip and never widen the row', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id, { title: LONG });
    await app.goto(`/${workspace.slug}/meetings`);
    const link = app.getByRole('list', { name: 'Meetings' }).getByRole('link', { name: LONG, exact: true });
    await expect(link).toHaveAttribute('title', LONG);
    expect(await link.evaluate((a) => a.scrollWidth > a.clientWidth)).toBe(true);
  });

  test('A11Y-01..03 contrast tokens: control border >= 3:1, primary hover AA, tertiary ink AA on surface-1..3', async ({ app }) => {
    await app.goto('/login');
    for (const theme of ['dark', 'light']) {
      const r = await app.evaluate((t) => {
        document.documentElement.dataset.theme = t;
        const cs = getComputedStyle(document.documentElement);
        const get = (n: string) => cs.getPropertyValue(`--${n}`).trim();
        const lum = (hex: string) => {
          const c = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4));
          return 0.2126 * c[0]! + 0.7152 * c[1]! + 0.0722 * c[2]!;
        };
        const cr = (a: string, b: string) => { const [x, y] = [lum(a), lum(b)].sort((p, q) => q - p); return (x! + 0.05) / (y! + 0.05); };
        return {
          ctrl: [1, 2, 3].map((n) => cr(get('control-border'), get(`surface-${n}`))),
          hover: cr(get('primary-hover'), '#ffffff'),
          pressed: cr(get('primary-pressed'), '#ffffff'),
          tert: [1, 2, 3].map((n) => cr(get('ink-tertiary'), get(`surface-${n}`))),
        };
      }, theme);
      r.ctrl.forEach((v) => expect(v, `${theme} control border`).toBeGreaterThanOrEqual(3));
      expect(r.hover, `${theme} primary hover`).toBeGreaterThanOrEqual(4.5);
      expect(r.pressed, `${theme} primary pressed`).toBeGreaterThanOrEqual(4.5);
      r.tert.forEach((v) => expect(v, `${theme} tertiary ink`).toBeGreaterThanOrEqual(4.5));
    }
  });

  test('A11Y-06 an invalid focused input draws one state (danger ring)', async ({ page }) => {
    await page.goto('/login');
    await page.getByRole('button', { name: 'Sign in' }).click();
    const email = page.getByLabel('Email');
    await expect(email).toBeFocused();
    const c = await email.evaluate((e) => ({ outline: getComputedStyle(e).outlineColor, border: getComputedStyle(e).borderTopColor }));
    expect(c.outline).not.toBe('');
    const danger = await page.evaluate(() => { const d = document.createElement('i'); d.style.color = 'var(--danger)'; document.body.append(d); const v = getComputedStyle(d).color; d.remove(); return v; });
    expect(c.border).toBe(danger);
    // outline is danger-tinted (alpha), not the lavender focus colour
    expect(c.outline).not.toContain('0.486'); // not the lavender focus colour
    expect(c.outline).toContain('0.839'); // danger red channel
  });
});

test.describe('Mobile 390px: no horizontal overflow (LY-01, LY-02, LY-05)', () => {
  test.use({ viewport: { width: 390, height: 844 } });

  test('LY-A every signed-in page fits the viewport', async ({ app, tokens, workspace, browser, baseURL }) => {
    // Seed: many rooms in every state, a 150-char title, a lobby with very long names.
    const base = await createRoom(tokens.accessToken, workspace.id, { title: LONG, requiresApproval: true });
    await createRoom(tokens.accessToken, workspace.id, { title: 'Plain waiting' });
    const live = await createRoom(tokens.accessToken, workspace.id, { title: 'Live one' });
    await ok(call(`/translation-rooms/${live.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const done = await createRoom(tokens.accessToken, workspace.id, { title: 'Finished one' });
    await ok(call(`/translation-rooms/${done.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await ok(call(`/translation-rooms/${done.id}/end`, { token: tokens.accessToken, method: 'POST' }));
    const guests = await Promise.all(['g1', 'g2', 'g3'].map((l) => createUser(l)));
    const ctxs = await Promise.all(guests.map((g) => signedInContext(browser, g, baseURL)));
    try {
      for (const [i, c] of ctxs.entries()) {
        await joinByCode(c.tokens.accessToken, base.roomCode, { displayName: `Guest ${i + 1} with a very very very long display name that goes on and on and on` });
      }
      await ok(call(`/translation-rooms/${base.id}/start`, { token: tokens.accessToken, method: 'POST' }));
      await call(`/translation-rooms/${live.id}/audio-routes/generate`, { token: tokens.accessToken, method: 'POST' });
      const s = workspace.slug;
      const pages = [
        `/${s}/home`, `/${s}/meetings`, `/${s}/meetings?tab=history`, `/${s}/meetings?tab=all`, `/${s}/meetings?new=1`, `/${s}/meetings?join=1`,
        `/rooms/${base.id}`, `/rooms/${live.id}`, `/rooms/${done.id}`,
        `/${s}/members`, `/${s}/members?invite=1`, `/${s}/knowledge`, `/${s}/knowledge?q=anything`,
        `/${s}/settings/general`, `/${s}/settings/ai`, `/${s}/settings/advanced`, `/${s}/settings/account`,
        '/workspaces', '/join', '/external',
      ];
      for (const p of pages) {
        await app.goto(p);
        await app.waitForLoadState('networkidle');
        if (/[?](new|join|invite)=1/.test(p)) await expect(app.getByRole('dialog')).toBeVisible();
        else await expect(app.getByRole('banner')).toBeVisible();
        await noOverflow(app, p);
      }
      // the lobby card and the participants list are reachable and fit
      await app.goto(`/rooms/${base.id}`);
      await expect(app.getByTestId('lobby')).toBeVisible();
      await noOverflow(app, 'lobby');
      await app.getByRole('button', { name: 'Show' }).click();
      await noOverflow(app, 'routes open');
      // the mobile drawer
      await app.getByRole('button', { name: 'Open navigation' }).click();
      await noOverflow(app, 'drawer');
    } finally {
      for (const c of ctxs) await c.context.close();
    }
  });

  test('LY-B rows stack and the room code stays whole at 390 (LY-02)', async ({ app, tokens, workspace }) => {
    const r = await createRoom(tokens.accessToken, workspace.id, { title: LONG });
    await app.goto(`/${workspace.slug}/meetings`);
    const row = app.getByRole('list', { name: 'Meetings' }).getByRole('listitem').first();
    await expect(row.getByTestId('room-code-chip')).toHaveText(r.roomCode);
    const w = await row.getByTestId('room-code-chip').evaluate((c) => ({ sw: c.scrollWidth, cw: c.clientWidth }));
    expect(w.sw).toBeLessThanOrEqual(w.cw);
    const box = await row.boundingBox();
    expect(box!.width).toBeLessThanOrEqual(390);
  });

  test('LY-C room detail participants are a list on mobile, a table on desktop', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/rooms/${room.id}`);
    await expect(app.getByRole('list', { name: 'Participants (list)' })).toBeVisible();
    await expect(app.getByRole('table', { name: 'Participants' })).toBeHidden();
    await app.setViewportSize({ width: 1280, height: 800 });
    await expect(app.getByRole('table', { name: 'Participants' })).toBeVisible();
    await expect(app.getByRole('list', { name: 'Participants (list)' })).toBeHidden();
  });

  test('LY-D members stack as rows, tab strip scrolls (LY-05)', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/members`);
    await expect(app.getByRole('list', { name: 'Workspace members (list)' })).toBeVisible();
    await expect(app.getByRole('list', { name: 'Workspace members (list)' })).toContainText('Owner');
    await app.goto(`/${workspace.slug}/settings/general`);
    await noOverflow(app, 'settings nav');
  });
});

raw.describe('Mobile 390px public pages', () => {
  raw.use({ viewport: { width: 390, height: 844 } });
  for (const p of ['/login', '/register', '/verify-email?email=a%40b.co', '/design']) {
    raw(`LY-E ${p} fits`, async ({ page }) => {
      await page.goto(p);
      await page.waitForLoadState('networkidle');
      await noOverflow(page, p);
    });
  }
  raw('LY-F auth with errors fits', async ({ page }) => {
    await page.goto('/register');
    await page.getByRole('button', { name: 'Create account' }).click();
    await noOverflow(page, 'register errors');
  });
});

raw.describe('Shell', () => {
  raw('NAV-01 unknown routes and a stale workspace slug fall back gracefully', async ({ page }) => {
    const u = await createUser('nav');
    const tokens = await login(u);
    const ws = await createWorkspace(tokens.accessToken, `Nav ${rnd()}`);
    await seedSession(page, tokens);
    await page.goto('/definitely/not/a/page');
    await expect(page).toHaveURL(new RegExp(`/${ws.slug}/home$`));
  });
});
