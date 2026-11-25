import { call, createRoom, createUser, expect, inviteAndAccept, joinByCode, ok, rnd, signedInContext, test } from '../fixtures';

const pill = (page: import('@playwright/test').Page, s: string) =>
  page.getByRole('heading', { level: 1 }).locator('xpath=following-sibling::div').locator(`[data-status="${s}"]`);

test.describe('Meetings (rooms)', () => {
  test('ROOM-01 create meeting via dialog', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/meetings`);
    await app.getByRole('button', { name: 'New meeting' }).first().click();
    const dialog = app.getByRole('dialog', { name: 'Create meeting' });
    const title = `Planning ${rnd()}`;
    await dialog.getByLabel('Title').fill(title);
    await dialog.getByRole('checkbox', { name: 'English' }).waitFor();
    await dialog.getByRole('checkbox', { name: 'Japanese' }).click();
    await dialog.getByRole('checkbox', { name: 'Requires approval' }).click();
    await dialog.getByRole('button', { name: 'Create meeting' }).click();
    await expect(app).toHaveURL(/\/rooms\/[0-9a-f-]{36}$/);
    await expect(app.getByRole('heading', { name: title, level: 1 })).toBeVisible();
    await expect(app.locator('[data-status="WAITING"]').first()).toBeVisible();
    await expect(app.getByText('Lobby on')).toBeVisible();
    await expect(app.getByTestId('room-code')).toHaveText(/^[a-z0-9]{12}$/);
    await expect(app.getByText(/Vietnamese → English, Japanese/)).toBeVisible();
    // IA-04: a real breadcrumb with the room title
    const crumb = app.getByRole('navigation', { name: 'Breadcrumb' });
    await expect(crumb.getByRole('link', { name: 'Meetings' })).toBeVisible();
    await expect(crumb.locator('[aria-current="page"]')).toContainText(title);
  });

  test('ROOM-02 list with status pills and status filter kept in the URL', async ({ app, tokens, workspace }) => {
    const a = await createRoom(tokens.accessToken, workspace.id, { title: 'Alpha waiting' });
    const b = await createRoom(tokens.accessToken, workspace.id, { title: 'Beta live' });
    await ok(call(`/translation-rooms/${b.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/${workspace.slug}/meetings`);
    const list = app.getByRole('list', { name: 'Meetings' });
    await expect(list.getByRole('link', { name: a.title, exact: true })).toBeVisible();
    await expect(list.getByRole('link', { name: b.title, exact: true })).toBeVisible();
    await expect(list.locator('[data-status="WAITING"]')).toHaveCount(1);
    await expect(list.locator('[data-status="IN_PROGRESS"]')).toHaveCount(1);
    await app.getByRole('combobox', { name: 'Filter by status' }).click();
    await app.getByRole('option', { name: 'In progress' }).click();
    await expect(app).toHaveURL(/status=IN_PROGRESS/);
    await expect(list.getByRole('link', { name: a.title, exact: true })).toHaveCount(0);
    await expect(list.getByRole('link', { name: b.title, exact: true })).toBeVisible();
    await app.reload();
    await expect(app.getByRole('combobox', { name: 'Filter by status' })).toContainText('In progress');
  });

  test('ROOM-03 host lifecycle start/pause/resume/stop/end', async ({ app, tokens, workspace }) => {
    const r = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/rooms/${r.id}`);
    await expect(pill(app, 'WAITING')).toBeVisible();
    // one primary per state (LY-07): Start while waiting
    await app.getByRole('button', { name: 'Start', exact: true }).click();
    await expect(pill(app, 'IN_PROGRESS')).toBeVisible();
    await app.getByRole('button', { name: 'Pause' }).click();
    await expect(pill(app, 'PAUSED')).toBeVisible();
    await app.getByRole('button', { name: 'Resume' }).click();
    await expect(pill(app, 'IN_PROGRESS')).toBeVisible();
    await app.getByRole('button', { name: 'More actions' }).click();
    await app.getByRole('menuitem', { name: 'Stop translation' }).click();
    await expect(app.getByTestId('toast').filter({ hasText: 'Translation stopped' })).toBeVisible();
    await expect(pill(app, 'IN_PROGRESS')).toBeVisible();
    // End is destructive: a menu item plus a confirmation dialog
    await app.getByRole('button', { name: 'More actions' }).click();
    await app.getByRole('menuitem', { name: 'End meeting' }).click();
    const confirm = app.getByRole('dialog', { name: 'End this meeting?' });
    await confirm.getByRole('button', { name: 'Keep meeting' }).click();
    await expect(pill(app, 'IN_PROGRESS')).toBeVisible();
    await app.getByRole('button', { name: 'More actions' }).click();
    await app.getByRole('menuitem', { name: 'End meeting' }).click();
    await confirm.getByRole('button', { name: 'End meeting' }).click();
    await expect(pill(app, 'ENDED')).toBeVisible();
    await expect(app.getByRole('group', { name: 'Room controls' })).toHaveCount(0);
    await expect(app.getByText('This meeting is ended.')).toBeVisible();
  });

  test('ROOM-04 cancel a waiting meeting (confirmed)', async ({ app, tokens, workspace }) => {
    const r = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/rooms/${r.id}`);
    await app.getByRole('button', { name: 'More actions' }).click();
    await app.getByRole('menuitem', { name: 'Cancel meeting' }).click();
    await app.getByRole('dialog', { name: 'Cancel this meeting?' }).getByRole('button', { name: 'Cancel meeting' }).click();
    await expect(app.locator('[data-status="CANCELLED"]').first()).toBeVisible();
  });

  test('ROOM-05 two-user lobby: guest waits (sees no roster or code), host admits', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { requiresApproval: true });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const guestUser = await createUser('guest');
    await inviteAndAccept(tokens.accessToken, workspace.id, guestUser);
    const guest = await signedInContext(browser, guestUser, baseURL);
    try {
      await app.goto(`/rooms/${room.id}`);
      await expect(app.getByTestId('lobby')).toHaveCount(0);

      await guest.page.goto(`/rooms/${room.id}`);
      await expect(guest.page.getByRole('heading', { name: 'Join this meeting' })).toBeVisible();
      await guest.page.getByLabel('Display name (optional)').fill('Guest Gina');
      await guest.page.getByRole('button', { name: 'Join meeting' }).click();
      await expect(guest.page.getByText('Waiting for the host', { exact: false }).first()).toBeVisible();
      await expect(guest.page.getByRole('link', { name: 'Open meeting' })).toHaveCount(0);
      // FN-08: while waiting there is neither the room code nor the roster
      await expect(guest.page.getByTestId('room-code')).toHaveCount(0);
      await expect(guest.page.getByRole('table', { name: 'Participants' })).toHaveCount(0);

      const lobby = app.getByTestId('lobby');
      await expect(lobby.getByText('Guest Gina')).toBeVisible();
      await app.getByRole('button', { name: 'Admit Guest Gina' }).click();
      await expect(lobby).toHaveCount(0);
      const table = app.getByRole('table', { name: 'Participants' });
      await expect(table.getByRole('row', { name: /Guest Gina/ })).toContainText('Connected');

      await expect(guest.page.getByText('Waiting for the host')).toHaveCount(0);
      await expect(guest.page.getByRole('link', { name: 'Open meeting' })).toBeVisible();
    } finally {
      await guest.context.close();
    }
  });

  test('ROOM-06 host denies a guest (toast says declined)', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { requiresApproval: true });
    const guestUser = await createUser('denied');
    await inviteAndAccept(tokens.accessToken, workspace.id, guestUser);
    const guest = await signedInContext(browser, guestUser, baseURL);
    try {
      await guest.page.goto(`/rooms/${room.id}`);
      await guest.page.getByLabel('Display name (optional)').fill('Denied Dan');
      await guest.page.getByRole('button', { name: 'Join meeting' }).click();
      await expect(guest.page.getByText('Waiting for the host', { exact: false }).first()).toBeVisible();
      await app.goto(`/rooms/${room.id}`);
      await app.getByRole('button', { name: 'Deny Denied Dan' }).click();
      await expect(app.getByTestId('toast').filter({ hasText: 'Request from Denied Dan declined' })).toBeVisible();
      await expect(guest.page.getByText('You were removed from this meeting')).toBeVisible();
    } finally {
      await guest.context.close();
    }
  });

  test('ROOM-07 outsider joins by code and follows the lobby; guest shell has no workspace nav', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { requiresApproval: true });
    const guestUser = await createUser('coder');
    const guest = await signedInContext(browser, guestUser, baseURL);
    try {
      await guest.page.goto('/join');
      const code = guest.page.getByLabel('Room code');
      await code.fill('short');
      await expect(guest.page.getByRole('alert')).toContainText('exactly 12 characters');
      await expect(guest.page.getByRole('button', { name: 'Join meeting' })).toBeDisabled();
      await code.fill('zzzzzzzzzzzz');
      await guest.page.getByRole('button', { name: 'Join meeting' }).click();
      await expect(guest.page.getByRole('alert').filter({ hasText: /not found/i }).first()).toBeVisible();
      await code.fill(room.roomCode);
      await guest.page.getByLabel('Display name (optional)').fill('Outsider Olga');
      await guest.page.getByRole('button', { name: 'Join meeting' }).click();
      // joining by code lands on the room page, which explains the wait
      await expect(guest.page).toHaveURL(new RegExp(`/rooms/${room.id}$`));
      await expect(guest.page.getByText('Waiting for the host', { exact: true })).toBeVisible();

      // FN-08: a person with no workspace gets a minimal shell
      const side = guest.page.getByRole('complementary', { name: 'Sidebar' });
      await expect(side.getByTestId('workspace-switcher')).toHaveCount(0);
      await expect(side.getByRole('link', { name: 'Meetings' })).toHaveCount(0);
      await expect(side.getByRole('link', { name: 'Join by code' })).toBeVisible();

      // Outsiders cannot open a room they have not joined...
      const stranger = await createUser('stranger');
      const other = await signedInContext(browser, stranger, baseURL);
      await other.page.goto(`/rooms/${room.id}`);
      await expect(other.page.getByRole('heading', { name: 'Meeting not found' })).toBeVisible();
      await other.context.close();

      // ...but the joined guest can follow it (room id comes from the shared link).
      await guest.page.goto(`/rooms/${room.id}`);
      await expect(guest.page.getByText('Waiting for the host', { exact: true })).toBeVisible();
      await app.goto(`/rooms/${room.id}`);
      await expect(app.getByTestId('lobby').getByText('Outsider Olga')).toBeVisible();
    } finally {
      await guest.context.close();
    }
  });

  test('ROOM-09 create dialog validation is explained, not silent', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/meetings?new=1`);
    const dialog = app.getByRole('dialog', { name: 'Create meeting' });
    const submit = dialog.getByRole('button', { name: 'Create meeting' });
    await submit.click();
    await expect(dialog.getByRole('alert').filter({ hasText: 'Give the meeting a title.' })).toBeVisible();
    await expect(dialog.getByLabel('Title')).toHaveAttribute('aria-invalid', 'true');
    await dialog.getByLabel('Title').fill('Has title');
    await dialog.getByRole('checkbox', { name: 'English' }).click();
    await submit.click();
    await expect(dialog.getByRole('alert').filter({ hasText: 'Add at least one target language.' })).toBeVisible();
    await expect(app).toHaveURL(/new=1/);
  });

  test('ROOM-10 filtered-empty state is not "no meetings yet" and clears', async ({ app, tokens, workspace }) => {
    await createRoom(tokens.accessToken, workspace.id, { title: 'Only waiting' });
    await app.goto(`/${workspace.slug}/meetings?status=CANCELLED`);
    await expect(app.getByRole('heading', { name: 'No cancelled meetings' })).toBeVisible();
    await expect(app.getByRole('heading', { name: 'No meetings yet' })).toHaveCount(0);
    await app.getByRole('button', { name: 'Clear filter' }).click();
    await expect(app).not.toHaveURL(/status=/);
    await expect(app.getByRole('link', { name: 'Only waiting', exact: true })).toBeVisible();
  });

  test('ROOM-11 Active / History / All tabs and search live in the URL', async ({ app, tokens, workspace }) => {
    const live = await createRoom(tokens.accessToken, workspace.id, { title: 'Sprint review live' });
    const done = await createRoom(tokens.accessToken, workspace.id, { title: 'Retro finished' });
    await ok(call(`/translation-rooms/${live.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await ok(call(`/translation-rooms/${done.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await ok(call(`/translation-rooms/${done.id}/end`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/${workspace.slug}/meetings`);
    const list = app.getByRole('list', { name: 'Meetings' });
    await expect(list.getByRole('link', { name: live.title, exact: true })).toBeVisible();
    await expect(list.getByRole('link', { name: done.title, exact: true })).toHaveCount(0);
    await app.getByRole('tab', { name: 'History' }).click();
    await expect(app).toHaveURL(/tab=history/);
    await expect(list.getByRole('link', { name: done.title, exact: true })).toBeVisible();
    await expect(list.getByRole('link', { name: live.title, exact: true })).toHaveCount(0);
    await app.getByRole('tab', { name: 'All' }).click();
    await expect(list.getByRole('link', { name: live.title, exact: true })).toBeVisible();
    await expect(list.getByRole('link', { name: done.title, exact: true })).toBeVisible();
    await app.getByRole('searchbox', { name: 'Search meetings' }).fill('retro');
    await expect(app).toHaveURL(/q=retro/);
    await expect(list.getByRole('link', { name: live.title, exact: true })).toHaveCount(0);
    await app.reload();
    await expect(app.getByRole('searchbox', { name: 'Search meetings' })).toHaveValue('retro');
    await app.getByRole('searchbox', { name: 'Search meetings' }).fill('zzz-nothing');
    await expect(app.getByRole('heading', { name: /No meetings match/ })).toBeVisible();
    // legacy URLs still land here
    await app.goto('/rooms');
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/meetings`));
  });

  test('ROOM-12 copy code: success toast only when the clipboard write worked', async ({ app, tokens, workspace, context }) => {
    const r = await createRoom(tokens.accessToken, workspace.id);
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await app.goto(`/rooms/${r.id}`);
    await app.getByRole('button', { name: `Copy room code ${r.roomCode}` }).click();
    await expect(app.getByTestId('toast').filter({ hasText: 'Room code copied' })).toBeVisible();
    expect(await app.evaluate(() => navigator.clipboard.readText())).toBe(r.roomCode);
  });

  test('ROOM-13 copy code failure is reported honestly', async ({ app, tokens, workspace }) => {
    const r = await createRoom(tokens.accessToken, workspace.id);
    await app.addInitScript(() => {
      Object.defineProperty(navigator, 'clipboard', { value: { writeText: () => Promise.reject(new Error('denied')) }, configurable: true });
    });
    await app.goto(`/rooms/${r.id}`);
    await app.getByRole('button', { name: `Copy room code ${r.roomCode}` }).click();
    await expect(app.getByTestId('toast').filter({ hasText: 'Could not copy' })).toBeVisible();
    await expect(app.getByTestId('toast').filter({ hasText: 'Room code copied' })).toHaveCount(0);
  });

  test('ROOM-14 lobby: Admit all', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { requiresApproval: true });
    const g1 = await createUser('lobby1');
    const g2 = await createUser('lobby2');
    const c1 = await signedInContext(browser, g1, baseURL);
    const c2 = await signedInContext(browser, g2, baseURL);
    try {
      await joinByCode(c1.tokens.accessToken, room.roomCode, { displayName: 'First Fay' });
      await joinByCode(c2.tokens.accessToken, room.roomCode, { displayName: 'Second Sam' });
      await app.goto(`/rooms/${room.id}`);
      await expect(app.getByTestId('lobby')).toContainText('Lobby (2)');
      await app.getByRole('button', { name: 'Admit all' }).click();
      await expect(app.getByTestId('lobby')).toHaveCount(0);
      const table = app.getByRole('table', { name: 'Participants' });
      await expect(table.getByRole('row', { name: /First Fay/ })).toContainText('Connected');
      await expect(table.getByRole('row', { name: /Second Sam/ })).toContainText('Connected');
    } finally {
      await c1.context.close();
      await c2.context.close();
    }
  });

  test('ROOM-15 a missing room fires one 404, not two (participants wait for the room)', async ({ app }) => {
    const urls: string[] = [];
    app.on('response', (r) => { if (r.status() === 404) urls.push(new URL(r.url()).pathname); });
    await app.goto('/rooms/00000000-0000-7000-8000-000000000000');
    await expect(app.getByRole('heading', { name: 'Meeting not found' })).toBeVisible();
    expect(urls.filter((u) => u.includes('/participants'))).toEqual([]);
  });
});
