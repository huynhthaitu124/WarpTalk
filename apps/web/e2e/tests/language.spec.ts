// Mid-meeting language changes: PUT /translation-rooms/{id}/participants/me/languages, then the dub-{lang} subscription.
import { call, createRoom, expect, ok, test, type Tokens } from '../fixtures';

interface P { userId: string | null; speakLanguage: string; listenLanguage: string }
const me = async (tokens: Tokens, roomId: string, userId: string) =>
  (await ok(call<P[]>(`/translation-rooms/${roomId}/participants`, { token: tokens.accessToken }))).find((p) => p.userId === userId)!;
const put = (tokens: Tokens, roomId: string, body: unknown) =>
  call<P>(`/translation-rooms/${roomId}/participants/me/languages`, { token: tokens.accessToken, method: 'PUT', body });

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
});
