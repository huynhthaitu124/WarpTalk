// Voice cloning (Settings > Account): consent, enrollment from a WAV upload, error mapping, revoke (real backend).
// The server may or may not have a voice provider: success and 503 VOICE_CLONING_UNAVAILABLE are both valid for a good sample.
import AxeBuilder from '@axe-core/playwright';
import { createHash } from 'node:crypto';
import type { Page } from '@playwright/test';
import { test as raw } from '@playwright/test';
import { call, createUser, createWorkspace, expect, login, makeWav, ok, rnd, seedSession, test, verifyEmail } from '../fixtures';

interface VoiceStatus { consent: { granted: boolean; consentVersion?: string | null }; profile?: { qualityScore?: number | null } | null; cloneEnabled: boolean }
const status = (token: string) => ok(call<VoiceStatus>('/me/voice', { token }));
const grantViaApi = (token: string) =>
  ok(call('/me/voice/consent', { token, body: { granted: true, consentVersion: '2026-11', acknowledgementHash: 'a'.repeat(64) } }));
const wavFile = (name: string, buffer: Buffer) => ({ name, mimeType: 'audio/wav', buffer });
const section = (p: Page) => p.getByTestId('voice-section');

async function setTheme(page: Page, theme: 'dark' | 'light') {
  await page.addInitScript((t) => { try { localStorage.setItem('wt.theme', t); } catch { /* ignore */ } }, theme);
}
async function scan(page: Page, label: string) {
  const res = await new AxeBuilder({ page }).withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa']).analyze();
  const bad = res.violations.filter((v) => v.impact === 'serious' || v.impact === 'critical');
  expect(bad.map((v) => `${v.id} (${v.impact}): ${v.nodes.slice(0, 3).map((n) => `${n.target.join(' ')} -> ${n.any.map((a) => a.message).join('; ')}`).join(' | ')}`), `axe violations on ${label}`).toEqual([]);
}
const VIEWPORTS = [{ name: '1280', width: 1280, height: 800 }, { name: '390', width: 390, height: 844 }] as const;

test.describe('Voice cloning', () => {
  test('VOICE-01 consent: the checkbox gates "I consent"; the hash is the SHA-256 of the displayed text; status shows the version and date', async ({ app, tokens, workspace }) => {
    await app.goto(`/${workspace.slug}/settings/account`);
    await expect(section(app).getByRole('heading', { name: 'Voice cloning' })).toBeVisible();
    await expect(app.getByTestId('voice-consent-state')).toHaveAttribute('data-granted', 'false');
    await expect(section(app).getByRole('button', { name: 'Record sample' })).toHaveCount(0);
    const consent = section(app).getByRole('button', { name: 'I consent' });
    await expect(consent).toBeDisabled();
    await expect(app.getByTestId('consent-text')).toContainText('version 2026-11');
    await expect(app.getByTestId('consent-text')).toContainText('Revoking: I can revoke at any time');
    const paragraphs = await app.getByTestId('consent-text').locator('p').allInnerTexts();
    const shown = paragraphs.slice(1).join('\n\n'); // first <p> is the version label
    await section(app).getByRole('checkbox', { name: /I have read the text above/ }).click();
    await expect(consent).toBeEnabled();
    const [req] = await Promise.all([
      app.waitForRequest((r) => r.url().endsWith('/me/voice/consent') && r.method() === 'POST'),
      consent.click(),
    ]);
    expect(req.postDataJSON()).toEqual({ granted: true, consentVersion: '2026-11', acknowledgementHash: createHash('sha256').update(shown).digest('hex') });
    await expect(app.getByTestId('voice-consent-state')).toHaveAttribute('data-granted', 'true');
    await expect(app.getByTestId('voice-granted-at')).toContainText('version 2026-11');
    await expect(app.getByTestId('voice-quality')).toHaveText('No sample yet');
    await expect(app.getByTestId('voice-clone-state')).toHaveAttribute('data-enabled', 'false');
    const st = await status(tokens.accessToken);
    expect(st.consent).toMatchObject({ granted: true, consentVersion: '2026-11' });
    await app.reload();
    await expect(app.getByTestId('voice-consent-state')).toHaveAttribute('data-granted', 'true');
  });

  test('VOICE-02 revoke asks for confirmation (Cancel keeps consent), then returns to the consent card', async ({ app, tokens, workspace }) => {
    await grantViaApi(tokens.accessToken);
    await app.goto(`/${workspace.slug}/settings/account`);
    await section(app).getByRole('button', { name: 'Revoke consent' }).click();
    const dlg = app.getByRole('dialog', { name: 'Revoke voice cloning consent?' });
    await expect(dlg).toContainText('stored voice sample and profile are deleted');
    await dlg.getByRole('button', { name: 'Cancel' }).click();
    expect((await status(tokens.accessToken)).consent.granted).toBe(true);
    await section(app).getByRole('button', { name: 'Revoke consent' }).click();
    await app.getByRole('dialog').getByRole('button', { name: 'Revoke and delete' }).click();
    await expect(app.getByTestId('voice-consent-state')).toHaveAttribute('data-granted', 'false');
    await expect(section(app).getByRole('button', { name: 'I consent' })).toBeDisabled();
    expect((await status(tokens.accessToken)).consent.granted).toBe(false);
  });

  test('VOICE-03 a silent WAV is refused with the friendly "too quiet" message', async ({ app, tokens, workspace }) => {
    await grantViaApi(tokens.accessToken);
    await app.goto(`/${workspace.slug}/settings/account`);
    await app.getByTestId('voice-file-input').setInputFiles(wavFile('silence.wav', makeWav({ seconds: 12, kind: 'silent' })));
    await expect(app.getByTestId('voice-ready')).toContainText('silence.wav');
    await expect(app.getByTestId('voice-ready')).toContainText('12.0 s');
    await app.getByRole('button', { name: 'Submit sample' }).click();
    const err = app.getByTestId('voice-error');
    await expect(err).toHaveAttribute('data-code', 'VOICE_SAMPLE_TOO_QUIET');
    await expect(err).toContainText('too quiet');
    await expect(err).toContainText('Move closer to the microphone');
    expect((await status(tokens.accessToken)).profile ?? null).toBeNull();
  });

  test('VOICE-04 a speech-like WAV is accepted with a quality score, or the server says it has no voice provider', async ({ app, tokens, workspace }) => {
    await grantViaApi(tokens.accessToken);
    await app.goto(`/${workspace.slug}/settings/account`);
    await app.getByTestId('voice-file-input').setInputFiles(wavFile('speech.wav', makeWav({ seconds: 12, kind: 'speech' })));
    await app.getByRole('button', { name: 'Submit sample' }).click();
    const ok_ = app.getByTestId('voice-result');
    const err = app.getByTestId('voice-error');
    await expect(ok_.or(err)).toBeVisible();
    if (await ok_.isVisible()) {
      await expect(ok_).toContainText(/Quality score \d+ \/ 100/);
      await expect(app.getByTestId('voice-quality')).toHaveText(/^\d+ \/ 100$/);
      expect((await status(tokens.accessToken)).profile?.qualityScore).toBeGreaterThan(0);
    } else {
      await expect(err).toHaveAttribute('data-code', 'VOICE_CLONING_UNAVAILABLE');
      await expect(err).toContainText('no voice provider is configured');
    }
  });

  test('VOICE-05 other sample problems map to specific messages (too short, not a WAV, wrong file size)', async ({ app, tokens, workspace }) => {
    await grantViaApi(tokens.accessToken);
    await app.goto(`/${workspace.slug}/settings/account`);
    await app.getByTestId('voice-file-input').setInputFiles(wavFile('short.wav', makeWav({ seconds: 2, kind: 'speech' })));
    await app.getByRole('button', { name: 'Submit sample' }).click();
    await expect(app.getByTestId('voice-error')).toHaveAttribute('data-code', 'VOICE_SAMPLE_TOO_SHORT');
    await expect(app.getByTestId('voice-error')).toContainText('at least 10 seconds');
    await app.getByRole('button', { name: 'Discard' }).click();
    await app.getByTestId('voice-file-input').setInputFiles({ name: 'song.mp3', mimeType: 'audio/mpeg', buffer: Buffer.from('ID3 not a wav at all, definitely longer than forty-four bytes of text') });
    await expect(section(app).getByRole('alert')).toContainText('That is not a WAV recording');
    await expect(app.getByTestId('voice-ready')).toHaveCount(0);
    await app.getByTestId('voice-file-input').setInputFiles(wavFile('big.wav', Buffer.alloc(7 * 1024 * 1024)));
    await expect(section(app).getByRole('alert')).toContainText('under 6 MB');
  });

  test('VOICE-06 the server enforces consent: enrolling without it is refused', async ({ tokens }) => {
    const r = await call<{ code: string }>('/me/voice/enroll', { token: tokens.accessToken, body: { sampleBase64: makeWav({ seconds: 6, kind: 'speech' }).toString('base64'), format: 'wav' } });
    expect(r.status).toBeGreaterThanOrEqual(400);
    expect(r.body.code).toBe('VOICE_CONSENT_REQUIRED');
  });

  test('VOICE-07 consent is per user: another user\'s consent is never visible', async () => {
    const other = await createUser('voiceother');
    await grantViaApi((await login(other)).accessToken);
    const fresh = await createUser('voicefresh');
    const t = (await login(fresh)).accessToken;
    expect((await status(t)).consent.granted).toBe(false);
  });
});

// ---- a11y + visual
test.describe('Voice cloning: a11y', () => {
  for (const theme of ['dark', 'light'] as const) {
    for (const vp of VIEWPORTS) {
      test(`VOICE-08 axe: consent card and enrollment card (${theme}, ${vp.name})`, async ({ app, tokens, workspace }) => {
        await app.setViewportSize(vp);
        await setTheme(app, theme);
        await app.goto(`/${workspace.slug}/settings/account`);
        await expect(app.getByTestId('consent-text')).toBeVisible();
        expect(await app.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await scan(app, `voice consent ${theme} ${vp.name}`);
        await grantViaApi(tokens.accessToken);
        await app.reload();
        await expect(app.getByRole('button', { name: 'Record sample' })).toBeVisible();
        await scan(app, `voice enroll ${theme} ${vp.name}`);
        await app.getByTestId('voice-file-input').setInputFiles(wavFile('silence.wav', makeWav({ seconds: 6, kind: 'silent' })));
        await app.getByRole('button', { name: 'Submit sample' }).click();
        await expect(app.getByTestId('voice-error')).toBeVisible();
        expect(await app.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
        await scan(app, `voice error ${theme} ${vp.name}`);
      });
    }
  }
});

raw.describe('Voice cloning: visual regression', () => {
  async function seeded(page: Page) {
    const id = rnd();
    const u = { email: `e2e-vrvoice-${id}@example.com`, password: 'Passw0rd!e2e', fullName: 'Ada Visual' };
    await ok(call('/auth/register', { body: u }));
    await verifyEmail(u.email);
    const tokens = await login({ ...u, userId: '' });
    const ws = await createWorkspace(tokens.accessToken, 'Visual HQ');
    await seedSession(page, tokens);
    return { ws, tokens };
  }
  for (const theme of ['dark', 'light'] as const) {
    for (const vp of VIEWPORTS) {
      raw(`VOICE-09 voice section: consent, then enrollment (${theme}, ${vp.name})`, async ({ page }) => {
        await page.setViewportSize(vp);
        await setTheme(page, theme);
        const { ws, tokens } = await seeded(page);
        await page.goto(`/${ws.slug}/settings/account`);
        await expect(page.getByTestId('consent-text')).toBeVisible();
        const masks = [page.getByTestId('account-menu')];
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(150);
        await expect(page.getByTestId('voice-section')).toHaveScreenshot(`voice-consent-${theme}-${vp.name}.png`, { mask: masks });
        await grantViaApi(tokens.accessToken);
        await page.reload();
        await expect(page.getByRole('button', { name: 'Record sample' })).toBeVisible();
        await page.evaluate(() => document.fonts.ready);
        await page.waitForTimeout(150);
        await expect(page.getByTestId('voice-section')).toHaveScreenshot(`voice-enroll-${theme}-${vp.name}.png`, { mask: [...masks, page.getByTestId('voice-granted-at')] });
      });
    }
  }
});
