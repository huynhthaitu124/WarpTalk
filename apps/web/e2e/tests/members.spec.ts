// Members: role change, removal and ownership transfer with confirm dialogs and RBAC (real backend).
import type { Page } from '@playwright/test';
import { call, createUser, expect, inviteAndAccept, login, ok, signedInContext, test, type TestUser } from '../fixtures';

const rows = (p: Page) => p.getByTestId('member-row');
const row = (p: Page, u: TestUser) => rows(p).filter({ hasText: u.email });
const role = async (token: string, wsId: string, u: TestUser) =>
  (await ok(call<{ items: { userId: string; role: string }[] }>(`/workspaces/${wsId}/members?pageSize=100`, { token }))).items.find((m) => m.userId === u.userId)?.role;

test.describe('Members management', () => {
  test('MEM-04 owner changes a role after a confirm dialog (Cancel changes nothing)', async ({ app, tokens, workspace }) => {
    const m = await createUser('mover');
    await inviteAndAccept(tokens.accessToken, workspace.id, m);
    await app.goto(`/${workspace.slug}/members`);
    await row(app, m).getByRole('button', { name: /^Actions for/ }).click();
    await app.getByRole('menuitem', { name: 'Make admin' }).click();
    const dlg = app.getByRole('dialog');
    await expect(dlg).toContainText('Admins can invite members');
    await dlg.getByRole('button', { name: 'Cancel' }).click();
    expect(await role(tokens.accessToken, workspace.id, m)).toBe('MEMBER');
    await row(app, m).getByRole('button', { name: /^Actions for/ }).click();
    await app.getByRole('menuitem', { name: 'Make admin' }).click();
    await app.getByRole('dialog').getByRole('button', { name: 'Make admin' }).click();
    await expect(row(app, m).locator('[data-status="ADMIN"]')).toBeVisible();
    expect(await role(tokens.accessToken, workspace.id, m)).toBe('ADMIN');
    await row(app, m).getByRole('button', { name: /^Actions for/ }).click();
    await expect(app.getByRole('menuitem', { name: 'Make member' })).toBeVisible();
  });

  test('MEM-05 remove member: confirm dialog, member loses access', async ({ app, tokens, workspace }) => {
    const m = await createUser('leaver');
    await inviteAndAccept(tokens.accessToken, workspace.id, m);
    await app.goto(`/${workspace.slug}/members`);
    await row(app, m).getByRole('button', { name: /^Actions for/ }).click();
    await app.getByRole('menuitem', { name: 'Remove from workspace' }).click();
    await expect(app.getByRole('dialog')).toContainText('loses access');
    await app.getByRole('dialog').getByRole('button', { name: 'Remove member' }).click();
    await expect(row(app, m)).toHaveCount(0);
    expect(await role(tokens.accessToken, workspace.id, m)).toBeUndefined();
    expect((await call(`/workspaces/${workspace.id}/members`, { token: (await login(m)).accessToken })).status).toBeGreaterThanOrEqual(403);
  });

  test('MEM-06 RBAC: admins can remove members only; members have no controls; the owner row and own row have none; API agrees', async ({ app, tokens, workspace, browser, baseURL, user }) => {
    const admin = await createUser('adm');
    const member = await createUser('mem');
    await inviteAndAccept(tokens.accessToken, workspace.id, admin, 'ADMIN');
    await inviteAndAccept(tokens.accessToken, workspace.id, member);
    await app.goto(`/${workspace.slug}/members`);
    await expect(rows(app)).toHaveCount(3);
    await expect(row(app, user).getByRole('button', { name: /^Actions for/ })).toHaveCount(0);

    const a = await signedInContext(browser, admin, baseURL);
    const m = await signedInContext(browser, member, baseURL);
    try {
      await a.page.goto(`/${workspace.slug}/members`);
      await expect(rows(a.page)).toHaveCount(3);
      await expect(row(a.page, user).getByRole('button', { name: /^Actions for/ })).toHaveCount(0);
      await row(a.page, member).getByRole('button', { name: /^Actions for/ }).click();
      await expect(a.page.getByRole('menuitem')).toHaveText(['Remove from workspace']);
      await a.page.keyboard.press('Escape');
      await m.page.goto(`/${workspace.slug}/members`);
      await expect(rows(m.page)).toHaveCount(3);
      await expect(m.page.getByRole('button', { name: /^Actions for/ })).toHaveCount(0);
      // The server enforces the same rules.
      expect((await call(`/workspaces/${workspace.id}/members/${admin.userId}/role`, { method: 'PUT', token: a.tokens.accessToken, body: { role: 'MEMBER' } })).status).toBe(403);
      expect((await call(`/workspaces/${workspace.id}/members/${user.userId}`, { method: 'DELETE', token: a.tokens.accessToken })).status).toBeGreaterThanOrEqual(400);
      expect((await call(`/workspaces/${workspace.id}/members/${admin.userId}`, { method: 'DELETE', token: m.tokens.accessToken })).status).toBe(403);
    } finally {
      await a.context.close();
      await m.context.close();
    }
  });

  test('MEM-07 transfer ownership: confirm, new owner is OWNER, previous owner becomes ADMIN and loses owner-only controls', async ({ app, tokens, workspace, user }) => {
    const m = await createUser('heir');
    await inviteAndAccept(tokens.accessToken, workspace.id, m, 'ADMIN');
    await app.goto(`/${workspace.slug}/members`);
    await row(app, m).getByRole('button', { name: /^Actions for/ }).click();
    await app.getByRole('menuitem', { name: 'Transfer ownership…' }).click();
    await expect(app.getByRole('dialog')).toContainText('You stay as an admin');
    await app.getByRole('dialog').getByRole('button', { name: 'Transfer ownership' }).click();
    await expect.poll(() => role(tokens.accessToken, workspace.id, m)).toBe('OWNER');
    expect(await role(tokens.accessToken, workspace.id, user)).toBe('ADMIN');
    await expect(row(app, m).locator('[data-status="OWNER"]')).toBeVisible();
    await expect(app.getByRole('menuitem')).toHaveCount(0);
    await expect(row(app, m).getByRole('button', { name: /^Actions for/ })).toHaveCount(0);
  });

  test('MEM-08 API errors from a confirm dialog are shown inline', async ({ app, tokens, workspace }) => {
    const m = await createUser('err');
    await inviteAndAccept(tokens.accessToken, workspace.id, m);
    await app.goto(`/${workspace.slug}/members`);
    await app.route(`**/workspaces/${workspace.id}/members/${m.userId}`, (r) => r.fulfill({ status: 409, contentType: 'application/json', body: JSON.stringify({ code: 'CONFLICT', message: 'cannot remove right now' }) }));
    await row(app, m).getByRole('button', { name: /^Actions for/ }).click();
    await app.getByRole('menuitem', { name: 'Remove from workspace' }).click();
    await app.getByRole('dialog').getByRole('button', { name: 'Remove member' }).click();
    await expect(app.getByRole('dialog').getByRole('alert')).toContainText('cannot remove right now');
    await expect(row(app, m)).toHaveCount(1);
  });
});
