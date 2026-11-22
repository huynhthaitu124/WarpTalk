import { createRoom, expect, test } from '../fixtures';

// Runs against the static export (`next build` -> out/, served by scripts/serve-out.mjs with the same
// fallback rules as the macOS bundle server). Only the `export` project runs this file.
test.describe('Static export (macOS bundle)', () => {
  test('EXP-01 deep link /rooms/<id> loads the SPA and renders the room (signed in)', async ({ app, tokens, workspace }) => {
    const room = await createRoom(tokens.accessToken, workspace.id, { title: 'Export deep link' });
    const res = await app.goto(`/rooms/${room.id}`);
    expect(res?.status()).toBe(200);
    await expect(app.getByRole('heading', { name: 'Export deep link', level: 1 })).toBeVisible();
    await app.reload();
    await expect(app.getByRole('heading', { name: 'Export deep link', level: 1 })).toBeVisible();
  });

  test('EXP-02 signed out: deep link redirects to /login, and a missing asset is a real 404', async ({ page, request }) => {
    await page.goto('/meetings/00000000-0000-0000-0000-000000000000');
    await expect(page).toHaveURL(/\/login$/);
    await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
    expect((await request.get('/_next/static/missing.js')).status()).toBe(404);
  });
});
