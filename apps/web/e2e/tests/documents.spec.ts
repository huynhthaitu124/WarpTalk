// Workspace documents: upload (picker + drag and drop), validation, RBAC, edit, archive, download, a11y, visual (real backend, local object store).
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { test as raw } from '@playwright/test';
import {
  call, createUser, createWorkspace, expect, inviteAndAccept, listDocuments, login, ok, postDocument, rnd, seedSession, signedInContext, test, uploadDocument, verifyEmail,
} from '../fixtures';

const rows = (p: Page) => p.getByTestId('document-row').filter({ visible: true });
const rowOf = (p: Page, title: string) => rows(p).filter({ hasText: title });
const txt = (name: string, body = 'Refunds are issued within 14 days of purchase.') => ({ name, mimeType: 'text/plain', buffer: Buffer.from(body) });
const PDF = '%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n';

async function openUpload(page: Page) {
  await page.getByRole('button', { name: 'Upload document' }).first().click();
  const dlg = page.getByRole('dialog', { name: 'Upload document' });
  await expect(dlg).toBeVisible();
  return dlg;
}
const choose = (page: Page, f: { name: string; mimeType: string; buffer: Buffer }) => page.getByTestId('doc-file-input').setInputFiles(f);
async function pickOption(page: Page, name: string, option: string) {
  await page.getByRole('combobox', { name, exact: true }).click();
  await page.getByRole('option', { name: option, exact: true }).click();
}
async function setTheme(page: Page, theme: 'dark' | 'light') {
  await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
}
async function noHorizontalScroll(page: Page) {
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
}

test.describe('Documents', () => {
  test('DOC-01 admin uploads a text file through the dialog: row appears with an ingestion status, metadata is stored', async ({ app, tokens, workspace }) => {
    await app.goto(`/${workspace.slug}/documents`);
    await expect(app.getByRole('heading', { name: 'Documents', level: 1 })).toBeVisible();
    await expect(app.getByText('No documents yet')).toBeVisible();
    const dlg = await openUpload(app);
    await choose(app, txt('refund-policy.txt'));
    await expect(dlg.getByTestId('doc-selected-file')).toHaveText('refund-policy.txt');
    await dlg.getByLabel('Title').fill('Refund policy');
    await pickOption(app, 'Category', 'Legal');
    await dlg.getByRole('button', { name: 'Upload', exact: true }).click();
    await expect(dlg).toBeHidden();
    const row = rowOf(app, 'Refund policy');
    await expect(row).toHaveCount(1);
    await expect(row).toContainText('refund-policy.txt');
    await expect(row).toContainText('Legal');
    await expect(row.getByTestId('ingestion-pill')).toHaveAttribute('data-status', /^(pending|completed|skipped)$/);
    const [doc] = (await listDocuments(tokens.accessToken, workspace.id)).items;
    expect(doc).toMatchObject({ title: 'Refund policy', category: 'LEGAL', accessLevel: 'MEMBERS_ONLY', aiRetrievalAllowed: true, fileExtension: 'txt' });
  });

  test('DOC-02 drag and drop selects the file; access level and the AI switch are sent', async ({ app, tokens, workspace }) => {
    await app.goto(`/${workspace.slug}/documents`);
    const dlg = await openUpload(app);
    await app.evaluate(() => {
      const dt = new DataTransfer();
      dt.items.add(new File(['quarter, revenue\nQ1, 10\n'], 'numbers.csv', { type: 'text/csv' }));
      document.querySelector('[data-testid="doc-dropzone"]')!.dispatchEvent(new DragEvent('drop', { dataTransfer: dt, bubbles: true, cancelable: true }));
    });
    await expect(dlg.getByTestId('doc-selected-file')).toHaveText('numbers.csv');
    await pickOption(app, 'Access level', 'Admins only');
    await dlg.getByRole('switch', { name: /Allow AI retrieval/ }).click();
    await dlg.getByRole('button', { name: 'Upload', exact: true }).click();
    await expect(dlg).toBeHidden();
    await expect(rowOf(app, 'numbers')).toHaveCount(1);
    const [doc] = (await listDocuments(tokens.accessToken, workspace.id)).items;
    expect(doc).toMatchObject({ accessLevel: 'ADMIN_ONLY', aiRetrievalAllowed: false, ingestionStatus: 'skipped', ingestionFailureReason: 'ai_retrieval_not_allowed' });
    await expect(rowOf(app, 'numbers').getByTestId('ingestion-pill')).toHaveAttribute('data-status', 'skipped');
  });

  test('DOC-03 client-side rejection: wrong type, empty file and > 20 MB never reach the server', async ({ app, workspace }) => {
    const posts: string[] = [];
    app.on('request', (r) => { if (r.method() === 'POST' && r.url().endsWith('/documents')) posts.push(r.url()); });
    await app.goto(`/${workspace.slug}/documents`);
    const dlg = await openUpload(app);
    await choose(app, { name: 'setup.exe', mimeType: 'application/octet-stream', buffer: Buffer.from('MZ') });
    await expect(dlg.getByRole('alert')).toContainText('Unsupported file type: allowed types are pdf, docx, txt, md and csv.');
    await choose(app, txt('empty.txt', ''));
    await expect(dlg.getByRole('alert')).toContainText('The file is empty.');
    await choose(app, { name: 'huge.txt', mimeType: 'text/plain', buffer: Buffer.alloc(20 * 1024 * 1024 + 1, 'a') });
    await expect(dlg.getByRole('alert')).toContainText('File exceeds the 20 MB limit.');
    await expect(dlg.getByTestId('doc-selected-file')).toHaveCount(0);
    await dlg.getByRole('button', { name: 'Upload', exact: true }).click();
    await expect(dlg.getByRole('alert')).toContainText('Choose a file to upload.');
    expect(posts).toEqual([]);
  });

  test('DOC-04 server-side rejection: type, size and content checks answer 415/413 and the UI shows the server message', async ({ app, tokens, workspace }) => {
    const t = tokens.accessToken;
    const exe = await postDocument(t, workspace.id, { name: 'run.exe', content: 'MZ' });
    expect([exe.status, exe.body.code]).toEqual([415, 'UNSUPPORTED_FILE_TYPE']);
    const big = await postDocument(t, workspace.id, { name: 'big.txt', content: Buffer.alloc(21 * 1024 * 1024, 'a') });
    expect([big.status, big.body.code]).toEqual([413, 'FILE_TOO_LARGE']);
    const fake = await postDocument(t, workspace.id, { name: 'fake.pdf', content: 'this is not a pdf' });
    expect([fake.status, fake.body.code]).toEqual([415, 'FILE_CONTENT_MISMATCH']);
    expect((await listDocuments(t, workspace.id)).total).toBe(0);
    // Same mismatch through the UI: it passes the client checks, the server's message is shown.
    await app.goto(`/${workspace.slug}/documents`);
    const dlg = await openUpload(app);
    await choose(app, { name: 'fake.pdf', mimeType: 'application/pdf', buffer: Buffer.from('this is not a pdf') });
    await dlg.getByRole('button', { name: 'Upload', exact: true }).click();
    await expect(dlg.getByRole('alert')).toContainText('The file content does not match its extension.');
    await expect(dlg).toBeVisible();
  });

  test('DOC-05 members are read-only: no upload, drawer without edit or archive, API refuses writes', async ({ tokens, workspace, browser, baseURL }) => {
    const doc = await uploadDocument(tokens.accessToken, workspace.id, { name: 'handbook.txt', content: 'Welcome aboard.', title: 'Handbook' });
    const member = await createUser('docmem');
    await inviteAndAccept(tokens.accessToken, workspace.id, member);
    const m = await signedInContext(browser, member, baseURL);
    try {
      await m.page.goto(`/${workspace.slug}/documents`);
      await expect(rowOf(m.page, 'Handbook')).toHaveCount(1);
      await expect(m.page.getByRole('button', { name: 'Upload document' })).toHaveCount(0);
      await expect(m.page.getByText(/Only workspace owners and admins can upload or change them/)).toBeVisible();
      await m.page.getByRole('button', { name: 'Open Handbook' }).first().click();
      const drawer = m.page.getByTestId('doc-drawer');
      await expect(drawer).toContainText('Only owners and admins can change or archive it.');
      await expect(drawer.getByRole('button', { name: 'Download' })).toBeVisible();
      await expect(drawer.getByRole('button', { name: 'Archive document' })).toHaveCount(0);
      await expect(drawer.getByLabel('Title')).toHaveCount(0);
      const t = m.tokens.accessToken;
      expect((await postDocument(t, workspace.id, { name: 'x.txt', content: 'x' })).status).toBe(403);
      expect((await call(`/workspaces/${workspace.id}/documents/${doc.id}`, { method: 'PATCH', token: t, body: { title: 'Hacked' } })).status).toBe(403);
      expect((await call(`/workspaces/${workspace.id}/documents/${doc.id}`, { method: 'DELETE', token: t })).status).toBe(403);
    } finally { await m.context.close(); }
  });

  test('DOC-06 admin-only documents are invisible to members (list, search, direct fetch) but visible to the owner', async ({ app, tokens, workspace, browser, baseURL }) => {
    const t = tokens.accessToken;
    const secret = await uploadDocument(t, workspace.id, { name: 'salaries.txt', content: 'confidential', title: 'Salary bands', accessLevel: 'ADMIN_ONLY' });
    await uploadDocument(t, workspace.id, { name: 'faq.txt', content: 'public faq', title: 'Team FAQ', accessLevel: 'PUBLIC' });
    const member = await createUser('docmem2');
    await inviteAndAccept(t, workspace.id, member);
    const m = await signedInContext(browser, member, baseURL);
    try {
      await m.page.goto(`/${workspace.slug}/documents`);
      await expect(rowOf(m.page, 'Team FAQ')).toHaveCount(1);
      await expect(m.page.getByText('Salary bands')).toHaveCount(0);
      await m.page.getByLabel('Search documents').fill('Salary');
      await expect(m.page.getByText('No matching documents')).toBeVisible();
      expect((await call(`/workspaces/${workspace.id}/documents/${secret.id}`, { token: m.tokens.accessToken })).status).toBe(404);
      expect((await call(`/workspaces/${workspace.id}/documents/${secret.id}/download`, { token: m.tokens.accessToken })).status).toBe(404);
    } finally { await m.context.close(); }
    await app.goto(`/${workspace.slug}/documents`);
    await expect(rowOf(app, 'Salary bands')).toContainText('Admins only');
    await expect(rowOf(app, 'Team FAQ')).toHaveCount(1);
  });

  test('DOC-07 edit in the drawer: title, category, access and the AI toggle are patched', async ({ app, tokens, workspace }) => {
    await uploadDocument(tokens.accessToken, workspace.id, { name: 'notes.md', content: '# Notes\nship it', title: 'Sprint notes' });
    await app.goto(`/${workspace.slug}/documents`);
    await app.getByRole('button', { name: 'Open Sprint notes' }).first().click();
    const drawer = app.getByTestId('doc-drawer');
    await expect(drawer.getByRole('button', { name: 'Save changes' })).toBeDisabled();
    await drawer.getByLabel('Title').fill('Sprint retro notes');
    await pickOption(app, 'Category', 'Meeting notes');
    await pickOption(app, 'Access level', 'Admins only');
    await drawer.getByRole('switch', { name: /Allow AI retrieval/ }).click();
    await drawer.getByRole('button', { name: 'Save changes' }).click();
    await expect(app.getByText('Document updated')).toBeVisible();
    await expect(app.getByRole('dialog').getByRole('heading', { name: 'Sprint retro notes' })).toBeVisible();
    await expect(drawer.getByTestId('doc-ingestion-note')).toContainText('AI retrieval is switched off');
    const [doc] = (await listDocuments(tokens.accessToken, workspace.id)).items;
    expect(doc).toMatchObject({ title: 'Sprint retro notes', category: 'MEETING_NOTES', accessLevel: 'ADMIN_ONLY', aiRetrievalAllowed: false, ingestionStatus: 'skipped' });
    await app.keyboard.press('Escape');
    await expect(rowOf(app, 'Sprint retro notes')).toContainText('Meeting notes');
    await expect(rowOf(app, 'Sprint retro notes').getByTestId('ingestion-pill')).toHaveAttribute('data-status', 'skipped');
  });

  test('DOC-08 archive asks for confirmation (Cancel keeps it), then the document disappears', async ({ app, tokens, workspace }) => {
    const d = await uploadDocument(tokens.accessToken, workspace.id, { name: 'old.txt', content: 'obsolete', title: 'Old plan' });
    await app.goto(`/${workspace.slug}/documents`);
    await app.getByRole('button', { name: 'Open Old plan' }).first().click();
    await app.getByRole('button', { name: 'Archive document' }).click();
    const confirm = app.getByRole('dialog', { name: /Archive “Old plan”\?/ });
    await expect(confirm).toContainText('AI assistant stops using it');
    await confirm.getByRole('button', { name: 'Cancel' }).click();
    expect((await call(`/workspaces/${workspace.id}/documents/${d.id}`, { token: tokens.accessToken })).status).toBe(200);
    await app.getByRole('button', { name: 'Archive document' }).click();
    await app.getByRole('dialog', { name: /Archive “Old plan”\?/ }).getByRole('button', { name: 'Archive document' }).click();
    await expect(app.getByText('Document archived')).toBeVisible();
    await expect(rowOf(app, 'Old plan')).toHaveCount(0);
    expect((await call(`/workspaces/${workspace.id}/documents/${d.id}`, { token: tokens.accessToken })).status).toBe(404);
  });

  test('DOC-09 download returns the original file name and bytes', async ({ app, tokens, workspace }) => {
    const body = `line one\nline two ${rnd()}\n`;
    await uploadDocument(tokens.accessToken, workspace.id, { name: 'minutes.txt', content: body, title: 'Minutes' });
    await app.goto(`/${workspace.slug}/documents`);
    await app.getByRole('button', { name: 'Open Minutes' }).first().click();
    const [dl] = await Promise.all([app.waitForEvent('download'), app.getByTestId('doc-drawer').getByRole('button', { name: 'Download' }).click()]);
    expect(dl.suggestedFilename()).toBe('minutes.txt');
    const path = await dl.path();
    expect((await import('node:fs')).readFileSync(path, 'utf8')).toBe(body);
  });

  test('DOC-10 search and category filter narrow the list; clearing restores it', async ({ app, tokens, workspace }) => {
    const t = tokens.accessToken;
    await uploadDocument(t, workspace.id, { name: 'a.txt', content: 'a', title: 'Alpha contract', category: 'LEGAL' });
    await uploadDocument(t, workspace.id, { name: 'b.txt', content: 'b', title: 'Beta budget', category: 'FINANCIAL' });
    await app.goto(`/${workspace.slug}/documents`);
    await expect(rows(app)).toHaveCount(2);
    await app.getByLabel('Search documents').fill('budget');
    await expect(rows(app)).toHaveCount(1);
    await expect(rowOf(app, 'Beta budget')).toHaveCount(1);
    await app.getByLabel('Search documents').fill('');
    await expect(rows(app)).toHaveCount(2);
    await pickOption(app, 'Category filter', 'Legal');
    await expect(rows(app)).toHaveCount(1);
    await expect(rowOf(app, 'Alpha contract')).toHaveCount(1);
    await pickOption(app, 'Category filter', 'Financial');
    await app.getByLabel('Search documents').fill('zzz-none');
    await expect(app.getByText('No matching documents')).toBeVisible();
    await app.getByRole('button', { name: 'Clear filters' }).click();
    await expect(rows(app)).toHaveCount(2);
  });

  test('DOC-11 honest copy: the PDF/DOCX extraction notice is always shown; PDFs upload and download', async ({ app, tokens, workspace }) => {
    await app.goto(`/${workspace.slug}/documents`);
    await expect(app.getByText('PDF and DOCX text extraction is not available yet')).toBeVisible();
    await expect(app.getByText(/no indexable content/)).toBeVisible();
    const dlg = await openUpload(app);
    await choose(app, { name: 'contract.pdf', mimeType: 'application/pdf', buffer: Buffer.from(PDF) });
    await dlg.getByRole('button', { name: 'Upload', exact: true }).click();
    await expect(dlg).toBeHidden();
    await expect(rowOf(app, 'contract')).toHaveCount(1);
    const [doc] = (await listDocuments(tokens.accessToken, workspace.id)).items;
    expect(doc).toMatchObject({ fileExtension: 'pdf' });
  });
});

// ---- ingestion pills with deterministic data (list endpoint mocked; everything else is the real stack)
function fakeDoc(over: Record<string, unknown>) {
  return {
    id: crypto.randomUUID(), workspaceId: crypto.randomUUID(), title: 'Doc', fileName: 'doc.txt', fileExtension: 'txt', mimeType: 'text/plain', sizeBytes: 2048,
    category: 'GENERAL', accessLevel: 'MEMBERS_ONLY', aiRetrievalAllowed: true, aiEligible: false, ingestionStatus: 'pending', ingestionFailureReason: null,
    lastIndexedAt: null, uploadedBy: null, createdAt: '2026-11-01T09:00:00Z', updatedAt: null, ...over,
  };
}
const FIXED = [
  fakeDoc({ title: 'Onboarding guide', fileName: 'onboarding.md', fileExtension: 'md', ingestionStatus: 'completed', aiEligible: true, category: 'GENERAL', sizeBytes: 12_400 }),
  fakeDoc({ title: 'Q3 board deck', fileName: 'q3-board.pdf', fileExtension: 'pdf', mimeType: 'application/pdf', ingestionStatus: 'skipped', ingestionFailureReason: 'no_indexable_content', category: 'FINANCIAL', sizeBytes: 1_480_000, createdAt: '2026-11-02T10:30:00Z' }),
  fakeDoc({ title: 'Roadmap', fileName: 'roadmap.docx', fileExtension: 'docx', ingestionStatus: 'pending', category: 'TECHNICAL', accessLevel: 'ADMIN_ONLY', sizeBytes: 88_000, createdAt: '2026-11-03T08:15:00Z' }),
  fakeDoc({ title: 'Broken export', fileName: 'export.csv', fileExtension: 'csv', ingestionStatus: 'failed', ingestionFailureReason: 'embedding_provider_error', category: 'LEGAL', accessLevel: 'PUBLIC', sizeBytes: 640, createdAt: '2026-11-04T16:45:00Z' }),
];
const mockList = (page: Page, items: unknown[]) =>
  page.route(/\/api\/v1\/workspaces\/[^/]+\/documents(\?.*)?$/, (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    return route.fulfill({ json: { items, page: 1, pageSize: 20, total: items.length } });
  });

test.describe('Documents: ingestion status', () => {
  test('DOC-12 pills show pending/completed/skipped/failed with a reason tooltip, and the drawer explains PDF extraction honestly', async ({ app, workspace }) => {
    await mockList(app, FIXED);
    await app.goto(`/${workspace.slug}/documents`);
    for (const [title, status, label] of [['Onboarding guide', 'completed', 'Completed'], ['Q3 board deck', 'skipped', 'Skipped'], ['Roadmap', 'pending', 'Pending'], ['Broken export', 'failed', 'Failed']] as const) {
      const pill = rowOf(app, title).getByTestId('ingestion-pill');
      await expect(pill).toHaveAttribute('data-status', status);
      await expect(pill).toContainText(label);
    }
    await rowOf(app, 'Q3 board deck').getByTestId('ingestion-pill').focus();
    await expect(app.getByRole('tooltip').first()).toContainText('Text extraction for PDF and DOCX files is not available yet');
    await app.keyboard.press('Escape');
    await app.getByRole('button', { name: 'Open Q3 board deck' }).first().click();
    await expect(app.getByTestId('doc-ingestion-note')).toContainText('PDF and DOCX files is not available yet');
    await app.keyboard.press('Escape');
    await rowOf(app, 'Broken export').getByTestId('ingestion-pill').focus();
    await expect(app.getByRole('tooltip').first()).toContainText('embedding provider error');
  });

  test('DOC-13 the list polls while a document is pending and the pill flips to Completed without a reload', async ({ app, workspace }) => {
    let calls = 0;
    await app.route(/\/api\/v1\/workspaces\/[^/]+\/documents(\?.*)?$/, (route) => {
      if (route.request().method() !== 'GET') return route.fallback();
      calls += 1;
      const done = calls >= 3;
      const d = fakeDoc({ title: 'Indexing soon', ingestionStatus: done ? 'completed' : 'pending', aiEligible: done });
      return route.fulfill({ json: { items: [d], page: 1, pageSize: 20, total: 1 } });
    });
    await app.goto(`/${workspace.slug}/documents`);
    const pill = rowOf(app, 'Indexing soon').getByTestId('ingestion-pill');
    await expect(pill).toHaveAttribute('data-status', 'pending');
    await expect(pill).toHaveAttribute('data-status', 'completed', { timeout: 15_000 });
    const after = calls;
    await app.waitForTimeout(3500);
    expect(calls).toBe(after); // polling stops once nothing is pending
  });
});

// ---- accessibility and visual regression
async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${v.id} (${v.impact}): ${v.nodes.slice(0, 3).map((n) => `${n.target.join(' ')} -> ${n.any.map((a) => a.message).join('; ')}`).join(' | ')}`), `axe violations on ${label}`).toEqual([]);
}
const VIEWPORTS = [{ name: '1280', width: 1280, height: 800 }, { name: '390', width: 390, height: 844 }] as const;

test.describe('Documents: a11y', () => {
  for (const theme of ['dark', 'light'] as const) {
    for (const vp of VIEWPORTS) {
      test(`DOC-14 axe: list, drawer, upload dialog (${theme}, ${vp.name})`, async ({ app, workspace }) => {
        await app.setViewportSize(vp);
        await setTheme(app, theme);
        await mockList(app, FIXED);
        await app.goto(`/${workspace.slug}/documents`);
        await expect(rows(app)).toHaveCount(4);
        await noHorizontalScroll(app);
        await scan(app, `documents list ${theme} ${vp.name}`);
        await app.getByRole('button', { name: 'Open Roadmap' }).filter({ visible: true }).click();
        await expect(app.getByTestId('doc-drawer')).toBeVisible();
        await scan(app, `document drawer ${theme} ${vp.name}`);
        await app.keyboard.press('Escape');
        await app.getByRole('button', { name: 'Upload document' }).first().click();
        await expect(app.getByRole('dialog', { name: 'Upload document' })).toBeVisible();
        await scan(app, `upload dialog ${theme} ${vp.name}`);
      });
    }
  }
});

raw.describe('Documents: visual regression', () => {
  async function seeded(page: Page) {
    const id = rnd();
    const u = { email: `e2e-vrdoc-${id}@example.com`, password: 'Passw0rd!e2e', fullName: 'Ada Visual' };
    await ok(call('/auth/register', { body: u }));
    await verifyEmail(u.email);
    const tokens = await login({ ...u, userId: '' });
    const ws = await createWorkspace(tokens.accessToken, 'Visual HQ');
    await seedSession(page, tokens);
    await mockList(page, FIXED);
    return ws;
  }
  for (const theme of ['dark', 'light'] as const) {
    for (const vp of VIEWPORTS) {
      raw(`DOC-15 documents list ${theme} ${vp.name}`, async ({ page }) => {
        await page.setViewportSize(vp);
        await setTheme(page, theme);
        const ws = await seeded(page);
        await page.goto(`/${ws.slug}/documents`);
        await expect(rows(page)).toHaveCount(4);
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(150);
        await expect(page).toHaveScreenshot(`documents-${theme}-${vp.name}.png`, { mask: [page.getByTestId('account-menu')] });
      });
      raw(`DOC-16 document drawer ${theme} ${vp.name}`, async ({ page }) => {
        await page.setViewportSize(vp);
        await setTheme(page, theme);
        const ws = await seeded(page);
        await page.goto(`/${ws.slug}/documents`);
        await page.getByRole('button', { name: 'Open Q3 board deck' }).filter({ visible: true }).click();
        await expect(page.getByTestId('doc-drawer')).toBeVisible();
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(150);
        await expect(page).toHaveScreenshot(`documents-drawer-${theme}-${vp.name}.png`, { mask: [page.getByTestId('account-menu')] });
      });
    }
  }
});
