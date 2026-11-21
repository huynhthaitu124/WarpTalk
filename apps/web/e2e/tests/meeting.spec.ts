import { call, createRoom, expect, ok, test } from '../fixtures';

test.describe('Meeting shell', () => {
  test('MTG-01 join shows "LiveKit not configured" with roster and disabled mic', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Standup', displayName: 'Host Hana' });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto(`/meetings/${room.id}`);
    await expect(app.getByRole('heading', { name: 'Standup', level: 1 })).toBeVisible();
    await expect(app.getByTestId('meeting-prejoin')).toBeVisible();
    await app.getByRole('button', { name: 'Join meeting' }).click();
    await expect(app.getByText('LiveKit is not configured')).toBeVisible();
    await expect(app.getByTestId('livekit-not-configured')).toContainText('LIVEKIT_URL');
    await expect(app.getByRole('button', { name: 'Unmute' })).toBeDisabled();
    await expect(app.getByTestId('captions-log')).toContainText('Captions are unavailable');
    await expect(app.getByTestId('participant-grid').getByText('Host Hana').first()).toBeVisible();
  });

  test('MTG-02 listen-language selector', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/meetings/${room.id}`);
    const sel = app.getByRole('combobox', { name: 'Listen language' });
    await sel.click();
    await expect(app.getByRole('option')).toHaveText(['Original audio', 'Dubbed: Vietnamese', 'Dubbed: English', 'Dubbed: Japanese']);
    await app.getByRole('option', { name: 'Dubbed: Japanese' }).click();
    await expect(sel).toContainText('Dubbed: Japanese');
  });

  test('MTG-03 meetings list shows live rooms only', async ({ app, tokens, workspace }) => {
    const live = await createRoom(tokens.accessToken, workspace.id, { title: 'Live one' });
    await ok(call(`/translation-rooms/${live.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const ended = await createRoom(tokens.accessToken, workspace.id, { title: 'Done one' });
    await ok(call(`/translation-rooms/${ended.id}/end`, { token: tokens.accessToken, method: 'POST' }));
    await app.goto('/meetings');
    const list = app.getByRole('list', { name: 'Live meetings' });
    await expect(list.getByText('Live one')).toBeVisible();
    await expect(list.getByText('Done one')).toHaveCount(0);
    await app.getByRole('link', { name: 'Open meeting Live one' }).click();
    await expect(app).toHaveURL(new RegExp(`/meetings/${live.id}$`));
  });

  test('MTG-04 try again returns to pre-join', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id);
    await app.goto(`/meetings/${room.id}`);
    await app.getByRole('button', { name: 'Join meeting' }).click();
    await expect(app.getByText('LiveKit is not configured')).toBeVisible();
    await app.getByRole('button', { name: 'Try again' }).click();
    await expect(app.getByTestId('meeting-prejoin')).toBeVisible();
  });
});
