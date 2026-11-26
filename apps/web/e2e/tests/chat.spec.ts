// Meeting chat: REST is the source of truth (idempotent post, paged history, host hide, translations),
// the LiveKit "chat" topic only speeds delivery up. The e2e backend has no LiveKit, so a meeting row is seeded
// (fixtures/db.ts `seedMeetingRow`) and the panel UI runs in the meeting harness (scripted media session) against the real API.
import {
  call, createRoom, createUser, expect, joinByCode, login, ok, seedMeetingRow, test, type Tokens,
} from '../fixtures';

interface Msg { messageId: string; senderId: string | null; senderName: string; content: string; createdAt: string; hidden: boolean; translations: Record<string, string> }
const base = (roomId: string) => `/meetings/${roomId}/chat/messages`;
const post = (token: string, roomId: string, content: string, clientMessageId: string) => call<Msg>(base(roomId), { token, body: { content, clientMessageId } });
const history = (token: string, roomId: string, q = '') => ok(call<Msg[]>(`${base(roomId)}${q}`, { token }));

interface Ctx { roomId: string; roomCode: string }
/** Started room hosted by the signed-in user, with the meeting row seeded (what a LiveKit join would create). */
async function startedRoom(tokens: Tokens, user: { userId: string }, workspaceId: string, o: { meetingRow?: boolean } = {}): Promise<Ctx> {
  const room = await createRoom(tokens.accessToken, workspaceId, { title: 'Chat room', displayName: 'Host Hana' });
  await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
  if (o.meetingRow !== false) seedMeetingRow(room.id, user.userId);
  return { roomId: room.id, roomCode: room.roomCode };
}
async function guestIn(c: Ctx, name: string) {
  const u = await createUser(name);
  const t = await login(u);
  await joinByCode(t.accessToken, c.roomCode, { displayName: `${name} Guest` });
  return { u, token: t.accessToken, name: `${name} Guest` };
}
test.describe('Meeting chat API', () => {
  test('CHAT-01 post is idempotent on clientMessageId; validation, access and state rules', async ({ tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    const a = await post(tokens.accessToken, c.roomId, '  hello  ', 'cid-1');
    expect(a.status).toBe(201);
    expect(a.body).toMatchObject({ content: 'hello', senderId: user.userId, hidden: false, translations: {} });
    const again = await post(tokens.accessToken, c.roomId, 'hello', 'cid-1');
    expect(again.status).toBe(200);
    expect(again.body.messageId).toBe(a.body.messageId);
    expect(await history(tokens.accessToken, c.roomId)).toHaveLength(1);

    for (const [content, cid] of [['   ', 'cid-2'], ['x'.repeat(1001), 'cid-3'], ['ok', ''], ['ok', 'c'.repeat(101)]] as const) {
      const r = await post(tokens.accessToken, c.roomId, content, cid);
      expect(r.status, `${content.slice(0, 8)}/${cid.slice(0, 4)}`).toBe(400);
    }
    // Somebody who never joined may not read or write; a room whose meeting has not started has no history.
    const outsider = await login(await createUser('outsider'));
    expect((await post(outsider.accessToken, c.roomId, 'hi', 'o-1')).status).toBe(403);
    expect((await call(base(c.roomId), { token: outsider.accessToken })).status).toBe(403);
    const early = await startedRoom(tokens, user, workspace.id, { meetingRow: false });
    expect((await post(tokens.accessToken, early.roomId, 'hi', 'e-1')).status).toBe(404);
    expect(await history(tokens.accessToken, early.roomId)).toEqual([]);
    // Ended room: chat is closed.
    await ok(call(`/translation-rooms/${c.roomId}/end`, { token: tokens.accessToken, method: 'POST' }));
    expect((await post(tokens.accessToken, c.roomId, 'late', 'cid-9')).status).toBe(409);
  });

  test('CHAT-02 history pages oldest-first with limit and before', async ({ tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    const ids: string[] = [];
    for (let i = 1; i <= 7; i++) ids.push((await post(tokens.accessToken, c.roomId, `m${i}`, `p-${i}`)).body.messageId);
    const last3 = await history(tokens.accessToken, c.roomId, '?limit=3');
    expect(last3.map((m) => m.content)).toEqual(['m5', 'm6', 'm7']);
    const mid = await history(tokens.accessToken, c.roomId, `?limit=3&before=${last3[0]!.messageId}`);
    expect(mid.map((m) => m.content)).toEqual(['m2', 'm3', 'm4']);
    const first = await history(tokens.accessToken, c.roomId, `?limit=3&before=${mid[0]!.messageId}`);
    expect(first.map((m) => m.content)).toEqual(['m1']);
    expect(await history(tokens.accessToken, c.roomId, `?before=${first[0]!.messageId}`)).toEqual([]);
    expect(ids).toHaveLength(7);
  });

  test('CHAT-03 hide is host-only, idempotent, and hidden messages are visible to the host only (flagged)', async ({ tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    const g = await guestIn(c, 'hider');
    const m = (await post(g.token, c.roomId, 'rude', 'h-1')).body;
    expect((await post(tokens.accessToken, c.roomId, 'fine', 'h-2')).status).toBe(201);
    expect((await call(`${base(c.roomId)}/${m.messageId}/hide`, { token: g.token, method: 'POST', body: {} })).status).toBe(403);
    for (let i = 0; i < 2; i++) {
      const r = await call<Msg>(`${base(c.roomId)}/${m.messageId}/hide`, { token: tokens.accessToken, method: 'POST', body: { reason: 'spam' } });
      expect(r.status).toBe(200);
      expect(r.body.hidden).toBe(true);
    }
    expect((await history(tokens.accessToken, c.roomId)).map((x) => [x.content, x.hidden])).toEqual([['rude', true], ['fine', false]]);
    expect((await history(g.token, c.roomId)).map((x) => x.content)).toEqual(['fine']);
  });
});
