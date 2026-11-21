import { createWorkspace, expect, test, createUser, login, seedSession, rnd } from '../fixtures';
import { test as raw } from '@playwright/test';

raw.describe('Workspaces (new user)', () => {
  raw('WS-01 empty state -> create workspace', async ({ page }) => {
    const u = await createUser('wsnew');
    const tokens = await login(u);
    await seedSession(page, tokens);
    await page.goto('/rooms');
    await expect(page.getByRole('heading', { name: 'Create a workspace first' })).toBeVisible();
    await page.getByRole('button', { name: 'Create workspace' }).click();
    const dialog = page.getByRole('dialog', { name: 'Create workspace' });
    const name = `Studio ${rnd()}`;
    await dialog.getByLabel('Name').fill(name);
    await dialog.getByRole('button', { name: 'Create workspace' }).click();
    await expect(page.getByTestId('current-workspace')).toHaveText(name);
    await expect(page.getByRole('heading', { name: 'No rooms yet' })).toBeVisible();
    await expect(page.getByTestId('toast').first()).toContainText('Workspace created');
  });
});

test.describe('Workspaces', () => {
  test('WS-02 switch workspace and persist selection', async ({ app, tokens, workspace }) => {
    const second = await createWorkspace(tokens.accessToken, `Second ${rnd()}`);
    await app.goto('/rooms');
    const current = app.getByTestId('current-workspace');
    await expect(current).toBeVisible();
    await app.getByTestId('workspace-switcher').click();
    const shown = (await current.textContent())!.trim();
    const other = shown === workspace.name ? second : workspace;
    await app.getByRole('menuitem', { name: other.name }).click();
    await expect(current).toHaveText(other.name);
    await app.reload();
    await expect(current).toHaveText(other.name);
  });

  test('WS-03 members list shows the owner', async ({ app, user }) => {
    await app.goto('/settings?tab=members');
    const table = app.getByRole('table', { name: 'Workspace members' });
    await expect(table.getByText(user.email)).toBeVisible();
    await expect(table.locator('[data-status="OWNER"]')).toHaveText('Owner');
  });

  test('WS-04 edit workspace settings and persist', async ({ app }) => {
    await app.goto('/settings');
    await app.getByLabel('Max participants').fill('12');
    await app.getByRole('button', { name: 'Save changes' }).click();
    await expect(app.getByTestId('toast').first()).toContainText('Workspace settings saved');
    await app.reload();
    await expect(app.getByLabel('Max participants')).toHaveValue('12');
  });

  test('WS-05 API validation error is shown', async ({ app }) => {
    await app.goto('/settings');
    await app.getByLabel('Max participants').fill('0');
    await app.getByRole('button', { name: 'Save changes' }).click();
    await expect(app.getByRole('alert')).toBeVisible();
  });
});
