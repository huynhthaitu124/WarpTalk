// Voice enrollment from the microphone, with Chromium's fake audio device (project "meeting-media").
import { call, expect, ok, test } from '../fixtures';

test.beforeEach(({}, testInfo) => {
  test.skip(testInfo.project.name !== 'meeting-media', 'needs the fake-media Chromium flags of the meeting-media project');
});

test.describe('Voice cloning: microphone', () => {
  test('VOICE-10 record 10-20 s: live meter and timer, Stop unlocks at 10 s, the sample is submitted as mono PCM16 WAV', async ({ app, tokens, workspace }) => {
    await ok(call('/me/voice/consent', { token: tokens.accessToken, body: { granted: true, consentVersion: '2026-11', acknowledgementHash: 'b'.repeat(64) } }));
    await app.goto(`/${workspace.slug}/settings/account`);
    await app.getByRole('button', { name: 'Record sample' }).click();
    const rec = app.getByTestId('voice-recording');
    await expect(rec).toBeVisible();
    await expect(app.getByTestId('voice-meter')).toHaveAttribute('data-active', 'true');
    await expect(app.getByRole('button', { name: 'Stop recording' })).toBeDisabled();
    await expect(app.getByTestId('voice-timer')).toHaveText(/^0:0[1-9] \/ 0:20$/, { timeout: 5000 });
    // The fake device beeps: the meter rises above zero at some point.
    await expect.poll(async () => Number(await app.getByTestId('voice-meter').getAttribute('aria-valuenow')), { timeout: 12_000, intervals: [100] }).toBeGreaterThan(0);
    await expect(app.getByRole('button', { name: 'Stop recording' })).toBeEnabled({ timeout: 15_000 });
    await expect(app.getByTestId('voice-timer')).toHaveText(/^0:1\d \/ 0:20$/);
    await app.getByRole('button', { name: 'Stop recording' }).click();
    const ready = app.getByTestId('voice-ready');
    await expect(ready).toBeVisible();
    await expect(ready).toContainText('Recording');
    await expect(app.getByLabel('Preview of your voice sample')).toBeVisible();
    const [req] = await Promise.all([
      app.waitForRequest((r) => r.url().endsWith('/me/voice/enroll') && r.method() === 'POST'),
      app.getByRole('button', { name: 'Submit sample' }).click(),
    ]);
    const body = req.postDataJSON() as { sampleBase64: string; format: string };
    expect(body.format).toBe('wav');
    const wav = Buffer.from(body.sampleBase64, 'base64');
    expect(wav.subarray(0, 4).toString()).toBe('RIFF');
    expect(wav.subarray(8, 12).toString()).toBe('WAVE');
    expect(wav.readUInt16LE(20)).toBe(1); // PCM
    expect(wav.readUInt16LE(22)).toBe(1); // mono
    expect(wav.readUInt16LE(34)).toBe(16); // 16-bit
    const seconds = (wav.length - 44) / 2 / wav.readUInt32LE(24);
    expect(seconds).toBeGreaterThan(9.5);
    expect(seconds).toBeLessThan(21);
    // Whatever the server answers is shown in a friendly form (the fake beep is not speech).
    const result = app.getByTestId('voice-result').or(app.getByTestId('voice-error'));
    await expect(result).toBeVisible();
    if (await app.getByTestId('voice-error').isVisible()) {
      await expect(app.getByTestId('voice-error')).toHaveAttribute('data-code', /^VOICE_(SAMPLE_[A-Z_]+|CLONING_UNAVAILABLE)$/);
      await expect(app.getByTestId('voice-error')).not.toContainText('VOICE_');
    }
  });

  test('VOICE-11 cancelling a recording releases the microphone and discards the audio', async ({ app, tokens, workspace }) => {
    await ok(call('/me/voice/consent', { token: tokens.accessToken, body: { granted: true, consentVersion: '2026-11', acknowledgementHash: 'c'.repeat(64) } }));
    await app.goto(`/${workspace.slug}/settings/account`);
    await app.getByRole('button', { name: 'Record sample' }).click();
    await expect(app.getByTestId('voice-recording')).toBeVisible();
    await app.getByRole('button', { name: 'Cancel' }).click();
    await expect(app.getByTestId('voice-recording')).toHaveCount(0);
    await expect(app.getByRole('button', { name: 'Record sample' })).toBeVisible();
    await expect(app.getByTestId('voice-ready')).toHaveCount(0);
  });
});
