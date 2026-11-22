// Cross-component contract fixes (docs/architecture/CONTRACT_AUDIT.md): B-02 original audio for same-language
// listeners, B-03 caption speaker label, B-04 multi-sentence captions, B-09 secondary listener identity,
// live suggestions (SSE) and the AI data controls in workspace settings.
import type { Page, Route } from '@playwright/test';
import { call, createRoom, createUser, expect, inviteAndAccept, ok, signedInContext, test } from '../fixtures';
import { mountMeeting } from '../harness/helpers';

const NOW = 1_700_000_000_000;
const HARNESS = 'captions';
const AUDIO = 'audio';

async function mountCaptions(page: Page, payloads: Record<string, unknown>[], lang = 'en') {
  await page.goto('/login');
  await page.evaluate(async ({ path, i }) => {
    const m = await (async () => {
      for (let n = 0; n < 500 && !window.__wtHarness; n++) await new Promise((r) => setTimeout(r, 20));
      return window.__wtHarness![path as 'captions']();
    })();
    const el = document.createElement('div');
    document.body.append(el);
    m.mount(el, i);
  }, {
    path: HARNESS,
    i: { messages: payloads.map((p) => ({ payload: JSON.stringify(p), now: NOW })), translationLang: lang, room: { status: 'IN_PROGRESS', translationActive: true }, connection: 'connected', quality: 'good' },
  });
}

const PIPELINE = { identity: 'pipeline-agent-host1', name: 'pipeline-agent-host1' };

test.describe('Captions contract', () => {
  test('CAP-01 speakerName is the label, never the pipeline agent', async ({ page }) => {
    await mountCaptions(page, [
      { type: 'stt', segmentId: 's1', speakerId: '0190a1b2-0000-7000-8000-000000000001', speakerName: 'Ana Pereira', lang: 'vi', text: 'Xin chào', isFinal: true, translations: {}, sentenceIndex: 0, isLastSentence: true },
    ]);
    const line = page.getByTestId('caption-line');
    await expect(line).toContainText('Ana Pereira');
    await expect(line).not.toContainText('pipeline-agent');
    await expect(line).not.toContainText('0190a1b2');
  });

  test('CAP-02 multi-sentence segment: original and cumulative translation are complete, not the last sentence only', async ({ page }) => {
    const base = { type: 'translation', segmentId: 's1', speakerName: 'Ana', lang: 'vi', isFinal: true };
    await mountCaptions(page, [
      { ...base, text: 'Xin chào mọi người.', translations: { en: 'Hello everyone.' }, sentenceIndex: 0, isLastSentence: false },
      { ...base, text: 'Hôm nay ta họp.', translations: { en: 'Hello everyone. Today we meet.' }, sentenceIndex: 1, isLastSentence: false },
      { ...base, text: 'Cảm ơn.', translations: { en: 'Hello everyone. Today we meet. Thanks.' }, sentenceIndex: 2, isLastSentence: true },
    ]);
    await expect(page.getByTestId('caption-line')).toHaveCount(1);
    await expect(page.getByTestId('caption-original')).toHaveText('Xin chào mọi người. Hôm nay ta họp. Cảm ơn.');
    await expect(page.getByTestId('caption-translation')).toHaveText('Hello everyone. Today we meet. Thanks.');
  });

  test('CAP-06 current pipeline shape: cumulative text and translations replace per segment; speakerUserId resolves via roster key', async ({ page }) => {
    const base = { type: 'translation', segmentId: 's1', speakerName: 'Ana', speakerUserId: 'u1', lang: 'vi', isFinal: true, is_early: false };
    await mountCaptions(page, [
      { ...base, text: 'Một.', sentenceText: 'Một.', translations: { en: 'One.' }, sentenceTranslations: { en: 'One.' }, sentenceIndex: 0, isLastSentence: false },
      { ...base, text: 'Một. Hai.', sentenceText: 'Hai.', translations: { en: 'One. Two.' }, sentenceTranslations: { en: 'Two.' }, sentenceIndex: 1, isLastSentence: true },
    ]);
    await expect(page.getByTestId('caption-line')).toHaveCount(1);
    await expect(page.getByTestId('caption-original')).toHaveText('Một. Hai.');
    await expect(page.getByTestId('caption-translation')).toHaveText('One. Two.');
  });

  test('CAP-03 legacy per-sentence shape (chunk_idx, sentence-only translations) is appended too; late sentence 0 keeps order', async ({ page }) => {
    const base = { type: 'translation', segment_id: 's1', speaker_id: 'Ana', lang: 'vi', is_early: false };
    await mountCaptions(page, [
      { ...base, text: 'Hai.', translations: { en: 'Two.' }, chunk_idx: 1, is_final_chunk: true },
      { ...base, text: 'Một.', translations: { en: 'One.' }, chunk_idx: 0, is_final_chunk: false },
    ]);
    await expect(page.getByTestId('caption-original')).toHaveText('Một. Hai.');
    await expect(page.getByTestId('caption-translation')).toHaveText('One. Two.');
  });

  test('CAP-04 old shape without indexes still works (overwrite, snake_case, plain text)', async ({ page }) => {
    await mountCaptions(page, [
      { type: 'stt', segment_id: 's1', speaker_id: 'Ana Pereira', text: 'Xin', lang: 'vi', is_early: true, translations: {} },
      { type: 'translation', segment_id: 's1', speaker_id: 'Ana Pereira', text: 'Xin chào', lang: 'vi', is_early: false, translations: { en: 'Hello' } },
    ]);
    await expect(page.getByTestId('caption-line')).toHaveCount(1);
    await expect(page.getByTestId('caption-original')).toHaveText('Xin chào');
    await expect(page.getByTestId('caption-translation')).toHaveText('Hello');
  });

  test('CAP-05 no name in the packet: label comes from the roster by participant id, even when the sender is the pipeline agent', async ({ app, tokens, user, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Roster names', displayName: 'Host Hana' });
    await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
    const parts = await ok(call<{ id: string; userId?: string }[]>(`/translation-rooms/${room.id}/participants`, { token: tokens.accessToken }));
    const me = parts.find((p) => p.userId === user.userId)!;
    await mountMeeting(app, 'live', { roomId: room.id, selfUserId: user.userId, selfName: 'Host Hana', participants: [{ identity: user.userId, name: 'Host Hana', isLocal: true }] });
    await expect(app.getByTestId('meeting-room')).toBeVisible();
    const send = (body: unknown) => app.evaluate(([b, f]) => (window as unknown as { __meeting: { data(t: string, b: unknown, f: unknown): void } }).__meeting.data('captions', b, f), [body, PIPELINE]);
    await app.getByRole('tab', { name: /^People/ }).click();
    await expect(app.getByTestId('person').first()).toBeVisible(); // roster loaded
    await app.getByRole('tab', { name: /^Transcript/ }).click();
    await send({ segmentId: 's1', speakerId: me.id, lang: 'vi', text: 'Xin chào', isFinal: true, translations: {} });
    // by LiveKit identity (userId) and with a `:tag` suffix
    await send({ segmentId: 's2', speakerId: `${user.userId}:listener`, lang: 'vi', text: 'Chào', isFinal: true, translations: {} });
    const lines = app.getByTestId('caption-line');
    await expect(lines).toHaveCount(2);
    for (const i of [0, 1]) {
      await expect(lines.nth(i)).toContainText('Host Hana');
      await expect(lines.nth(i)).not.toContainText('pipeline-agent');
    }
    await expect(app.getByTestId('caption-overlay')).toContainText('Host Hana');
  });
});

test.describe('Audio subscription contract', () => {
  type Remote = { identity: string; pubs: { name: string; kind?: string; source?: string }[] };
  const mic = (name = 'mic') => ({ name, source: 'microphone' });
  async function run(page: Page, listen: string, remotes: Remote[], roster: Record<string, string>) {
    await page.goto('/login');
    return page.evaluate(async ({ path, a }) => {
      const m = await (async () => {
      for (let n = 0; n < 500 && !window.__wtHarness; n++) await new Promise((r) => setTimeout(r, 20));
      return window.__wtHarness![path as 'captions']();
    })();
      return m.run(a.listen, a.remotes, a.roster) as Record<string, string[]>;
    }, { path: AUDIO, a: { listen, remotes, roster } });
  }
  const room: Remote[] = [
    { identity: 'u-vi', pubs: [mic(), { name: 'dub-en' }] },
    { identity: 'u-en', pubs: [mic(), { name: 'dub-vi' }] },
    { identity: 'u-vi2', pubs: [mic('mic2')] },
  ];
  const roster = { 'u-vi': 'vi', 'u-vi2': 'vi', 'u-en': 'en' };

  test('AUD-01 a vi listener hears the vi speakers original and the en speakers dub-vi, not the en original', async ({ page }) => {
    const s = await run(page, 'vi', room, roster);
    expect(s['u-vi']).toEqual(['mic']);
    expect(s['u-vi2']).toEqual(['mic2']);
    expect(s['u-en']).toEqual(['dub-vi']);
  });

  test('AUD-02 an en listener hears the vi speakers through dub-en only; the en speakers original', async ({ page }) => {
    const s = await run(page, 'en', room, roster);
    expect(s['u-vi']).toEqual(['dub-en']);
    expect(s['u-en']).toEqual(['mic']);
    expect(s['u-vi2']).toEqual([]);
  });

  test('AUD-03 a listener in a third language hears only dubs', async ({ page }) => {
    const s = await run(page, 'ja', room, roster);
    expect(s['u-vi']).toEqual([]);
    expect(s['u-en']).toEqual([]);
  });

  test('AUD-04 "original" subscribes to every microphone and no dub', async ({ page }) => {
    const s = await run(page, 'original', room, roster);
    expect(s['u-vi']).toEqual(['mic']);
    expect(s['u-en']).toEqual(['mic']);
    expect(s['u-vi2']).toEqual(['mic2']);
  });

  test('AUD-05 identity tags are stripped for the roster lookup; secondary listener connections are never played; unknown speakers stay muted', async ({ page }) => {
    const s = await run(page, 'vi', [
      { identity: 'u-vi:bridge-listener', pubs: [mic('x')] },
      { identity: 'u-vi:other', pubs: [mic('y')] },
      { identity: 'u-ghost', pubs: [mic('z'), { name: 'cam', kind: 'video' }] },
    ], roster);
    expect(s['u-vi:bridge-listener']).toEqual([]);
    expect(s['u-vi:other']).toEqual(['y']);
    expect(s['u-ghost']).toEqual(['cam']); // video always; mic of an unknown speaker is muted
  });
});

const SUG = (o: Record<string, unknown>) => ({
  suggestionId: 'sg-1', category: 'clarification', content: 'Ask which deadline applies', detail: 'Two dates were mentioned',
  confidence: 0.8, language: 'en', severity: 'medium', ttlSeconds: 600, ...o,
});
const sse = (events: unknown[]) => events.map((e) => `event: suggestion\ndata: ${JSON.stringify(e)}\n\n`).join('');

async function meetWithRoute(app: Page, tokens: { accessToken: string }, user: { userId: string }, workspaceId: string, stream: (r: Route) => Promise<void>, followups?: (r: Route) => Promise<void>) {
  const room = await createRoom(tokens.accessToken, workspaceId, { title: 'Suggestions', displayName: 'Host Hana' });
  await ok(call(`/translation-rooms/${room.id}/start`, { token: tokens.accessToken, method: 'POST' }));
  await app.route(`**/api/v1/meetings/${room.id}/suggestions/stream*`, stream);
  await app.route(`**/api/v1/meetings/${room.id}/followups`, followups ?? ((r) => r.fulfill({ status: 404, json: { code: 'NOT_FOUND', message: 'no' } })));
  await mountMeeting(app, 'live', { roomId: room.id, selfUserId: user.userId, selfName: 'Host Hana', participants: [{ identity: user.userId, name: 'Host Hana', isLocal: true }] });
  await expect(app.getByTestId('meeting-room')).toBeVisible();
  return room;
}

test.describe('Live suggestions', () => {
  test('SUG-01 SSE stream renders suggestions with a count badge; follow-ups listed; copy works', async ({ app, tokens, user, workspace, context }) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write']);
    await meetWithRoute(app, tokens, user, workspace.id,
      (r) => r.fulfill({ status: 200, contentType: 'text/event-stream', body: `: keep-alive\n\n${sse([SUG({}), SUG({ suggestionId: 'sg-2', category: 'risk', content: 'Budget not confirmed', severity: 'high', sources: [{ title: 'Q3 plan' }] })])}` }),
      (r) => r.fulfill({ json: { items: [{ id: 'f1', content: 'Send the revised timeline', owner: 'Ana' }, 'Book a follow-up call'] } }));
    const tab = app.getByRole('tab', { name: /^Suggestions/ });
    await expect(tab).toBeVisible();
    await expect(app.getByTestId('suggestions-badge')).toHaveText(/2/);
    await tab.click();
    const items = app.getByTestId('suggestion');
    await expect(items).toHaveCount(2); // reconnect re-delivers the same ids: deduplicated
    await expect(items.nth(0)).toContainText('Ask which deadline applies');
    await expect(items.nth(1)).toContainText('Sources: Q3 plan');
    await expect(app.getByTestId('followup')).toHaveCount(2);
    await expect(app.getByTestId('followup').first()).toContainText('Send the revised timeline');
    await app.getByRole('button', { name: 'Copy follow-up: Book a follow-up call' }).click();
    expect(await app.evaluate(() => navigator.clipboard.readText())).toBe('Book a follow-up call');
  });

  test('SUG-02 dismiss removes a suggestion for good; snooze hides it', async ({ app, tokens, user, workspace }) => {
    await meetWithRoute(app, tokens, user, workspace.id,
      (r) => r.fulfill({ status: 200, contentType: 'text/event-stream', body: sse([SUG({}), SUG({ suggestionId: 'sg-2', content: 'Second one' }), SUG({ suggestionId: 'sg-3', content: 'Third one' })]) }));
    await app.getByRole('tab', { name: /^Suggestions/ }).click();
    await expect(app.getByTestId('suggestion')).toHaveCount(3);
    await app.getByRole('button', { name: 'Dismiss suggestion: Second one' }).click();
    await app.getByRole('button', { name: 'Snooze suggestion: Third one' }).click();
    await expect(app.getByTestId('suggestion')).toHaveCount(1);
    await expect(app.getByTestId('suggestion-content')).toHaveText('Ask which deadline applies');
    await app.waitForTimeout(2500); // the stream reconnects and re-delivers: dismissed/snoozed stay hidden
    await expect(app.getByTestId('suggestion')).toHaveCount(1);
  });

  test('SUG-03 404 from the stream (backend not deployed) hides the Suggestions tab entirely', async ({ app, tokens, user, workspace }) => {
    await meetWithRoute(app, tokens, user, workspace.id, (r) => r.fulfill({ status: 404, json: { code: 'NOT_FOUND', message: 'nope' } }));
    await expect(app.getByRole('tab', { name: 'Transcript' })).toBeVisible();
    await expect(app.getByRole('tab', { name: /^Suggestions/ })).toHaveCount(0);
    await expect(app.getByTestId('suggestions-tab')).toHaveCount(0);
  });

  test('SUG-04 SSE parser: split chunks, comments, multi-line data, CRLF', async ({ page }) => {
    await page.goto('/login');
    const got = await page.evaluate(async () => {
      const path = 'suggestions';
      const m = await (async () => {
      for (let n = 0; n < 500 && !window.__wtHarness; n++) await new Promise((r) => setTimeout(r, 20));
      return window.__wtHarness![path as 'captions']();
    })();
      const out: { event: string; data: string }[] = [];
      const feed = m.createSseParser((x: { event: string; data: string }) => out.push(x));
      for (const c of [': hi\n\nevent: sugg', 'estion\ndata: {"a"', ':1}\n\ndata: l1\r\ndata: l2\r\n\r\n']) feed(c);
      return out;
    });
    expect(got).toEqual([{ event: 'suggestion', data: '{"a":1}' }, { event: 'message', data: 'l1\nl2' }]);
  });
});

test.describe('AI data controls', () => {
  const base = { maxParticipantsLimit: 50, allowGuestAccess: false, defaultAiModel: null, audioRetentionDays: 30 };

  test('SET-AI-01 owner toggles the three switches; each change PUTs the full settings', async ({ app, workspace }) => {
    let state = { ...base, aiRetrievalAllowed: false, externalLlmAllowed: false, aiSuggestEnabled: true };
    const puts: Record<string, unknown>[] = [];
    await app.route(`**/api/v1/workspaces/${workspace.id}/settings`, async (r) => {
      if (r.request().method() === 'PUT') { const b = r.request().postDataJSON() as typeof state; puts.push(b); state = b; }
      await r.fulfill({ json: state });
    });
    await app.goto(`/${workspace.slug}/settings/ai`);
    const card = app.getByTestId('ai-privacy');
    const retrieval = card.getByRole('switch', { name: 'Allow AI retrieval' });
    const external = card.getByRole('switch', { name: 'Allow external LLM providers' });
    const suggest = card.getByRole('switch', { name: 'Enable AI suggestions' });
    await expect(retrieval).not.toBeChecked();
    await expect(suggest).toBeChecked();
    await retrieval.click();
    await expect(retrieval).toBeChecked();
    await external.click();
    await expect(external).toBeChecked();
    await suggest.click();
    await expect(suggest).not.toBeChecked();
    expect(puts).toHaveLength(3);
    expect(puts[2]).toMatchObject({ ...base, aiRetrievalAllowed: true, externalLlmAllowed: true, aiSuggestEnabled: false });
  });

  test('SET-AI-02 a member cannot reach the AI settings, and the API refuses their write', async ({ workspace, tokens, browser, baseURL }) => {
    const member = await createUser('member');
    await inviteAndAccept(tokens.accessToken, workspace.id, member);
    const m = await signedInContext(browser, member, baseURL);
    try {
      await m.page.goto(`/${workspace.slug}/settings/ai`);
      await expect(m.page.getByRole('heading', { name: 'Not available' })).toBeVisible();
      await expect(m.page.getByTestId('ai-privacy')).toHaveCount(0);
      const put = await call(`/workspaces/${workspace.id}/settings`, { method: 'PUT', token: m.tokens.accessToken, body: { ...base, aiRetrievalAllowed: true } });
      expect(put.status).toBe(403);
    } finally { await m.context.close(); }
  });
});
