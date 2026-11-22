// Mid-meeting language changes: PUT /translation-rooms/{id}/participants/me/languages, then the dub-{lang} subscription.
import type { Page } from '@playwright/test';
import { call, createRoom, expect, ok, test, type Tokens } from '../fixtures';
import { mountMeeting } from '../harness/helpers';

interface P { userId: string | null; speakLanguage: string; listenLanguage: string }
const me = async (tokens: Tokens, roomId: string, userId: string) =>
  (await ok(call<P[]>(`/translation-rooms/${roomId}/participants`, { token: tokens.accessToken }))).find((p) => p.userId === userId)!;
const put = (tokens: Tokens, roomId: string, body: unknown) =>
  call<P>(`/translation-rooms/${roomId}/participants/me/languages`, { token: tokens.accessToken, method: 'PUT', body });

async function live(page: Page, tokens: Tokens, user: { userId: string }, workspaceId: string) {
  const room = await createRoom(tokens.accessToken, workspaceId, { title: 'Lang room', displayName: 'Host Hana' });
  await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
  await mountMeeting(page, 'live', { roomId: room.id, selfUserId: user.userId, selfName: 'Host Hana', listen: 'en', participants: [{ identity: user.userId, name: 'Host Hana', isLocal: true }] });
  await expect(page.getByTestId('meeting-room')).toBeVisible();
  return room.id;
}
const bar = (page: Page) => page.getByRole('toolbar', { name: 'Meeting controls' });

test.describe('Language switch', () => {
  test('LANG-01 API: listen/speak update, listen-only, validation', async ({ tokens, user, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { displayName: 'Host Hana' });
    const a = await put(tokens, room.id, { listenLanguage: 'en' });
    expect(a.status).toBe(200);
    expect(a.body.listenLanguage).toBe('en');
    const b = await put(tokens, room.id, { speakLanguage: 'none' });
    expect(b.status).toBe(200);
    expect(b.body.speakLanguage).toBe('none');
    expect((await me(tokens, room.id, user.userId)).speakLanguage).toBe('none');
    expect((await put(tokens, room.id, { listenLanguage: 'none' })).status).toBeGreaterThanOrEqual(400);
    expect((await put(tokens, room.id, {})).status).toBe(400);
    expect((await put(tokens, room.id, { listenLanguage: 'x y' })).status).toBe(400);
  });

  test('LANG-02 listen menu records the language, then switches the subscription; Original is local only', async ({ app, tokens, user, workspace }) => {
    const roomId = await live(app, tokens, user, workspace.id);
    await bar(app).getByRole('button', { name: 'Listen language' }).click();
    await app.getByRole('menuitem', { name: /Dubbed: Vietnamese/ }).click();
    await expect(app.locator('body')).toHaveAttribute('data-listen', 'vi');
    expect((await me(tokens, roomId, user.userId)).listenLanguage).toBe('vi');
    await bar(app).getByRole('button', { name: 'Listen language' }).click();
    await expect(app.getByRole('menuitem', { name: /Dubbed: Vietnamese/ })).toContainText('(current)');
    await app.getByRole('menuitem', { name: 'Original audio' }).click();
    await expect(app.locator('body')).toHaveAttribute('data-listen', 'original');
    expect((await me(tokens, roomId, user.userId)).listenLanguage).toBe('vi'); // no server code for "original"
  });

  test('LANG-03 a failed change keeps the current subscription and says so', async ({ app, tokens, user, workspace }) => {
    const roomId = await live(app, tokens, user, workspace.id);
    await app.route('**/participants/me/languages', (r) => r.fulfill({ status: 500, contentType: 'application/json', body: JSON.stringify({ code: 'INTERNAL', message: 'boom' }) }));
    await bar(app).getByRole('button', { name: 'Listen language' }).click();
    await app.getByRole('menuitem', { name: /Dubbed: Japanese/ }).click();
    await expect(app.getByText('Could not change the listen language')).toBeVisible();
    await expect(app.locator('body')).not.toHaveAttribute('data-listen', 'ja');
    expect((await me(tokens, roomId, user.userId)).listenLanguage).not.toBe('ja');
  });

  test('LANG-04 settings menu: speak language and listen-only (none)', async ({ app, tokens, user, workspace }) => {
    const roomId = await live(app, tokens, user, workspace.id);
    await bar(app).getByRole('button', { name: 'Meeting settings' }).click();
    await expect(app.getByRole('menuitem')).toHaveText(['Vietnamese (current)', 'English', 'Japanese', 'Listen only (I do not speak)']);
    await app.getByRole('menuitem', { name: 'English' }).click();
    await expect.poll(async () => (await me(tokens, roomId, user.userId)).speakLanguage).toBe('en');
    await bar(app).getByRole('button', { name: 'Meeting settings' }).click();
    await app.getByRole('menuitem', { name: /Listen only/ }).click();
    await expect.poll(async () => (await me(tokens, roomId, user.userId)).speakLanguage).toBe('none');
    await bar(app).getByRole('button', { name: 'Meeting settings' }).click();
    await expect(app.getByRole('menuitem', { name: /Listen only/ })).toContainText('(current)');
  });
});
