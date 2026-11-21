import { addWorkspaceMember, call, createRoom, createUser, expect, signedInContext, test } from '../fixtures';

test.describe('Runtime settings', () => {
  test('SET-01 owner edits suggestion form and it persists', async ({ app }) => {
    await app.goto('/settings?tab=runtime');
    await expect(app.getByText('Inherited', { exact: true })).toBeVisible();
    await app.getByRole('switch', { name: 'Enable AI suggestions' }).click();
    await app.getByLabel('Minimum words').fill('6');
    await app.getByRole('button', { name: 'Save settings' }).click();
    await expect(app.getByTestId('toast').first()).toContainText('Settings saved');
    await app.reload();
    await expect(app.getByText(/Overridden at this scope/)).toBeVisible();
    await expect(app.getByLabel('Minimum words')).toHaveValue('6');
    await expect(app.getByRole('switch', { name: 'Enable AI suggestions' })).toBeChecked();
  });

  test('SET-02 raw JSON: local syntax error, then API validation error', async ({ app }) => {
    await app.goto('/settings?tab=runtime');
    await app.getByRole('tab', { name: 'Raw JSON' }).click();
    const editor = app.getByLabel('Settings JSON');
    await editor.fill('{ "min_words": ');
    await app.getByRole('button', { name: 'Save settings' }).click();
    await expect(app.getByRole('alert').filter({ hasText: 'Invalid JSON' })).toBeVisible();

    await editor.fill('{ "min_confidence": 5 }');
    await app.getByRole('button', { name: 'Save settings' }).click();
    const err = app.getByRole('alert').filter({ hasText: 'Rejected by the API' });
    await expect(err).toBeVisible();
    await expect(err).toContainText('min_confidence');
  });

  test('SET-03 member is read-only; API refuses their writes', async ({ app, workspace, browser, baseURL }) => {
    const memberUser = await createUser('member');
    addWorkspaceMember(workspace.id, memberUser.userId);
    const member = await signedInContext(browser, memberUser, baseURL);
    try {
      await member.page.goto('/settings?tab=runtime');
      await expect(member.page.getByTestId('current-workspace')).toHaveText(workspace.name);
      await expect(member.page.getByText('Read-only')).toBeVisible();
      await expect(member.page.getByLabel('Minimum words')).toBeDisabled();
      await expect(member.page.getByRole('button', { name: 'Save settings' })).toHaveCount(0);
      const put = await call(`/settings/suggestion?scope=workspace&id=${workspace.id}`, {
        method: 'PUT', token: member.tokens.accessToken, body: { min_words: 5 },
      });
      expect(put.status).toBe(403);
      // owner can still write
      await app.goto('/settings?tab=runtime');
      await expect(app.getByRole('button', { name: 'Save settings' })).toBeVisible();
    } finally {
      await member.context.close();
    }
  });

  test('SET-04 pipeline at room scope: save then reset', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Tuned room' });
    await app.goto('/settings?tab=runtime');
    await app.getByRole('combobox', { name: 'Namespace' }).click();
    await app.getByRole('option', { name: 'pipeline' }).click();
    await app.getByRole('combobox', { name: 'Scope' }).click();
    await app.getByRole('option', { name: 'Room' }).click();
    await expect(app.getByRole('combobox', { name: 'Room' })).toContainText(room.title);
    await app.getByLabel('Chunk duration (ms)').fill('4000');
    await app.getByRole('button', { name: 'Save settings' }).click();
    await expect(app.getByText(/Overridden at this scope/)).toBeVisible();
    await app.getByRole('button', { name: 'Reset to inherited' }).click();
    await expect(app.getByText('Inherited', { exact: true })).toBeVisible();
    await expect(app.getByLabel('Chunk duration (ms)')).toHaveValue('6000');
  });

  test('SET-05 member sees workspace settings read-only', async ({ workspace, browser, baseURL }) => {
    const memberUser = await createUser('member2');
    addWorkspaceMember(workspace.id, memberUser.userId);
    const member = await signedInContext(browser, memberUser, baseURL);
    try {
      await member.page.goto('/settings');
      await expect(member.page.getByText('Read-only')).toBeVisible();
      await expect(member.page.getByLabel('Max participants')).toBeDisabled();
      await expect(member.page.getByRole('button', { name: 'Save changes' })).toBeDisabled();
    } finally {
      await member.context.close();
    }
  });
});
