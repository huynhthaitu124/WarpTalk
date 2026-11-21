import { addWorkspaceMember, call, createRoom, createUser, expect, joinByCode, ok, rnd, signedInContext, test } from '../fixtures';

test.describe('Rooms', () => {
  test('ROOM-01 create room via dialog', async ({ app }) => {
    await app.goto('/rooms');
    await app.getByRole('button', { name: 'Create room' }).first().click();
    const dialog = app.getByRole('dialog', { name: 'Create room' });
    const title = `Planning ${rnd()}`;
    await dialog.getByLabel('Title').fill(title);
    await dialog.getByRole('checkbox', { name: 'English' }).waitFor();
    await dialog.getByRole('checkbox', { name: 'Japanese' }).click();
    await dialog.getByRole('checkbox', { name: 'Requires approval' }).click();
    await dialog.getByRole('button', { name: 'Create room' }).click();
    await expect(app).toHaveURL(/\/rooms\/[0-9a-f-]{36}$/);
    await expect(app.getByRole('heading', { name: title, level: 1 })).toBeVisible();
    await expect(app.locator('[data-status="WAITING"]').first()).toBeVisible();
    await expect(app.getByText('Lobby on')).toBeVisible();
    await expect(app.getByTestId('room-code')).toHaveText(/^[a-z0-9]{12}$/);
    await expect(app.getByText(/Vietnamese → English, Japanese/)).toBeVisible();
  });

  test('ROOM-02 list with status pills and filter', async ({ app, tokens, workspace }) => {
    const a = await createRoom(tokens.accessToken, workspace.id, { title: 'Alpha waiting' });
    const b = await createRoom(tokens.accessToken, workspace.id, { title: 'Beta live' });
    await ok(call(`/translation-rooms/${b.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto('/rooms');
    const list = app.getByRole('list', { name: 'Rooms' });
    await expect(list.getByRole('link', { name: a.title })).toBeVisible();
    await expect(list.getByRole('link', { name: b.title })).toBeVisible();
    await expect(list.locator('[data-status="WAITING"]')).toHaveCount(1);
    await expect(list.locator('[data-status="IN_PROGRESS"]')).toHaveCount(1);
    await app.getByRole('combobox', { name: 'Filter by status' }).click();
    await app.getByRole('option', { name: 'In progress' }).click();
    await expect(list.getByRole('link', { name: a.title })).toHaveCount(0);
    await expect(list.getByRole('link', { name: b.title })).toBeVisible();
  });

  test('ROOM-03 host lifecycle start/pause/resume/stop/end', async ({ app, tokens, workspace }) => {
    const r = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/rooms/${r.id}`);
    const pill = (s: string) => app.getByRole('heading', { level: 1 }).locator('xpath=following-sibling::p').locator(`[data-status="${s}"]`);
    await expect(pill('WAITING')).toBeVisible();
    await app.getByRole('button', { name: 'Start' }).click();
    await expect(pill('IN_PROGRESS')).toBeVisible();
    await app.getByRole('button', { name: 'Pause' }).click();
    await expect(pill('PAUSED')).toBeVisible();
    await app.getByRole('button', { name: 'Resume' }).click();
    await expect(pill('IN_PROGRESS')).toBeVisible();
    await app.getByRole('button', { name: 'Stop translation' }).click();
    await expect(app.getByTestId('toast').filter({ hasText: 'Translation stopped' })).toBeVisible();
    await expect(pill('IN_PROGRESS')).toBeVisible();
    await app.getByRole('button', { name: 'End', exact: true }).click();
    await expect(pill('ENDED')).toBeVisible();
    await expect(app.getByRole('group', { name: 'Room controls' })).toHaveCount(0);
    await expect(app.getByText('This room is ended.')).toBeVisible();
  });

  test('ROOM-04 cancel a waiting room', async ({ app, tokens, workspace }) => {
    const r = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/rooms/${r.id}`);
    await app.getByRole('button', { name: 'Cancel' }).click();
    await expect(app.locator('[data-status="CANCELLED"]').first()).toBeVisible();
  });

  test('ROOM-05 two-user lobby: guest waits, host admits', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { requiresApproval: true });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const guestUser = await createUser('guest');
    addWorkspaceMember(workspace.id, guestUser.userId);
    const guest = await signedInContext(browser, guestUser, baseURL);
    try {
      await app.goto(`/rooms/${room.id}`);
      await expect(app.getByTestId('lobby')).toHaveCount(0);

      await guest.page.goto(`/rooms/${room.id}`);
      await expect(guest.page.getByRole('heading', { name: 'Join this room' })).toBeVisible();
      await guest.page.getByLabel('Display name (optional)').fill('Guest Gina');
      await guest.page.getByRole('button', { name: 'Join room' }).click();
      await expect(guest.page.getByText('Waiting for the host', { exact: false }).first()).toBeVisible();
      await expect(guest.page.getByRole('link', { name: 'Open meeting' })).toHaveCount(0);

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

  test('ROOM-06 host denies a guest', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { requiresApproval: true });
    const guestUser = await createUser('denied');
    addWorkspaceMember(workspace.id, guestUser.userId);
    const guest = await signedInContext(browser, guestUser, baseURL);
    try {
      await guest.page.goto(`/rooms/${room.id}`);
      await guest.page.getByLabel('Display name (optional)').fill('Denied Dan');
      await guest.page.getByRole('button', { name: 'Join room' }).click();
      await expect(guest.page.getByText('Waiting for the host', { exact: false }).first()).toBeVisible();
      await app.goto(`/rooms/${room.id}`);
      await app.getByRole('button', { name: 'Deny Denied Dan' }).click();
      await expect(guest.page.getByText('You were removed from this room')).toBeVisible();
    } finally {
      await guest.context.close();
    }
  });

  test('ROOM-07 outsider joins by code and follows the lobby on the room page', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { requiresApproval: true });
    const guestUser = await createUser('coder');
    const guest = await signedInContext(browser, guestUser, baseURL);
    try {
      await guest.page.goto('/join');
      const code = guest.page.getByLabel('Room code');
      await code.fill('short');
      await expect(guest.page.getByRole('alert')).toContainText('exactly 12 characters');
      await expect(guest.page.getByRole('button', { name: 'Join room' })).toBeDisabled();
      await code.fill('zzzzzzzzzzzz');
      await guest.page.getByRole('button', { name: 'Join room' }).click();
      await expect(guest.page.getByRole('alert').filter({ hasText: /not found/i }).first()).toBeVisible();
      await code.fill(room.roomCode);
      await guest.page.getByLabel('Display name (optional)').fill('Outsider Olga');
      await guest.page.getByRole('button', { name: 'Join room' }).click();
      await expect(guest.page.getByTestId('join-result')).toContainText('Waiting for the host to admit you');

      // Outsiders cannot open a room they have not joined...
      const stranger = await createUser('stranger');
      const other = await signedInContext(browser, stranger, baseURL);
      await other.page.goto(`/rooms/${room.id}`);
      await expect(other.page.getByRole('heading', { name: 'Room not found' })).toBeVisible();
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

  test('ROOM-08 generate audio routes', async ({ app, tokens, workspace, browser, baseURL }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    const guestUser = await createUser('router');
    const guest = await signedInContext(browser, guestUser, baseURL);
    try {
      await joinByCode(guest.tokens.accessToken, room.roomCode);
      await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
      await app.goto(`/rooms/${room.id}`);
      const card = app.getByRole('heading', { name: 'Audio routes' });
      await expect(card).toBeVisible();
      await app.getByRole('button', { name: 'Generate routes' }).click();
      await expect(app.getByRole('table', { name: 'Audio routes' }).or(app.getByText(/No routes yet/))).toBeVisible();
      await expect(app.getByTestId('toast').filter({ hasText: 'Action failed' })).toHaveCount(0);
    } finally {
      await guest.context.close();
    }
  });

  test('ROOM-09 create dialog validation', async ({ app }) => {
    await app.goto('/rooms?new=1');
    const dialog = app.getByRole('dialog', { name: 'Create room' });
    const submit = dialog.getByRole('button', { name: 'Create room' });
    await expect(submit).toBeDisabled();
    await dialog.getByLabel('Title').fill('Has title');
    await expect(submit).toBeEnabled();
    await dialog.getByRole('checkbox', { name: 'English' }).click();
    await expect(submit).toBeDisabled();
  });
});
