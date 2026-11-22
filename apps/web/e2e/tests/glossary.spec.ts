// Glossary: workspace glossaries (CRUD, inline terms, CSV import/export, RBAC), the system-admin global glossary,
// a11y and visual snapshots (dark/light, 1280 and 390). Real backend throughout.
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import {
  call, createRoom, createUser, createWorkspace, expect, inviteAndAccept, login, makeSystemAdmin, ok, rnd, seedSession, signedInContext, test, type Tokens,
} from '../fixtures';
import { test as raw } from '@playwright/test';

interface G { id: string; name: string; termCount: number; sourceLanguage: string; targetLanguage: string }
interface T { id: string; sourceTerm: string; targetTerm: string; caseSensitive: boolean; notes: string | null }
const mkGlossary = (token: string, wsId: string, name = `Terms ${rnd()}`) =>
  ok(call<G>('/glossaries', { token, body: { workspaceId: wsId, name, description: 'Product vocabulary', sourceLanguage: 'vi', targetLanguage: 'en' } }));
const addTerm = (token: string, id: string, s: string, t: string, notes?: string) =>
  ok(call<T>(`/glossaries/${id}/terms`, { token, body: { sourceTerm: s, targetTerm: t, caseSensitive: false, notes } }));
const terms = (token: string, id: string) => ok(call<T[]>(`/glossaries/${id}/terms`, { token }));

async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  expect(res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => `${v.id}: ${v.nodes[0]?.target.join(' ')}`), label).toEqual([]);
}

test.describe('Workspace glossary', () => {
  test('GLOS-01 owner creates a glossary from the Glossary nav item; validation; it opens on its detail page', async ({ app, tokens, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    await app.getByRole('link', { name: 'Glossary', exact: true }).click();
    await expect(app.getByRole('heading', { name: 'Glossary', level: 1 })).toBeVisible();
    await expect(app.getByText('No glossaries yet')).toBeVisible();
    await app.getByRole('button', { name: 'New glossary' }).first().click();
    const dlg = app.getByRole('dialog', { name: 'New glossary' });
    await dlg.getByRole('button', { name: 'Create glossary' }).click();
    await expect(dlg.getByText('Enter a name.')).toBeVisible();
    await dlg.getByLabel('Name').fill('Sales terms');
    await dlg.getByLabel('Description').fill('Pipeline vocabulary');
    await dlg.getByRole('combobox', { name: 'Target language' }).click();
    await app.getByRole('option', { name: 'Vietnamese' }).click(); // same as source
    await expect(dlg.getByText('Source and target language must differ.')).toBeVisible();
    await dlg.getByRole('combobox', { name: 'Target language' }).click();
    await app.getByRole('option', { name: 'Japanese' }).click();
    await dlg.getByRole('button', { name: 'Create glossary' }).click();
    await expect(app.getByRole('heading', { name: 'Sales terms', level: 1 })).toBeVisible();
    const list = await ok(call<G[]>(`/glossaries/workspace/${workspace.id}`, { token: tokens.accessToken }));
    expect(list).toHaveLength(1);
    expect(list[0]).toMatchObject({ name: 'Sales terms', sourceLanguage: 'vi', targetLanguage: 'ja' });
    // Duplicate names are refused by the API and reported in the dialog.
    await app.goto(`/${workspace.slug}/glossary`);
    await expect(app.getByRole('link', { name: 'Sales terms' })).toBeVisible();
    await app.getByRole('button', { name: 'New glossary' }).click();
    await app.getByRole('dialog').getByLabel('Name').fill('Sales terms');
    await app.getByRole('button', { name: 'Create glossary' }).click();
    await expect(app.getByRole('dialog').getByRole('alert')).toContainText('already exists');
  });

  test('GLOS-02 terms: inline add, inline edit, delete with confirm', async ({ app, tokens, workspace }) => {
    const g = await mkGlossary(tokens.accessToken, workspace.id);
    await app.goto(`/${workspace.slug}/glossary/${g.id}`);
    const add = app.getByRole('form', { name: 'Add term' });
    await add.getByRole('button', { name: 'Add term' }).click();
    await expect(add.getByText('Both the source term and the target term are required.')).toBeVisible();
    await add.getByLabel('Source term').fill('hợp đồng');
    await add.getByLabel('Target term').fill('contract');
    await add.getByLabel('Notes').fill('legal');
    await add.getByLabel('Case sensitive').check();
    await add.getByRole('button', { name: 'Add term' }).click();
    const row = app.getByTestId('term-row').filter({ hasText: 'hợp đồng' });
    await expect(row).toContainText('contract');
    await expect(row).toContainText('Case sensitive');
    await expect(add.getByLabel('Source term')).toHaveValue('');
    expect(await terms(tokens.accessToken, g.id)).toHaveLength(1);
    // Duplicate term: API error inline.
    await add.getByLabel('Source term').fill('hợp đồng');
    await add.getByLabel('Target term').fill('agreement');
    await add.getByRole('button', { name: 'Add term' }).click();
    await expect(add.getByRole('alert')).toBeVisible();
    await add.getByLabel('Source term').fill('');
    await add.getByLabel('Target term').fill('');
    // Edit inline.
    await row.getByRole('button', { name: 'Edit term hợp đồng' }).click();
    const edit = app.getByRole('form', { name: 'Edit term hợp đồng' });
    await edit.getByLabel('Target term').fill('contract (legal)');
    await edit.getByRole('button', { name: 'Save' }).click();
    await expect(app.getByTestId('term-row').filter({ hasText: 'contract (legal)' })).toBeVisible();
    expect((await terms(tokens.accessToken, g.id))[0]!.targetTerm).toBe('contract (legal)');
    // Delete: Cancel keeps, confirm removes.
    await app.getByRole('button', { name: 'Delete term hợp đồng' }).click();
    await app.getByRole('dialog').getByRole('button', { name: 'Cancel' }).click();
    await expect(app.getByTestId('term-row')).toHaveCount(1);
    await app.getByRole('button', { name: 'Delete term hợp đồng' }).click();
    await app.getByRole('dialog').getByRole('button', { name: 'Delete term' }).click();
    await expect(app.getByTestId('term-row')).toHaveCount(0);
    expect(await terms(tokens.accessToken, g.id)).toHaveLength(0);
  });

  test('GLOS-03 CSV import reports imported and skipped; export downloads the terms', async ({ app, tokens, workspace }) => {
    const g = await mkGlossary(tokens.accessToken, workspace.id);
    await addTerm(tokens.accessToken, g.id, 'báo giá', 'quote');
    await app.goto(`/${workspace.slug}/glossary/${g.id}`);
    const csv = 'source,target\nxin chào,hello\n"a,b","c"\nbáo giá,quotation\n';
    await app.locator('input[type=file]').setInputFiles({ name: 'terms.csv', mimeType: 'text/csv', buffer: Buffer.from(csv) });
    await expect(app.getByTestId('import-result')).toHaveText(/Imported 2 terms, skipped 1\./);
    await expect(app.getByTestId('term-row')).toHaveCount(3);
    expect((await terms(tokens.accessToken, g.id)).map((t) => t.sourceTerm).sort()).toEqual(['a,b', 'báo giá', 'xin chào']);
    // A malformed file is refused with the API message.
    await app.locator('input[type=file]').setInputFiles({ name: 'bad.csv', mimeType: 'text/csv', buffer: Buffer.from('only-one-column\n') });
    await expect(app.getByRole('alert').filter({ hasText: 'Import failed' })).toBeVisible();
    const [dl] = await Promise.all([app.waitForEvent('download'), app.getByRole('button', { name: 'Export CSV' }).click()]);
    expect(dl.suggestedFilename()).toMatch(/\.csv$/);
    const body = await (await import('node:fs/promises')).readFile(await dl.path(), 'utf8');
    expect(body).toContain('xin chào,hello');
    expect(body).toContain('"a,b",c');
  });

  test('GLOS-04 members see a read-only glossary (no edit controls, export allowed); the API refuses their writes', async ({ app, tokens, workspace, browser, baseURL }) => {
    const g = await mkGlossary(tokens.accessToken, workspace.id, 'Shared terms');
    await addTerm(tokens.accessToken, g.id, 'hello', 'xin chào');
    const member = await createUser('reader');
    await inviteAndAccept(tokens.accessToken, workspace.id, member);
    void app;
    const m = await signedInContext(browser, member, baseURL);
    try {
      await m.page.goto(`/${workspace.slug}/glossary`);
      await expect(m.page.getByRole('button', { name: 'New glossary' })).toHaveCount(0);
      await expect(m.page.getByText('Read-only', { exact: true })).toBeVisible();
      await m.page.getByRole('link', { name: 'Shared terms' }).click();
      await expect(m.page.getByTestId('term-row')).toHaveCount(1);
      for (const name of ['Import CSV', 'Edit', 'Delete']) await expect(m.page.getByRole('button', { name, exact: true })).toHaveCount(0);
      await expect(m.page.getByRole('form', { name: 'Add term' })).toHaveCount(0);
      await expect(m.page.getByRole('button', { name: /^Edit term/ })).toHaveCount(0);
      await expect(m.page.getByRole('button', { name: 'Export CSV' })).toBeVisible();
      const t = m.tokens.accessToken;
      expect((await call(`/glossaries/${g.id}/terms`, { token: t, body: { sourceTerm: 'x', targetTerm: 'y', caseSensitive: false } })).status).toBe(403);
      expect((await call(`/glossaries/${g.id}`, { token: t, method: 'DELETE' })).status).toBe(403);
      expect((await call('/glossaries', { token: t, body: { workspaceId: workspace.id, name: 'n', sourceLanguage: 'vi', targetLanguage: 'en' } })).status).toBe(403);
      expect((await call(`/glossaries/${g.id}/terms`, { token: t })).status).toBe(200);
    } finally {
      await m.context.close();
    }
  });

  test('GLOS-05 edit and delete a glossary; an outsider cannot read it', async ({ app, tokens, workspace }) => {
    const g = await mkGlossary(tokens.accessToken, workspace.id, 'Old name');
    await addTerm(tokens.accessToken, g.id, 'a', 'b');
    await app.goto(`/${workspace.slug}/glossary/${g.id}`);
    await app.getByRole('button', { name: 'Edit', exact: true }).click();
    await app.getByRole('dialog').getByLabel('Name').fill('New name');
    await app.getByRole('button', { name: 'Save changes' }).click();
    await expect(app.getByRole('heading', { name: 'New name', level: 1 })).toBeVisible();
    const outsider = await login(await createUser('out'));
    expect((await call(`/glossaries/${g.id}`, { token: outsider.accessToken })).status).toBeGreaterThanOrEqual(403);
    await app.getByRole('button', { name: 'Delete', exact: true }).click();
    await expect(app.getByRole('dialog')).toContainText('1 terms');
    await app.getByRole('dialog').getByRole('button', { name: 'Delete glossary' }).click();
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/glossary$`));
    await expect(app.getByText('No glossaries yet')).toBeVisible();
    expect((await call(`/glossaries/${g.id}`, { token: tokens.accessToken })).status).toBe(404);
  });
});

test.describe('Global glossary (system admin)', () => {
  test('GLOS-06 only system admins see and use it: draft -> publish -> visible to everyone -> edit -> audits -> archive -> delete', async ({ app, user, tokens, workspace, browser, baseURL }) => {
    test.setTimeout(90_000);
    // A plain owner has no entry and the API says 403.
    await app.goto(`/${workspace.slug}/settings/advanced`);
    await expect(app.getByRole('link', { name: 'AI providers' })).toBeVisible(); // settled (and signed in) before looking for absence
    await expect(app.getByRole('link', { name: 'Global glossary' })).toHaveCount(0);
    expect((await call('/admin/global-glossary', { token: tokens.accessToken })).status).toBe(403);
    await app.goto(`/${workspace.slug}/settings/global-glossary`);
    await expect(app.getByText('Not available')).toBeVisible();

    makeSystemAdmin(user.userId);
    const sa = await signedInContext(browser, user, baseURL);
    const p = sa.page;
    const term = `Churn ${rnd()}`;
    try {
      await p.goto(`/${workspace.slug}/settings/advanced`);
      await expect(p.getByRole('link', { name: 'Manage global glossary' })).toBeVisible();
      await p.getByRole('link', { name: 'Global glossary' }).first().click();
      await p.getByRole('button', { name: 'New term' }).click();
      const dlg = p.getByRole('dialog', { name: 'New global term' });
      await dlg.getByRole('button', { name: 'Create draft' }).click();
      await expect(dlg.getByText('Enter the term.')).toBeVisible();
      await dlg.getByRole('textbox', { name: /^Term\b/ }).fill(term);
      await dlg.getByLabel('Preferred translation').fill('tỷ lệ rời bỏ');
      await dlg.getByLabel('Business domain').fill('SaaS');
      await dlg.getByLabel('Definition').fill('Share of customers who stop paying in a period.');
      await dlg.getByRole('button', { name: 'Create draft' }).click();
      const row = p.getByTestId('global-term-row').filter({ hasText: term });
      await expect(row).toHaveAttribute('data-status', 'draft');
      // Drafts are not published to anybody.
      expect((await ok(call<{ term: string }[]>('/glossaries/global', { token: tokens.accessToken }))).map((t) => t.term)).not.toContain(term);
      await row.getByRole('button', { name: `Publish ${term}` }).click();
      await p.getByRole('dialog').getByRole('button', { name: 'Publish' }).click();
      await expect(row).toHaveAttribute('data-status', 'published');
      expect((await ok(call<{ term: string }[]>('/glossaries/global', { token: tokens.accessToken }))).map((t) => t.term)).toContain(term);
      // Everybody sees it, read-only, on their Glossary page.
      const reader = await createUser('globalreader');
      await inviteAndAccept(tokens.accessToken, workspace.id, reader);
      const r = await signedInContext(browser, reader, baseURL);
      try {
        await r.page.goto(`/${workspace.slug}/glossary`);
        await expect(r.page.getByRole('table', { name: 'Global glossary terms' })).toContainText(term);
      } finally {
        await r.context.close();
      }
      // Edit bumps the version; the audit drawer lists the trail.
      await row.getByRole('button', { name: `Edit ${term}` }).click();
      await p.getByRole('dialog').getByLabel('Preferred translation').fill('tỉ lệ rời bỏ');
      await p.getByRole('button', { name: 'Save changes' }).click();
      await expect(row).toContainText('tỉ lệ rời bỏ');
      await row.getByRole('button', { name: `Audit history for ${term}` }).click();
      const drawer = p.getByRole('dialog', { name: 'Audit history' });
      await expect(drawer.getByTestId('audit-entry').first()).toBeVisible();
      expect(await drawer.getByTestId('audit-entry').count()).toBeGreaterThanOrEqual(3);
      await scan(p, 'global glossary audit drawer');
      await p.keyboard.press('Escape');
      // Filter, archive, delete.
      await row.getByRole('button', { name: `Archive ${term}` }).click();
      await p.getByRole('dialog').getByRole('button', { name: 'Archive' }).click();
      await expect(row).toHaveAttribute('data-status', 'archived');
      expect((await ok(call<{ term: string }[]>('/glossaries/global', { token: tokens.accessToken }))).map((t) => t.term)).not.toContain(term);
      await row.getByRole('button', { name: `Delete ${term}` }).click();
      await p.getByRole('dialog').getByRole('button', { name: 'Delete' }).click();
      await expect(row).toHaveCount(0);
    } finally {
      await sa.context.close();
    }
  });
});

// ---- a11y + visual: deterministic content, both themes, 1280 and 390
raw.describe('Glossary a11y and visuals', () => {
  async function seeded() {
    const id = rnd();
    const u = { email: `e2e-glos-${id}@example.com`, password: 'Passw0rd!e2e', fullName: 'Ada Glossary' };
    await ok(call('/auth/register', { body: u }));
    const { verifyEmail } = await import('../fixtures');
    await verifyEmail(u.email);
    const tokens: Tokens = await login({ ...u, userId: '' });
    const ws = await createWorkspace(tokens.accessToken, 'Glossary HQ');
    const g = await mkGlossary(tokens.accessToken, ws.id, 'Product terms');
    await mkGlossary(tokens.accessToken, ws.id, 'Legal terms');
    for (const [s, t, n] of [['hợp đồng', 'contract', 'legal'], ['báo giá', 'quote', ''], ['khách hàng', 'customer', 'CRM']] as const) await addTerm(tokens.accessToken, g.id, s, t, n || undefined);
    void createRoom;
    return { tokens, ws, g };
  }
  const sizes = [{ w: 1280, h: 800 }, { w: 390, h: 844 }] as const;
  for (const theme of ['dark', 'light'] as const) {
    for (const s of sizes) {
      raw(`GLOS-07 list + detail a11y and snapshot ${s.w} ${theme}`, async ({ page }) => {
        const d = await seeded();
        await page.setViewportSize({ width: s.w, height: s.h });
        await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await seedSession(page, d.tokens);
        // Published global terms are shared platform state (GLOS-06 publishes one): pin the card out for stable pixels.
        await page.route('**/glossaries/global', (r) => r.fulfill({ status: 200, contentType: 'application/json', body: '[]' }));
        const masks = () => [page.getByTestId('account-menu'), page.getByTestId('glossary-updated')];
        const settle = async () => { await page.evaluate(() => document.fonts.ready); await page.waitForTimeout(150); };
        await page.goto(`/${d.ws.slug}/glossary`);
        await expect(page.getByRole('link', { name: 'Product terms' })).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await scan(page, `glossary list ${s.w} ${theme}`);
        await settle();
        await expect(page).toHaveScreenshot(`glossary-list-${s.w}-${theme}.png`, { mask: masks() });
        await page.goto(`/${d.ws.slug}/glossary/${d.g.id}`);
        await expect(page.getByRole('heading', { name: 'Product terms', level: 1 })).toBeVisible();
        await expect(page.getByText('hợp đồng').locator('visible=true').first()).toBeVisible();
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await scan(page, `glossary detail ${s.w} ${theme}`);
        await settle();
        await expect(page).toHaveScreenshot(`glossary-detail-${s.w}-${theme}.png`, { mask: masks() });
      });
    }
  }
});
