// Feature flags (ADR 0001 §1): translation meetings, dubbing, voice cloning and the external bridge are hidden by default
// and come back when `GET /features` says so. `flags` is the fixture that mocks that endpoint (fixtures/features.ts).
import type { Page } from '@playwright/test';
import { FLAGS_ALL, FLAGS_DEFAULT, call, createRoom, expect, installNativeMock, mockFeatures, ok, test } from '../fixtures';

const nav = (p: Page) => p.getByRole('navigation', { name: 'Primary' });

async function expectHome(page: Page, slug: string) {
  await expect(page).toHaveURL(new RegExp(`/${slug}/home$`));
  await expect(page.getByRole('heading', { name: /Welcome back/, level: 1 })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
}

test.describe('Flags off (platform defaults)', () => {
  test.use({ flags: FLAGS_DEFAULT });

  test('FEAT-01 navigation: Copilot replaces Meetings and External meeting; Home says "copilot", not "translate"', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    await expect(nav(app).getByRole('link', { name: 'Home' })).toBeVisible();
    await expect(nav(app).getByRole('link', { name: 'Copilot' })).toHaveAttribute('href', `/${workspace.slug}/sessions`);
    await expect(nav(app).getByRole('link', { name: 'Meetings' })).toHaveCount(0);
    await expect(nav(app).getByRole('link', { name: 'External meeting' })).toHaveCount(0);
    const quick = app.getByRole('region', { name: 'Quick actions' });
    await expect(quick.getByRole('link', { name: 'Start copilot session' })).toBeVisible();
    await expect(quick.getByRole('link', { name: /New meeting|Join by code/ })).toHaveCount(0);
    await expect(app.getByRole('heading', { name: /Welcome back/ })).toBeVisible();
    await expect(app.locator('main')).toContainText('transcripts, suggestions, follow-ups and minutes');
    await expect(app).toHaveTitle('Home · WarpTalk');
  });

  test('FEAT-02 command menu and shortcuts: no meeting/external entries; C starts a copilot session', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    await app.keyboard.press('ControlOrMeta+k');
    const menu = app.getByRole('dialog', { name: 'Command menu' });
    await expect(menu.getByRole('option', { name: /Start copilot session/ })).toBeVisible();
    await expect(menu.getByRole('option', { name: /Go to Copilot sessions/ })).toBeVisible();
    await expect(menu.getByRole('option', { name: /Go to Meetings|Create meeting|Join a meeting by code|Go to External meeting/ })).toHaveCount(0);
    await app.keyboard.press('Escape');
    await app.keyboard.press('c');
    await expect(app.getByRole('dialog', { name: 'Start copilot session' })).toBeVisible();
  });

  test('FEAT-03 routes of hidden features redirect to Home without an error', async ({ app, workspace, tokens }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    for (const path of ['/rooms', '/meetings', '/join', '/external', `/${workspace.slug}/meetings`, `/rooms/${room.id}`, `/meetings/${room.id}`]) {
      await app.goto(path);
      await expectHome(app, workspace.slug);
    }
  });

  test('FEAT-04 Settings -> Account has no voice-clone section', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/account`);
    await expect(app.getByRole('region', { name: 'Appearance' })).toBeVisible();
    await expect(app.getByRole('button', { name: 'Sign out' })).toBeVisible();
    await expect(app.getByTestId('voice-section')).toHaveCount(0);
    await expect(app.getByText('Voice cloning')).toHaveCount(0);
  });

  test('FEAT-05 a missing endpoint (404) behaves like the defaults', async ({ app, workspace }) => {
    await mockFeatures(app, 'missing');
    await app.goto(`/${workspace.slug}/home`);
    await expect(nav(app).getByRole('link', { name: 'Copilot' })).toBeVisible();
    await expect(nav(app).getByRole('link', { name: 'Meetings' })).toHaveCount(0);
    await app.goto('/external');
    await expectHome(app, workspace.slug);
  });

  test('FEAT-06 flags are asked per workspace and cached', async ({ app, workspace }) => {
    const f = await mockFeatures(app, FLAGS_DEFAULT);
    await app.goto(`/${workspace.slug}/home`);
    await expect(nav(app).getByRole('link', { name: 'Copilot' })).toBeVisible();
    await expect.poll(() => f.calls().some((u) => new URL(u).searchParams.get('workspaceId') === workspace.id)).toBe(true);
    const n = f.calls().filter((u) => u.includes(workspace.id)).length;
    await nav(app).getByRole('link', { name: 'Copilot' }).click();
    await nav(app).getByRole('link', { name: 'Home' }).click();
    await expect(app.getByRole('heading', { name: /Welcome back/ })).toBeVisible();
    expect(f.calls().filter((u) => u.includes(workspace.id)).length).toBe(n);
  });

  test('FEAT-07 copilot flag off hides the Copilot entry and redirects /sessions', async ({ app, workspace }) => {
    await mockFeatures(app, { copilot: false });
    await app.goto(`/${workspace.slug}/home`);
    await expect(nav(app).getByRole('link', { name: 'Copilot' })).toHaveCount(0);
    await expect(app.getByRole('link', { name: 'Start copilot session' })).toHaveCount(0);
    await app.goto(`/${workspace.slug}/sessions`);
    await expectHome(app, workspace.slug);
  });
});

test.describe('Flags on (mocked endpoint)', () => {
  test.use({ flags: FLAGS_ALL });

  test('FEAT-10 translation meetings: nav, quick actions, list and create dialog are back', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    await expect(nav(app).getByRole('link', { name: 'Meetings' })).toBeVisible();
    await expect(nav(app).getByRole('link', { name: 'External meeting' })).toBeVisible();
    await expect(nav(app).getByRole('link', { name: 'Copilot' })).toBeVisible();
    const quick = app.getByRole('region', { name: 'Quick actions' });
    await expect(quick.getByRole('link', { name: 'New meeting' })).toBeVisible();
    await expect(quick.getByRole('link', { name: 'Join by code' })).toBeVisible();
    await app.goto(`/${workspace.slug}/meetings?new=1`);
    const d = app.getByRole('dialog', { name: 'Create meeting' });
    await expect(d.getByText('Target languages')).toBeVisible();
    await app.keyboard.press('Escape');
    await app.goto('/rooms');
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/meetings$`));
  });

  test('FEAT-11 voice clone section shows in Settings -> Account', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/account`);
    await expect(app.getByRole('region', { name: 'Voice cloning' })).toBeVisible();
  });

  test('FEAT-12 external bridge: /external opens (browser fallback) and the nav item is there', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    await nav(app).getByRole('link', { name: 'External meeting' }).click();
    await expect(app).toHaveURL(/\/external$/);
    await expect(app.getByRole('navigation', { name: 'Breadcrumb' })).toContainText('External meeting');
  });

  test('FEAT-13 flags flip live per workspace response: only externalBridge on shows only that entry', async ({ app, workspace }) => {
    await mockFeatures(app, { externalBridge: true });
    await installNativeMock(app);
    await app.goto(`/${workspace.slug}/home`);
    await expect(nav(app).getByRole('link', { name: 'External meeting' })).toBeVisible();
    await expect(nav(app).getByRole('link', { name: 'Meetings' })).toHaveCount(0);
    await app.goto('/external');
    await expect(app).toHaveURL(/\/external$/);
    await app.goto('/meetings');
    await expectHome(app, workspace.slug);
  });

  test('FEAT-14 dubbing off, translation on: no audio routes, no listen-language controls', async ({ app, workspace, tokens }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Dub-less room' });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await mockFeatures(app, { translationMeetings: true, dubbing: false });
    await app.goto(`/rooms/${room.id}`);
    await expect(app.getByRole('heading', { name: 'Dub-less room', level: 1 })).toBeVisible();
    await expect(app.getByText('Advanced: audio routes')).toHaveCount(0);
    await app.goto(`/${workspace.slug}/meetings?join=1`);
    const d = app.getByRole('dialog', { name: 'Join a meeting' });
    await expect(d.getByText('I speak')).toBeVisible();
    await expect(d.getByText('I listen in')).toHaveCount(0);
    // with dubbing on, both come back
    await mockFeatures(app, { translationMeetings: true, dubbing: true });
    await app.goto(`/rooms/${room.id}`);
    await expect(app.getByText('Advanced: audio routes')).toBeVisible();
    await app.goto(`/${workspace.slug}/meetings?join=1`);
    await expect(app.getByRole('dialog', { name: 'Join a meeting' }).getByText('I listen in')).toBeVisible();
  });
});
