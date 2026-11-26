// Cross-component contract fixes (docs/architecture/CONTRACT_AUDIT.md): B-02 original audio for same-language
// listeners, B-03 caption speaker label, B-04 multi-sentence captions, B-09 secondary listener identity,
// live suggestions (SSE) and the AI data controls in workspace settings.
import type { Page } from '@playwright/test';
import { call, createUser, expect, inviteAndAccept, signedInContext, test } from '../fixtures';

const NOW = 1_700_000_000_000;
const HARNESS = 'captions';

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
});

test.describe('Live suggestions', () => {
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
