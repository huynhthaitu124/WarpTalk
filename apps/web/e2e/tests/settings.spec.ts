import { call, createRoom, createUser, expect, inviteAndAccept, signedInContext, test } from '../fixtures';

test.describe('Runtime settings', () => {
  test('SET-01 owner edits suggestion form and it persists', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/advanced`);
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

  test('SET-02 raw JSON (dev builds): local syntax error, then API validation error', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/advanced`);
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

  test('SET-03 member has no Advanced section; the API refuses their writes', async ({ app, workspace, tokens, browser, baseURL }) => {
    const memberUser = await createUser('member');
    await inviteAndAccept(tokens.accessToken, workspace.id, memberUser);
    const member = await signedInContext(browser, memberUser, baseURL);
    try {
      await member.page.goto(`/${workspace.slug}/settings/advanced`);
      await expect(member.page.getByTestId('current-workspace')).toHaveText(workspace.name);
      await expect(member.page.getByRole('heading', { name: 'Not available' })).toBeVisible();
      const nav = member.page.getByRole('navigation', { name: 'Settings sections' });
      await expect(nav.getByRole('link', { name: 'Advanced' })).toHaveCount(0);
      await expect(nav.getByRole('link', { name: 'AI providers' })).toHaveCount(0);
      const put = await call(`/settings/suggestion?scope=workspace&id=${workspace.id}`, {
        method: 'PUT', token: member.tokens.accessToken, body: { min_words: 5 },
      });
      expect(put.status).toBe(403);
      // owner can still write
      await app.goto(`/${workspace.slug}/settings/advanced`);
      await expect(app.getByRole('button', { name: 'Save settings' })).toBeVisible();
    } finally {
      await member.context.close();
    }
  });

  test('SET-04 pipeline at room scope: save then reset', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Tuned room' });
    await app.goto(`/${workspace.slug}/settings/advanced`);
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

  test('SET-05 member sees workspace settings read-only', async ({ workspace, tokens, browser, baseURL }) => {
    const memberUser = await createUser('member2');
    await inviteAndAccept(tokens.accessToken, workspace.id, memberUser);
    const member = await signedInContext(browser, memberUser, baseURL);
    try {
      await member.page.goto(`/${workspace.slug}/settings/general`);
      await expect(member.page.getByText('Read-only')).toBeVisible();
      await expect(member.page.getByLabel('Max participants')).toBeDisabled();
      await expect(member.page.getByRole('button', { name: 'Save changes' })).toBeDisabled();
    } finally {
      await member.context.close();
    }
  });

  test('SET-06 old ?tab= links and bare /settings forward to the right section', async ({ app, workspace }) => {
    await app.goto('/settings?tab=runtime');
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/settings/advanced`));
    await app.goto('/settings?tab=members');
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/members`));
    await app.goto(`/${workspace.slug}/settings`);
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/settings/general`));
    const crumb = app.getByRole('navigation', { name: 'Breadcrumb' });
    await expect(crumb).toContainText('Settings');
    await expect(crumb).toContainText('General');
  });

  test('SET-07 settings sections: owner sees all, workspace page shows description not the slug', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/general`);
    const nav = app.getByRole('navigation', { name: 'Settings sections' });
    for (const n of ['Account', 'General', 'AI providers', 'Advanced']) await expect(nav.getByRole('link', { name: n })).toBeVisible();
    await expect(app.getByText(`Slug: ${workspace.slug}`)).toHaveCount(0);
    await nav.getByRole('link', { name: 'Account' }).click();
    await expect(app.getByRole('heading', { name: 'Appearance' })).toBeVisible();
  });
});

test.describe('AI providers (bring your own)', () => {
  const emptyConfig = { byoEnabled: true, purposes: {}, credentials: [] };

  test('AI-01 owner sees provider keys and per-task models; BYO switched off on this deployment is explained', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/ai`);
    await expect(app.getByRole('heading', { name: 'Provider keys' })).toBeVisible();
    await expect(app.getByRole('heading', { name: 'What uses which model' })).toBeVisible();
    for (const p of ['OpenAI', 'Anthropic', 'Google Gemini', 'OpenAI-compatible endpoint']) await expect(app.getByText(p, { exact: true }).first()).toBeVisible();
    // the e2e backend has no AI_KEYS_KEK
    await expect(app.getByText('Bring-your-own AI is switched off')).toBeVisible();
  });

  test('AI-02 keys are write-only: only the last 4 are shown, the key never appears in the DOM', async ({ app, workspace }) => {
    const KEY = 'sk-test-SECRET-KEY-VALUE-1234';
    let saved = false;
    await app.route(`**/api/v1/ai/workspaces/${workspace.id}/config`, (r) => r.fulfill({
      json: saved
        ? { ...emptyConfig, credentials: [{ provider: 'openai', last4: '1234', createdAt: '2026-01-01T00:00:00Z' }] }
        : emptyConfig,
    }));
    await app.route(`**/api/v1/ai/workspaces/${workspace.id}/credentials/openai`, (r) => {
      if (r.request().method() === 'PUT') {
        expect(r.request().postDataJSON()).toMatchObject({ apiKey: KEY });
        saved = true;
        return r.fulfill({ json: { provider: 'openai', last4: '1234', createdAt: '2026-01-01T00:00:00Z' } });
      }
      saved = false;
      return r.fulfill({ status: 204, body: '' });
    });
    await app.route(`**/api/v1/ai/workspaces/${workspace.id}/credentials/openai/test`, (r) => r.fulfill({ json: { ok: true, latencyMs: 87, errorCode: null } }));
    await app.goto(`/${workspace.slug}/settings/ai`);
    const row = app.getByTestId('provider-openai');
    await row.getByLabel('OpenAI API key').fill(KEY);
    await expect(row.getByLabel('OpenAI API key')).toHaveAttribute('type', 'password');
    await row.getByRole('button', { name: 'Save key for OpenAI' }).click();
    await expect(row.getByText('Key ending 1234')).toBeVisible();
    await expect(app.locator('body')).not.toContainText(KEY);
    expect(await app.evaluate(() => document.documentElement.outerHTML.includes('SECRET-KEY-VALUE'))).toBe(false);
    await row.getByRole('button', { name: 'Test key for OpenAI' }).click();
    await expect(row.getByText('Works (87 ms)')).toBeVisible();
    await row.getByRole('button', { name: 'Remove key for OpenAI' }).click();
    await row.getByRole('button', { name: 'Confirm remove key for OpenAI' }).click();
    await expect(row.getByText('No key')).toBeVisible();
  });

  test('AI-03 RBAC: a plain member has no AI providers section and the API says 403', async ({ workspace, tokens, browser, baseURL }) => {
    const memberUser = await createUser('aimember');
    await inviteAndAccept(tokens.accessToken, workspace.id, memberUser);
    const member = await signedInContext(browser, memberUser, baseURL);
    try {
      await member.page.goto(`/${workspace.slug}/settings/ai`);
      await expect(member.page.getByRole('heading', { name: 'Not available' })).toBeVisible();
      const r = await call(`/ai/workspaces/${workspace.id}/config`, { token: member.tokens.accessToken });
      expect(r.status).toBe(403);
    } finally {
      await member.context.close();
    }
  });

  test('AI-04 a 403 from the API renders the owners-and-admins state', async ({ app, workspace }) => {
    await app.route(`**/api/v1/ai/workspaces/${workspace.id}/config`, (r) => r.fulfill({ status: 403, json: { code: 'FORBIDDEN', message: 'workspace owner or admin role required' } }));
    await app.goto(`/${workspace.slug}/settings/ai`);
    await expect(app.getByRole('heading', { name: 'Owners and admins only' })).toBeVisible();
  });

  test('AI-05 per-task config: choosing a provider without a key is explained, with a key it is sent', async ({ app, workspace }) => {
    let put: unknown = null;
    await app.route(`**/api/v1/ai/workspaces/${workspace.id}/config`, (r) => {
      if (r.request().method() === 'PUT') { put = r.request().postDataJSON(); return r.fulfill({ json: emptyConfig }); }
      return r.fulfill({ json: { ...emptyConfig, credentials: [{ provider: 'anthropic', last4: 'abcd', createdAt: '2026-01-01T00:00:00Z' }] } });
    });
    await app.goto(`/${workspace.slug}/settings/ai`);
    const summary = app.getByTestId('purpose-summary');
    await summary.getByRole('combobox', { name: 'Meeting summaries provider' }).click();
    await app.getByRole('option', { name: 'OpenAI', exact: true }).click();
    await summary.getByLabel('Model').fill('gpt-4o-mini');
    await app.getByRole('button', { name: 'Save configuration' }).click();
    await expect(app.getByText('add a OpenAI key first')).toBeVisible();
    await summary.getByRole('combobox', { name: 'Meeting summaries provider' }).click();
    await app.getByRole('option', { name: 'Anthropic' }).click();
    await summary.getByLabel('Model').fill('claude-sonnet');
    await app.getByRole('button', { name: 'Save configuration' }).click();
    await expect(app.getByTestId('toast').filter({ hasText: 'AI configuration saved' })).toBeVisible();
    expect(put).toMatchObject({ purposes: { summary: { provider: 'anthropic', model: 'claude-sonnet', enabled: true }, assistant: null } });
  });
});
