// Meeting chat: REST is the source of truth (idempotent post, paged history, host hide, translations),
// the LiveKit "chat" topic only speeds delivery up. The e2e backend has no LiveKit, so a meeting row is seeded
// (fixtures/db.ts `seedMeetingRow`) and the panel UI runs in the meeting harness (scripted media session) against the real API.
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import {
  call, createRoom, createUser, expect, inviteAndAccept, joinByCode, login, ok, seedChatTranslation, seedMeetingRow, signedInContext, test, type Tokens,
} from '../fixtures';
import { mountMeeting } from '../harness/helpers';

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
/** Mount the meeting UI for the host and open the Chat tab once the first history request has been answered. */
async function openChat(page: Page, c: Ctx, user: { userId: string }, selfName = 'Host Hana', o: { listen?: string } = {}) {
  const first = page.waitForResponse((r) => r.url().includes('/chat/messages') && r.request().method() === 'GET');
  await mountMeeting(page, 'live', { roomId: c.roomId, selfUserId: user.userId, selfName, listen: o.listen, participants: [{ identity: user.userId, name: selfName, isLocal: true }] });
  await first;
  await page.getByRole('tab', { name: /^Chat/ }).click();
}
const bubbles = (page: Page) => page.getByTestId('chat-message');
const deliver = (page: Page, body: unknown, from: { identity: string; name: string }) =>
  page.evaluate(([b, f]) => (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting.data('chat', b, f), [body, from] as const);

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

test.describe('Meeting chat panel', () => {
  test('CHAT-04 history loads on open and older pages load on demand (52 messages)', async ({ app, tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    for (let i = 1; i <= 52; i++) await post(tokens.accessToken, c.roomId, `msg ${String(i).padStart(2, '0')}`, `bulk-${i}`);
    await openChat(app, c, user);
    await expect(bubbles(app)).toHaveCount(50);
    await expect(bubbles(app).first()).toContainText('msg 03');
    await expect(bubbles(app).last()).toContainText('msg 52');
    await app.getByRole('button', { name: 'Load older messages' }).click();
    await expect(bubbles(app)).toHaveCount(52);
    await expect(bubbles(app).first()).toContainText('msg 01');
    await expect(app.getByRole('button', { name: 'Load older messages' })).toHaveCount(0);
  });

  test('CHAT-05 sending is optimistic: the bubble shows at once as "Sending…" and settles to sent', async ({ app, tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    await openChat(app, c, user);
    await app.route('**/meetings/*/chat/messages', async (route) => {
      if (route.request().method() === 'POST') await new Promise((r) => setTimeout(r, 900));
      await route.continue();
    });
    await app.getByRole('textbox', { name: 'Message' }).fill('optimistic hello');
    await app.getByRole('button', { name: 'Send message' }).click();
    await expect(bubbles(app)).toHaveCount(1);
    await expect(bubbles(app).first()).toHaveAttribute('data-status', 'sending');
    await expect(bubbles(app).first()).toContainText('Sending…');
    await expect(app.getByRole('textbox', { name: 'Message' })).toHaveValue('');
    await expect(bubbles(app).first()).toHaveAttribute('data-status', 'sent');
    await expect(bubbles(app)).toHaveCount(1);
    expect((await history(tokens.accessToken, c.roomId)).map((m) => m.content)).toEqual(['optimistic hello']);
  });

  test('CHAT-06 a lost response is retried with the same clientMessageId: stored exactly once', async ({ app, tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    await openChat(app, c, user);
    const seen: string[] = [];
    let dropped = false;
    await app.route('**/meetings/*/chat/messages', async (route) => {
      const req = route.request();
      if (req.method() !== 'POST') return route.continue();
      seen.push((req.postDataJSON() as { clientMessageId: string }).clientMessageId);
      if (!dropped) {
        dropped = true;
        await route.fetch(); // the server stores it...
        return route.abort('connectionreset'); // ...but the browser never sees the answer
      }
      return route.continue();
    });
    await app.getByRole('textbox', { name: 'Message' }).fill('once only');
    await app.getByRole('button', { name: 'Send message' }).click();
    await expect(bubbles(app).first()).toHaveAttribute('data-status', 'sent');
    expect(seen).toHaveLength(2);
    expect(seen[1]).toBe(seen[0]);
    expect((await history(tokens.accessToken, c.roomId)).map((m) => m.content)).toEqual(['once only']);
    await expect(bubbles(app)).toHaveCount(1);
  });

  test('CHAT-07 a failed send shows Not sent with Retry and Discard; Retry delivers once the server is back', async ({ app, tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    await openChat(app, c, user);
    let down = true;
    await app.route('**/meetings/*/chat/messages', (route) => (down && route.request().method() === 'POST'
      ? route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ code: 'UNAVAILABLE', message: 'try later' }) })
      : route.continue()));
    await app.getByRole('textbox', { name: 'Message' }).fill('flaky');
    await app.getByRole('button', { name: 'Send message' }).click();
    await expect(bubbles(app).first()).toHaveAttribute('data-status', 'failed');
    await expect(app.getByRole('alert').filter({ hasText: 'Not sent' })).toContainText('try later');
    down = false;
    await app.getByRole('button', { name: 'Retry sending message' }).click();
    await expect(bubbles(app).first()).toHaveAttribute('data-status', 'sent');
    expect((await history(tokens.accessToken, c.roomId)).map((m) => m.content)).toEqual(['flaky']);
    // Discard drops an unsent message without sending it.
    down = true;
    await app.getByRole('textbox', { name: 'Message' }).fill('never mind');
    await app.getByRole('button', { name: 'Send message' }).click();
    await app.getByRole('button', { name: 'Discard unsent message' }).click();
    await expect(bubbles(app)).toHaveCount(1);
  });

  test('CHAT-08 translations appear under the message in the viewer listen language and follow a language switch (REST poll)', async ({ app, tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    const g = await guestIn(c, 'trans');
    const m = (await post(g.token, c.roomId, 'Xin chào mọi người', 't-1')).body;
    await openChat(app, c, user, 'Host Hana', { listen: 'en' });
    await expect(bubbles(app)).toHaveCount(1);
    await expect(app.getByTestId('chat-translation')).toHaveCount(0);
    seedChatTranslation(m.messageId, 'en', 'Hello everyone');
    seedChatTranslation(m.messageId, 'ja', 'みなさん、こんにちは');
    // No push event: the panel polls every 5 s while it is open.
    await expect(app.getByTestId('chat-translation')).toHaveText(/Hello everyone/, { timeout: 12_000 });
    await expect(app.getByTestId('chat-translation')).toHaveAttribute('lang', 'en');
    await expect(app.getByText('みなさん、こんにちは')).toHaveCount(0);
    await app.getByRole('button', { name: 'Listen language' }).click();
    await app.getByRole('menuitem', { name: /Dubbed: Japanese/ }).click();
    await expect(app.getByTestId('chat-translation')).toHaveText(/みなさん、こんにちは/);
  });

  test('CHAT-09 chat.translated on the data channel refetches the message at once', async ({ app, tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    const g = await guestIn(c, 'push');
    const m = (await post(g.token, c.roomId, 'Cảm ơn', 'tp-1')).body;
    await openChat(app, c, user, 'Host Hana', { listen: 'en' });
    await expect(bubbles(app)).toHaveCount(1);
    seedChatTranslation(m.messageId, 'en', 'Thank you');
    const refetched = app.waitForResponse((r) => r.url().includes('/chat/messages') && r.request().method() === 'GET', { timeout: 3000 });
    await deliver(app, { type: 'chat.translated', messageId: m.messageId, translations: { en: 'Thank you' } }, { identity: g.u.userId, name: g.name });
    await refetched;
    await expect(app.getByTestId('chat-translation')).toHaveText(/Thank you/, { timeout: 3000 });
  });

  test('CHAT-10 REST-only messages raise the unread badge; a data-channel copy of a stored message is not shown twice', async ({ app, tokens, user, workspace }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    const g = await guestIn(c, 'dup');
    await openChat(app, c, user);
    await app.getByRole('tab', { name: /^Transcript/ }).click();
    const m = (await post(g.token, c.roomId, 'via REST only', 'd-1')).body;
    await expect(app.getByTestId('chat-unread')).toHaveText(/1/, { timeout: 12_000 });
    await app.getByRole('tab', { name: /^Chat/ }).click();
    await expect(bubbles(app)).toHaveCount(1);
    await deliver(app, { type: 'chat.message', ...m }, { identity: g.u.userId, name: g.name });
    await expect(bubbles(app)).toHaveCount(1);
    // A copy claiming another sender than the LiveKit identity is ignored.
    await deliver(app, { type: 'chat.message', ...m, messageId: '0197d3a0-0000-7000-8000-00000000beef', content: 'spoofed' }, { identity: 'someone-else', name: 'Mallory' });
    await expect(app.getByText('spoofed')).toHaveCount(0);
  });

  test('CHAT-11 host can hide a message; members never see a Hide control or the hidden text', async ({ app, tokens, user, workspace, browser, baseURL }) => {
    const c = await startedRoom(tokens, user, workspace.id);
    const member = await createUser('member');
    await inviteAndAccept(tokens.accessToken, workspace.id, member);
    const mt = await login(member);
    await joinByCode(mt.accessToken, c.roomCode, { displayName: 'Mem Ber' });
    await post(mt.accessToken, c.roomId, 'borderline remark', 'hide-1');
    await post(tokens.accessToken, c.roomId, 'welcome', 'hide-2');
    await openChat(app, c, user);
    await expect(bubbles(app)).toHaveCount(2);
    await app.getByRole('button', { name: 'Hide message from Mem Ber' }).click();
    await expect(bubbles(app).first()).toHaveAttribute('data-hidden', 'true');
    await expect(bubbles(app).first()).toContainText('Hidden from participants');
    expect((await history(tokens.accessToken, c.roomId)).find((m) => m.content === 'borderline remark')?.hidden).toBe(true);

    const m = await signedInContext(browser, member, baseURL);
    try {
      await openChat(m.page, c, member, 'Mem Ber');
      await expect(bubbles(m.page)).toHaveCount(1);
      await expect(m.page.getByText('borderline remark')).toHaveCount(0);
      await expect(m.page.getByRole('button', { name: /^Hide message/ })).toHaveCount(0);
    } finally {
      await m.context.close();
    }
  });

  for (const theme of ['dark', 'light'] as const) {
    test(`CHAT-12 a11y: chat tab with history, a translation and a hidden message (${theme})`, async ({ app, tokens, user, workspace }) => {
      await app.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
      const c = await startedRoom(tokens, user, workspace.id);
      const g = await guestIn(c, 'axe');
      const m = (await post(g.token, c.roomId, 'Xin chào', 'a-1')).body;
      const hid = (await post(g.token, c.roomId, 'hide me', 'a-2')).body;
      await post(tokens.accessToken, c.roomId, 'Welcome all', 'a-3');
      await ok(call(`${base(c.roomId)}/${hid.messageId}/hide`, { token: tokens.accessToken, method: 'POST', body: {} }));
      seedChatTranslation(m.messageId, 'en', 'Hello');
      await openChat(app, c, user, 'Host Hana', { listen: 'en' });
      await expect(app.getByTestId('chat-translation')).toHaveText(/Hello/, { timeout: 12_000 });
      await expect(bubbles(app)).toHaveCount(3);
      const res = await new AxeBuilder({ page: app }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
      expect(res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical').map((v) => `${v.id}: ${v.nodes[0]?.target.join(' ')}`)).toEqual([]);
    });
  }
});
