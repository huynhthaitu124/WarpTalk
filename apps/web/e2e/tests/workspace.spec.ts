import { call, createRoom, createWorkspace, expect, test, createUser, login, ok, seedSession, rnd } from '../fixtures';
import { test as raw } from '@playwright/test';

raw.describe('Workspaces (new user)', () => {
  raw('WS-01 no workspace -> Workspaces page -> create workspace -> its Home', async ({ page }) => {
    const u = await createUser('wsnew');
    const tokens = await login(u);
    await seedSession(page, tokens);
    await page.goto('/');
    await expect(page).toHaveURL(/\/workspaces$/);
    await expect(page.getByRole('heading', { name: 'No workspaces yet' })).toBeVisible();
    await page.getByRole('button', { name: 'Create workspace' }).first().click();
    const dialog = page.getByRole('dialog', { name: 'Create workspace' });
    const name = `Studio ${rnd()}`;
    await dialog.getByLabel('Name').fill(name);
    await dialog.getByRole('button', { name: 'Create workspace' }).click();
    await expect(page.getByTestId('current-workspace')).toHaveText(name);
    await expect(page).toHaveURL(/\/[a-z0-9-]+\/home$/);
    await expect(page.getByRole('heading', { name: 'No meetings yet' })).toBeVisible();
    await expect(page.getByTestId('toast').first()).toContainText('Workspace created');
    // legacy /rooms lands in the new workspace's Meetings
    await page.goto('/rooms');
    await expect(page).toHaveURL(/\/[a-z0-9-]+\/meetings$/);
  });
});

test.describe('Workspaces', () => {
  test('WS-02 switch workspace: URL, selection and reload agree', async ({ app, tokens, workspace }) => {
    const second = await createWorkspace(tokens.accessToken, `Second ${rnd()}`);
    await app.goto(`/${workspace.slug}/meetings`);
    const current = app.getByTestId('current-workspace');
    await expect(current).toHaveText(workspace.name);
    await app.getByTestId('workspace-switcher').click();
    await app.getByRole('menuitem', { name: second.name }).click();
    await expect(current).toHaveText(second.name);
    await expect(app).toHaveURL(new RegExp(`/${second.slug}/meetings`));
    await app.reload();
    await expect(current).toHaveText(second.name);
    // a bare "/" restores the last used workspace
    await app.goto('/');
    await expect(app).toHaveURL(new RegExp(`/${second.slug}/home`));
  });

  test('WS-03 members list shows the owner', async ({ app, user, workspace }) => {
    await app.goto(`/${workspace.slug}/members`);
    const table = app.getByRole('table', { name: 'Workspace members' });
    await expect(table.getByText(user.email)).toBeVisible();
    await expect(table.locator('[data-status="OWNER"]')).toHaveText('Owner');
  });

  test('WS-04 edit workspace settings and persist', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/general`);
    await app.getByLabel('Max participants').fill('12');
    await app.getByRole('button', { name: 'Save changes' }).click();
    await expect(app.getByTestId('toast').first()).toContainText('Workspace settings saved');
    await app.reload();
    await expect(app.getByLabel('Max participants')).toHaveValue('12');
  });

  test('WS-05 API validation error is shown', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/general`);
    await app.getByLabel('Max participants').fill('0');
    await app.getByRole('button', { name: 'Save changes' }).click();
    await expect(app.getByRole('alert')).toBeVisible();
  });

  test('WS-06 default workspace is the one with meetings, and it is persisted (FN-02)', async ({ page }) => {
    const u = await createUser('wsdefault');
    const tokens = await login(u);
    await createWorkspace(tokens.accessToken, `Empty one ${rnd()}`);
    const busy = await createWorkspace(tokens.accessToken, `Busy ${rnd()}`);
    await createWorkspace(tokens.accessToken, `Empty two ${rnd()}`);
    await createRoom(tokens.accessToken, busy.id, { title: 'Only here' });
    await seedSession(page, tokens);
    await page.goto('/');
    await expect(page.getByTestId('current-workspace')).toHaveText(busy.name);
    await expect(page).toHaveURL(new RegExp(`/${busy.slug}/home`));
    expect(await page.evaluate(() => localStorage.getItem('wt.workspace'))).toBe(busy.id);
    await page.reload();
    await expect(page.getByTestId('current-workspace')).toHaveText(busy.name);
    await page.getByRole('link', { name: 'Meetings', exact: true }).first().click();
    await expect(page.getByRole('link', { name: 'Only here', exact: true })).toBeVisible();
  });

  test('WS-07 the URL slug wins over the stored choice and becomes the stored choice', async ({ app, tokens, workspace }) => {
    const other = await createWorkspace(tokens.accessToken, `Other ${rnd()}`);
    await app.goto(`/${workspace.slug}/home`);
    await expect(app.getByTestId('current-workspace')).toHaveText(workspace.name);
    await app.goto(`/${other.slug}/home`);
    await expect(app.getByTestId('current-workspace')).toHaveText(other.name);
    expect(await app.evaluate(() => localStorage.getItem('wt.workspace'))).toBe(other.id);
  });

  test('WS-08 Home: quick actions, live and recent meetings', async ({ app, tokens, workspace }) => {
    const live = await createRoom(tokens.accessToken, workspace.id, { title: 'Live on home' });
    const done = await createRoom(tokens.accessToken, workspace.id, { title: 'Done on home' });
    await ok(call(`/translation-rooms/${live.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await ok(call(`/translation-rooms/${done.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await ok(call(`/translation-rooms/${done.id}/end`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/${workspace.slug}/home`);
    await expect(app.getByRole('heading', { name: /Welcome back/, level: 1 })).toBeVisible();
    const quick = app.getByRole('region', { name: 'Quick actions' });
    await expect(quick.getByRole('link', { name: /New meeting/ })).toBeVisible();
    await expect(quick.getByRole('link', { name: /Join by code/ })).toBeVisible();
    await expect(quick.getByRole('link', { name: /Search knowledge/ })).toBeVisible();
    await expect(app.getByRole('list', { name: 'Live now' }).getByRole('link', { name: live.title, exact: true })).toBeVisible();
    await expect(app.getByRole('list', { name: 'Recent history' }).getByRole('link', { name: done.title, exact: true })).toBeVisible();
    await quick.getByRole('link', { name: /Join by code/ }).click();
    await expect(app.getByRole('dialog', { name: 'Join a meeting' })).toBeVisible();
  });
});
