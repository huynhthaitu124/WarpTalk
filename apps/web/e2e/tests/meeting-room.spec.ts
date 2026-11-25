// In-meeting UI against a scripted media session (e2e/harness/meeting.tsx): controls, side panel,
// captions merge, reactions, hand raise, host controls (real backend), states and visual snapshots.
import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { call, createRoom, createUser, expect, inviteAndAccept, joinByCode, login, ok, seedMeetingRow, signedInContext, test } from '../fixtures';
import { mountMeeting, script, sent } from '../harness/helpers';

const ANA = { identity: 'u-ana', name: 'Ana Pereira' };
const BOB = { identity: 'u-bob', name: 'Bob Lee' };

interface Ctx { roomId: string; userId: string }

/** Real room (started unless told otherwise) hosted by the signed-in user, mounted with a scripted session. */
async function meet(page: Page, tokens: { accessToken: string }, user: { userId: string }, workspaceId: string, o: { start?: boolean; approval?: boolean; title?: string; others?: typeof ANA[]; micOn?: boolean } = {}): Promise<Ctx> {
  const room = await createRoom(tokens.accessToken, workspaceId, { title: o.title ?? 'Design sync', requiresApproval: o.approval, displayName: 'Host Hana' });
  if (o.start !== false) await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
  await mountMeeting(page, 'live', {
    roomId: room.id,
    selfUserId: user.userId,
    selfName: 'Host Hana',
    participants: [{ ...{ identity: user.userId, name: 'Host Hana' }, isLocal: true, micOn: o.micOn ?? false }, ...(o.others ?? [ANA, BOB])],
  });
  await expect(page.getByTestId('meeting-room')).toBeVisible();
  return { roomId: room.id, userId: user.userId };
}
const bar = (page: Page) => page.getByRole('toolbar', { name: 'Meeting controls' });
const caption = (o: Record<string, unknown>, ago?: number) => ({ meeting_id: 'm', speaker_id: 'Ana Pereira', is_early: false, ...(ago === undefined ? {} : { emittedAtMs: Date.now() - ago }), ...o });

test.describe('In-meeting UI', () => {
  test('MTG-08 status strip shows connection, network and translation state with text labels', async ({ app, tokens, user, workspace }) => {
    const c = await meet(app, tokens, user, workspace.id, { start: false });
    await expect(app.getByTestId('status-connection')).toHaveText('Connection: Connected');
    await expect(app.getByTestId('status-quality')).toHaveText('Network: Good');
    await expect(app.getByTestId('status-translation')).toHaveText('Translation: Not started');
    await ok(call(`/translation-rooms/${c.roomId}/start`, { token: tokens.accessToken, method: 'POST' }));
    await expect(app.getByTestId('status-translation')).toHaveAttribute('data-state', /^(Active|Stopped)$/); // room polls every 3s
    await ok(call(`/translation-rooms/${c.roomId}/pause`, { token: tokens.accessToken, method: 'POST' }));
    await expect(app.getByTestId('status-translation')).toHaveText('Translation: Paused');
    await script(app, 'setQuality', 'poor');
    await expect(app.getByTestId('status-quality')).toHaveText('Network: Poor');
  });

  test('MTG-11 control bar: mic, camera, screen share and captions toggles', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id);
    const mic = bar(app).getByRole('button', { name: 'Unmute microphone' });
    await expect(mic).toHaveAttribute('aria-pressed', 'false');
    await mic.click();
    await expect(bar(app).getByRole('button', { name: 'Mute microphone' })).toHaveAttribute('aria-pressed', 'true');
    const tile = app.locator(`[data-testid="participant-tile"][data-identity="${user.userId}"]`);
    await expect(tile.getByText('Microphone on')).toBeAttached();

    await bar(app).getByRole('button', { name: 'Turn camera on' }).click();
    await expect(tile.locator('video')).toBeVisible();
    await bar(app).getByRole('button', { name: 'Turn camera off' }).click();
    await expect(tile.locator('video')).toHaveCount(0);

    await expect(app.getByTestId('participant-grid')).toHaveAttribute('data-layout', 'grid');
    await bar(app).getByRole('button', { name: 'Share screen' }).click();
    await expect(app.getByTestId('participant-grid')).toHaveAttribute('data-layout', 'spotlight');
    await expect(app.getByRole('list', { name: 'Shared screen' })).toBeVisible();
    await bar(app).getByRole('button', { name: 'Stop sharing' }).click();
    await expect(app.getByTestId('participant-grid')).toHaveAttribute('data-layout', 'grid');

    await app.evaluate(() => (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting.data('captions', { type: 'translation', segment_id: 's1', text: 'Xin chào', lang: 'vi', is_early: false, translations: { en: 'Hello' } }, { identity: 'u-ana', name: 'Ana Pereira' }));
    await expect(app.getByTestId('caption-overlay')).toContainText('Hello');
    await bar(app).getByRole('button', { name: 'Hide captions' }).click();
    await expect(app.getByTestId('caption-overlay')).toHaveCount(0);
    await bar(app).getByRole('button', { name: 'Show captions' }).click();
    await expect(app.getByTestId('caption-overlay')).toBeVisible();
  });

  test('MTG-12 participant grid: speaking indicator and names, host badge', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id);
    const grid = app.getByTestId('participant-grid');
    await expect(grid.getByTestId('participant-tile')).toHaveCount(3);
    await expect(grid.getByText('Host Hana').first()).toBeVisible();
    const ana = grid.locator('[data-identity="u-ana"]');
    await expect(ana).toHaveAttribute('data-speaking', 'false');
    await script(app, 'speak', 'u-ana', true);
    await expect(ana).toHaveAttribute('data-speaking', 'true');
    await expect(ana).toContainText('speaking');
    await expect(grid.locator('[data-identity="u-bob"]')).toHaveAttribute('data-speaking', 'false');
    await script(app, 'speak', 'u-ana', false);
    await expect(ana).toHaveAttribute('data-speaking', 'false');
    // Participants arriving and leaving update the grid.
    await script(app, 'join', { identity: 'u-cy', name: 'Cy Ng' });
    await expect(grid.getByTestId('participant-tile')).toHaveCount(4);
    await script(app, 'leave', 'u-cy');
    await expect(grid.getByTestId('participant-tile')).toHaveCount(3);
    await expect(grid.locator(`[data-identity="${user.userId}"]`)).toContainText('host');
  });

  test('MTG-13 raise hand publishes on the reactions topic and shows on tiles and in People', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id);
    await bar(app).getByRole('button', { name: 'Raise hand' }).click();
    await expect(bar(app).getByRole('button', { name: 'Lower hand' })).toHaveAttribute('aria-pressed', 'true');
    expect(await sent(app)).toContainEqual({ topic: 'reactions', payload: { t: 'hand', raised: true } });
    const mine = app.locator(`[data-testid="participant-tile"][data-identity="${user.userId}"]`);
    await expect(mine.getByTestId('hand-badge')).toBeVisible();
    // A remote hand.
    await app.evaluate(() => (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting.data('reactions', { t: 'hand', raised: true }, { identity: 'u-bob', name: 'Bob Lee' }));
    const bobTile = app.locator('[data-testid="participant-tile"][data-identity="u-bob"]');
    await expect(bobTile.getByTestId('hand-badge')).toBeVisible();
    await bar(app).getByRole('button', { name: 'Lower hand' }).click();
    await expect(mine.getByTestId('hand-badge')).toHaveCount(0);
    expect(await sent(app)).toContainEqual({ topic: 'reactions', payload: { t: 'hand', raised: false } });
    // Garbage on the topic is ignored.
    await app.evaluate(() => (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting.data('reactions', 'not json', { identity: 'u-bob', name: 'Bob Lee' }));
    await expect(bobTile.getByTestId('hand-badge')).toBeVisible();
  });

  test('MTG-14 reactions: send from the menu, see remote ones with the sender name, they fade away', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id);
    await bar(app).getByRole('button', { name: 'Reactions' }).click();
    await app.getByRole('menuitem', { name: 'Applause' }).click();
    expect(await sent(app)).toContainEqual({ topic: 'reactions', payload: { t: 'reaction', emoji: '👏' } });
    const mine = app.getByTestId('reaction').filter({ hasText: 'You' });
    await expect(mine).toContainText('👏');
    await app.evaluate(() => (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting.data('reactions', { t: 'reaction', emoji: '🎉' }, { identity: 'u-ana', name: 'Ana Pereira' }));
    await expect(app.getByTestId('reaction').filter({ hasText: 'Ana Pereira' })).toContainText('🎉');
    await expect(app.getByTestId('reaction')).toHaveCount(0, { timeout: 8000 });
  });

  test('MTG-15 chat: REST-backed send, instant data-channel copy, dedupe by messageId, unread badge', async ({ app, tokens, user, workspace }) => {
    const history = app.waitForResponse((r) => r.url().includes('/chat/messages') && r.request().method() === 'GET');
    const c = await meet(app, tokens, user, workspace.id);
    await history;
    seedMeetingRow(c.roomId, user.userId);
    // A real participant posts through the API; the data channel delivers an instant copy of the same message.
    const guest = await createUser('ana');
    const code = (await ok(call<{ roomCode: string }>(`/translation-rooms/${c.roomId}`, { token: tokens.accessToken }))).roomCode;
    await joinByCode((await login(guest)).accessToken, code, { displayName: 'Ana Pereira' });
    const posted = await ok(call<Record<string, unknown>>(`/meetings/${c.roomId}/chat/messages`, { token: (await login(guest)).accessToken, body: { content: 'Hello team', clientMessageId: 'mtg15-ana' } }));
    const ana = (o: Record<string, unknown>) => ({ type: 'chat.message', ...posted, ...o });
    const deliver = (body: unknown) => app.evaluate(([b, id]) => (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting.data('chat', b, { identity: id as string, name: 'Ana Pereira' }), [body, guest.userId]);
    // Chat arrives while the Transcript tab is open: unread badge on the tab.
    await deliver(ana({}));
    await expect(app.getByTestId('chat-unread')).toHaveText(/1/);
    await app.getByRole('tab', { name: /^Chat/ }).click();
    await expect(app.getByTestId('chat-unread')).toHaveCount(0);
    await expect(app.getByTestId('chat-message')).toHaveCount(1);
    await expect(app.getByTestId('chat-message')).toContainText('Hello team');
    await expect(app.getByTestId('chat-message')).toContainText('Ana Pereira');
    const send = app.getByRole('button', { name: 'Send message' });
    await expect(send).toBeDisabled();
    await app.getByRole('textbox', { name: 'Message' }).fill('   ');
    await expect(send).toBeDisabled();
    await app.getByRole('textbox', { name: 'Message' }).fill('Hi Ana');
    await send.click();
    await expect(app.getByTestId('chat-message').nth(1)).toHaveAttribute('data-mine', 'true');
    await expect(app.getByTestId('chat-message').nth(1)).toContainText('Hi Ana');
    await expect(app.getByTestId('chat-message').nth(1)).toHaveAttribute('data-status', 'sent');
    // Stored by the backend and mirrored on the data channel for everybody else.
    const stored = await ok(call<{ content: string; messageId: string }[]>(`/meetings/${c.roomId}/chat/messages`, { token: tokens.accessToken }));
    expect(stored.map((m) => m.content)).toEqual(['Hello team', 'Hi Ana']);
    const msgs = (await sent(app)).filter((m) => m.topic === 'chat');
    expect(msgs).toHaveLength(1);
    expect(msgs[0]!.payload).toMatchObject({ content: 'Hi Ana', messageId: stored[1]!.messageId });
    // The "local only" notice is gone: history is saved.
    await expect(app.getByText('Messages are not saved')).toHaveCount(0);
    // Enter submits too; a duplicate messageId from the wire is ignored.
    await app.getByRole('textbox', { name: 'Message' }).fill('second');
    await app.keyboard.press('Enter');
    await expect(app.getByTestId('chat-message')).toHaveCount(3);
    await deliver(ana({}));
    await expect(app.getByTestId('chat-message')).toHaveCount(3);
  });

  test('MTG-16 transcript tab: original + translation merge per segment, speaker colour, latency badge', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id);
    await app.evaluate((msgs) => {
      const m = (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting;
      for (const x of msgs) m.data('captions', x.body, x.from);
    }, [
      { body: caption({ type: 'stt', segment_id: 's1', text: 'Xin chào mọi người', lang: 'vi', translations: {} }, 300), from: ANA },
      { body: caption({ type: 'translation', segment_id: 's1', text: 'Xin chào mọi người', lang: 'vi', translations: { en: 'Hello everyone' } }, 450), from: ANA },
      { body: caption({ type: 'stt', segment_id: 's2', speaker_id: 'Bob Lee', text: 'Cảm ơn', lang: 'vi', translations: {} }), from: BOB },
    ]);
    const lines = app.getByTestId('caption-line');
    await expect(lines).toHaveCount(2);
    await expect(lines.nth(0).getByTestId('caption-original')).toHaveText('Xin chào mọi người');
    await expect(lines.nth(0).getByTestId('caption-translation')).toHaveText('Hello everyone');
    await expect(lines.nth(0).getByTestId('caption-latency')).toHaveText(/\d+ ms/);
    await expect(lines.nth(0)).toContainText('Ana Pereira');
    await expect(lines.nth(1)).toContainText('Bob Lee');
    await expect(lines.nth(1)).toContainText('Translating to English…');
    // Speaker accent differs per speaker (decorative; the name is printed too).
    const c = await lines.evaluateAll((els) => els.map((e) => getComputedStyle(e).borderLeftColor));
    expect(c[0]).not.toBe(c[1]);
    await expect(app.getByTestId('caption-overlay')).toContainText('Cảm ơn');
  });

  test('MTG-17 People tab: roster from the room API, lobby with Admit/Deny and badge (real backend)', async ({ app, tokens, user, workspace }) => {
    const c = await meet(app, tokens, user, workspace.id, { approval: true });
    const guestA = await createUser('gina');
    const guestB = await createUser('gus');
    for (const g of [guestA, guestB]) await inviteAndAccept(tokens.accessToken, workspace.id, g);
    const code = (await ok(call<{ roomCode: string }>(`/translation-rooms/${c.roomId}`, { token: tokens.accessToken }))).roomCode;
    const ga = await login(guestA);
    const gb = await login(guestB);
    expect((await joinByCode(ga.accessToken, code, { displayName: 'Gina Guest' })).status).toBe('WAITING');
    expect((await joinByCode(gb.accessToken, code, { displayName: 'Gus Guest' })).status).toBe('WAITING');

    await expect(app.getByTestId('lobby-badge')).toHaveText(/2/);
    await expect(app.getByTestId('panel-badge')).toHaveText(/2/);
    await app.getByRole('tab', { name: /^People/ }).click();
    const lobby = app.getByTestId('people-lobby');
    await expect(lobby).toContainText('Waiting to be admitted (2)');
    await expect(lobby.getByTestId('person')).toHaveCount(2);
    const inRoom = app.getByTestId('people-in-room');
    await expect(inRoom.getByTestId('person').filter({ hasText: 'Host Hana' })).toContainText('Host');
    await expect(inRoom.getByTestId('person').filter({ hasText: 'Host Hana' })).toContainText('Vietnamese');

    await app.getByRole('button', { name: 'Admit Gina Guest' }).click();
    await expect(inRoom.getByTestId('person').filter({ hasText: 'Gina Guest' })).toBeVisible();
    await app.getByRole('button', { name: 'Deny Gus Guest' }).click();
    await expect(app.getByText('Gus Guest')).toHaveCount(0);
    await expect(app.getByTestId('lobby-badge')).toHaveCount(0);
    const parts = await ok(call<{ displayName: string; status: string }[]>(`/translation-rooms/${c.roomId}/participants`, { token: tokens.accessToken }));
    expect(parts.find((p) => p.displayName === 'Gina Guest')?.status).toBe('CONNECTED');
    expect(parts.find((p) => p.displayName === 'Gus Guest')?.status).toBe('KICKED');
  });

  test('MTG-18 host controls drive the room API: start, pause, resume, stop translation', async ({ app, tokens, user, workspace }) => {
    const c = await meet(app, tokens, user, workspace.id, { start: false });
    const status = async () => (await ok(call<{ status: string; translationActive: boolean }>(`/translation-rooms/${c.roomId}`, { token: tokens.accessToken })));
    const run = async (item: string) => {
      await bar(app).getByRole('button', { name: 'Host controls' }).click();
      await app.getByRole('menuitem', { name: item }).click();
    };
    await run('Start translation');
    await expect.poll(async () => (await status()).status).toBe('IN_PROGRESS');
    await expect(app.getByText('Translation started')).toBeVisible();
    await run('Pause translation');
    await expect.poll(async () => (await status()).status).toBe('PAUSED');
    await expect(app.getByTestId('status-translation')).toHaveText('Translation: Paused');
    await run('Resume translation');
    await expect.poll(async () => (await status()).status).toBe('IN_PROGRESS');
    await expect(app.getByTestId('status-translation')).toHaveAttribute('data-state', 'Active');
    await run('Stop translation');
    await expect.poll(async () => (await status()).translationActive).toBe(false);
    await expect(app.getByTestId('status-translation')).toHaveText('Translation: Stopped');
  });

  test('MTG-19 host exit menu: leave (with host note) and end for all; members get a plain Leave', async ({ app, tokens, user, workspace, browser, baseURL }) => {
    const c = await meet(app, tokens, user, workspace.id);
    await bar(app).getByRole('button', { name: 'Leave or end meeting' }).click();
    await app.getByRole('menuitem', { name: 'Leave meeting' }).click();
    const leave = app.getByRole('dialog', { name: 'Leave meeting?' });
    await expect(leave).toContainText('The meeting will continue without an active host');
    await leave.getByRole('button', { name: 'Cancel' }).click();
    await expect(leave).toHaveCount(0);

    await bar(app).getByRole('button', { name: 'Leave or end meeting' }).click();
    await app.getByRole('menuitem', { name: 'End meeting for all' }).click();
    const end = app.getByRole('dialog', { name: 'End meeting for everyone?' });
    await expect(end).toBeVisible();
    await end.getByRole('button', { name: 'Cancel' }).click();
    expect((await ok(call<{ status: string }>(`/translation-rooms/${c.roomId}`, { token: tokens.accessToken }))).status).toBe('IN_PROGRESS');
    await bar(app).getByRole('button', { name: 'Leave or end meeting' }).click();
    await app.getByRole('menuitem', { name: 'End meeting for all' }).click();
    await app.getByRole('dialog').getByRole('button', { name: 'End meeting' }).click();
    await expect(app.locator('body')).toHaveAttribute('data-meeting-ended', 'true');
    expect((await ok(call<{ status: string }>(`/translation-rooms/${c.roomId}`, { token: tokens.accessToken }))).status).toBe('ENDED');

    // A plain member has no host menu, no end option and leaves immediately.
    const room2 = await createRoom(tokens.accessToken, workspace.id, { title: 'Second' });
    await ok(call(`/translation-rooms/${room2.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const member = await createUser('member');
    await inviteAndAccept(tokens.accessToken, workspace.id, member);
    const m = await signedInContext(browser, member, baseURL);
    try {
      await mountMeeting(m.page, 'live', { roomId: room2.id, selfUserId: member.userId, selfName: 'Mem', participants: [{ identity: member.userId, name: 'Mem', isLocal: true }, ANA] });
      const mbar = m.page.getByRole('toolbar', { name: 'Meeting controls' });
      await expect(mbar.getByRole('button', { name: 'Host controls' })).toHaveCount(0);
      await expect(mbar.getByRole('button', { name: 'Leave or end meeting' })).toHaveCount(0);
      await mbar.getByRole('button', { name: 'Leave meeting' }).click();
      await expect(m.page.locator('body')).toHaveAttribute('data-meeting-left', 'true');
    } finally {
      await m.context.close();
    }
  });

  test('MTG-20 reconnecting banner and listen-language menu', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id);
    await expect(app.getByTestId('reconnecting-banner')).toHaveCount(0);
    await script(app, 'setConnection', 'reconnecting');
    await expect(app.getByTestId('reconnecting-banner')).toContainText('Reconnecting');
    await expect(app.getByTestId('status-connection')).toHaveText('Connection: Reconnecting');
    await script(app, 'setConnection', 'connected');
    await expect(app.getByTestId('reconnecting-banner')).toHaveCount(0);

    await bar(app).getByRole('button', { name: 'Listen language' }).click();
    await expect(app.getByRole('menuitem')).toHaveText(['Original audio', 'Dubbed: Vietnamese', 'Dubbed: English (current)', 'Dubbed: Japanese']);
    await app.keyboard.press('Escape');
  });

  test('MTG-21 side panel is always shown on wide screens; ended state offers a way back', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id);
    const panel = app.getByTestId('side-panel');
    await expect(panel).toBeVisible();
    await expect(bar(app).getByRole('button', { name: /side panel/ })).toHaveCount(0);
    await expect(panel.getByRole('tab')).toHaveText([/Transcript/, /Chat/, /People/, ...(await panel.getByRole('tab', { name: /Suggestions/ }).count() ? [/Suggestions/] : [])]); // Suggestions only when the backend serves the stream
  });

  test('MTG-22 a11y: no serious violations in the live meeting (dark and light)', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id, { others: [ANA, BOB] });
    await script(app, 'speak', 'u-ana', true);
    await app.evaluate(() => (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting.data('captions', { type: 'translation', segment_id: 's1', text: 'Xin chào', lang: 'vi', translations: { en: 'Hello' } }, { identity: 'u-ana', name: 'Ana Pereira' }));
    for (const theme of ['dark', 'light']) {
      await app.evaluate((t) => document.documentElement.setAttribute('data-theme', t), theme);
      for (const tab of [/^Transcript/, /^People/, /^Chat/]) {
        await app.getByRole('tab', { name: tab }).click();
        const res = await new AxeBuilder({ page: app }).include('[data-testid="meeting-room"]').withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
        const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
        expect(bad.map((v) => `${v.id}: ${v.nodes.slice(0, 2).map((n) => n.target.join(' ')).join(' | ')}`), `${theme} ${String(tab)}`).toEqual([]);
      }
    }
  });
});

test.describe('In-meeting UI: states', () => {
  test('MTG-23 ended view', async ({ app }) => {
    await mountMeeting(app, 'ended', { roomId: 'x', selfUserId: 'x', selfName: 'x', participants: [] });
    await expect(app.getByTestId('meeting-ended')).toContainText('This meeting has ended');
    await expect(app.getByRole('button', { name: 'Back to room details' })).toBeVisible();
  });
});

test.describe('In-meeting UI: layout 390', () => {
  test.use({ viewport: { width: 390, height: 844 } });
  test('MTG-24 mobile: compact two-column tiles, controls reachable, panel is a bottom sheet and no horizontal scroll', async ({ app, tokens, user, workspace }) => {
    await meet(app, tokens, user, workspace.id, { others: [ANA, BOB, { identity: 'u-cy', name: 'Cy Ng' }] });
    const tiles = app.getByTestId('participant-tile');
    await expect(tiles).toHaveCount(4);
    const [a, b] = [await tiles.nth(0).boundingBox(), await tiles.nth(1).boundingBox()];
    expect(a!.y).toBe(b!.y); // two columns
    expect(a!.height).toBeLessThan(140);
    // The side panel starts closed on phones; the stage and bar do not overlap.
    await expect(app.getByTestId('side-panel')).toBeHidden();
    const stage = (await app.getByTestId('stage').boundingBox())!;
    const barBox = (await bar(app).boundingBox())!;
    expect(stage.y + stage.height).toBeLessThanOrEqual(barBox.y + 1);
    expect(barBox.y + barBox.height).toBeLessThanOrEqual(844);
    expect(await app.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    await app.getByRole('button', { name: 'Open transcript' }).click();
    const panel = (await app.getByTestId('side-panel').boundingBox())!;
    expect(panel.y).toBeGreaterThan(150);
    await expect(app.getByRole('button', { name: 'Close panel' })).toBeVisible();
    await app.getByRole('button', { name: 'Close panel' }).click();
    await expect(app.getByTestId('side-panel')).toBeHidden();
  });
});

test.describe('In-meeting UI: visual', () => {
  const sizes = [{ name: '1280', width: 1280, height: 800 }, { name: '390', width: 390, height: 844 }];
  for (const size of sizes) {
    for (const theme of ['dark', 'light'] as const) {
      test(`MTG-25 in-meeting ${size.name} (${theme})`, async ({ app, tokens, user, workspace }) => {
        await app.setViewportSize({ width: size.width, height: size.height });
        await app.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
        await meet(app, tokens, user, workspace.id, { others: [ANA, BOB, { identity: 'u-cy', name: 'Cy Ng' }], micOn: true, title: 'Design sync' });
        await script(app, 'speak', 'u-ana', true);
        await script(app, 'setMic', 'u-ana', true);
        await app.evaluate(() => {
          const m = (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting;
          m.data('reactions', { t: 'hand', raised: true }, { identity: 'u-bob', name: 'Bob Lee' });
          m.data('captions', { type: 'translation', segment_id: 's1', speaker_id: 'Ana Pereira', text: 'Xin chào mọi người', lang: 'vi', is_early: false, translations: { en: 'Hello everyone' } }, { identity: 'u-ana', name: 'Ana Pereira' });
          m.data('captions', { type: 'translation', segment_id: 's2', speaker_id: 'Bob Lee', text: 'Chúng ta bắt đầu nhé', lang: 'vi', is_early: false, translations: { en: "Let's get started" } }, { identity: 'u-bob', name: 'Bob Lee' });
        });
        await expect(app.getByTestId('caption-overlay')).toContainText("Let's get started");
        await app.evaluate(() => document.fonts.ready);
        await app.waitForTimeout(200);
        await expect(app).toHaveScreenshot(`meeting-live-${size.name}-${theme}.png`, { mask: [app.getByTestId('status-quality')] });
      });
    }
  }
});
