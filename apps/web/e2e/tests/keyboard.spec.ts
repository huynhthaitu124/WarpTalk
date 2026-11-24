import { test as raw } from '../fixtures';
import { createWorkspace, expect, rnd, test } from '../fixtures';

raw.describe('Keyboard', () => {
  raw('KBD-02 focus ring visible while tabbing the login form', async ({ page }) => {
    await page.goto('/login');
    const seen: string[] = [];
    for (let i = 0; i < 5; i++) {
      await page.keyboard.press('Tab');
      const info = await page.evaluate(() => {
        const el = document.activeElement as HTMLElement;
        const cs = getComputedStyle(el);
        return { tag: el.tagName, name: el.getAttribute('aria-label') || el.textContent?.trim() || el.id, outlineW: parseFloat(cs.outlineWidth), outlineStyle: cs.outlineStyle, vis: el.matches(':focus-visible') };
      });
      if (info.tag === 'BODY') continue;
      expect(info.vis, `focus-visible on ${info.tag}`).toBe(true);
      expect(info.outlineStyle, `outline on ${info.name}`).not.toBe('none');
      expect(info.outlineW, `outline width on ${info.name}`).toBeGreaterThan(0);
      seen.push(info.tag);
    }
    expect(seen).toEqual(expect.arrayContaining(['INPUT', 'BUTTON', 'A']));
  });
});

test.describe('Keyboard (signed in)', () => {
  test('KBD-01 command menu: open, filter, run, escape', async ({ app }) => {
    await app.goto('/');
    await expect(app.getByTestId('current-workspace')).not.toHaveText('No workspace');
    await app.keyboard.press('ControlOrMeta+k');
    const dialog = app.getByRole('dialog', { name: 'Command menu' });
    await expect(dialog).toBeVisible();
    await expect(app.getByRole('combobox', { name: 'Search commands' })).toBeFocused();
    await app.keyboard.type('settings');
    await expect(app.getByRole('option')).toHaveCount(1);
    await app.keyboard.press('Enter');
    await expect(app).toHaveURL(/\/settings\/general/);
    await expect(dialog).toHaveCount(0);
    await app.keyboard.press('ControlOrMeta+k');
    await expect(dialog).toBeVisible();
    await app.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
  });

  test('KBD-03 dialog traps focus, Escape restores focus', async ({ app }) => {
    await app.goto('/rooms');
    const trigger = app.getByRole('button', { name: 'New meeting' }).first();
    await trigger.focus();
    await app.keyboard.press('Enter');
    const dialog = app.getByRole('dialog', { name: 'Create meeting' });
    await expect(dialog).toBeVisible();
    for (let i = 0; i < 12; i++) {
      await app.keyboard.press('Tab');
      expect(await app.evaluate(() => !!document.activeElement?.closest('[role="dialog"]'))).toBe(true);
    }
    await app.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
    await expect(trigger).toBeFocused();
  });

  test('KBD-04 skip link moves focus to main', async ({ app }) => {
    await app.goto('/');
    await expect(app.getByTestId('current-workspace')).toBeVisible();
    await app.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    await app.keyboard.press('Tab');
    const skip = app.getByRole('link', { name: 'Skip to content' });
    await expect(skip).toBeFocused();
    await app.keyboard.press('Enter');
    await expect(app).toHaveURL(/#main$/);
  });

  test('KBD-05 workspace switcher by keyboard', async ({ app, tokens, workspace }) => {
    const second = await createWorkspace(tokens.accessToken, `Keys ${rnd()}`);
    await app.goto('/');
    const switcher = app.getByTestId('workspace-switcher');
    const current = app.getByTestId('current-workspace');
    await expect(current).not.toHaveText('No workspace');
    const shown = (await current.textContent())!.trim();
    const other = shown === workspace.name ? second : workspace;
    await switcher.focus();
    await app.keyboard.press('Enter');
    await expect(app.getByRole('menu')).toBeVisible();
    // Items: [ws..., Manage workspaces, Create workspace]; walk until the target is focused, then select it.
    // (The menu may open with the first item already focused, so check before each ArrowDown.)
    for (let i = 0; i < 5; i++) {
      const focused = await app.evaluate(() => document.activeElement?.textContent?.trim());
      if (focused === other.name) break;
      await app.keyboard.press('ArrowDown');
    }
    await app.keyboard.press('Enter');
    await expect(current).toHaveText(other.name);
  });

  test('KBD-06 command menu opens the create-meeting dialog', async ({ app }) => {
    await app.goto('/');
    await expect(app.getByTestId('current-workspace')).not.toHaveText('No workspace');
    await app.keyboard.press('ControlOrMeta+k');
    await app.keyboard.type('create meeting');
    await app.keyboard.press('Enter');
    await expect(app.getByRole('dialog', { name: 'Create meeting' })).toBeVisible();
  });

  test('KBD-07 advertised shortcuts work: G then M/H/S, C, J (FN-01)', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/home`);
    await expect(app.getByTestId('current-workspace')).toHaveText(workspace.name);
    // shortcuts follow the feature flags: wait until they have loaded
    await expect(app.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'Meetings' })).toBeVisible();
    await app.keyboard.press('g');
    await app.keyboard.press('m');
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/meetings`));
    await expect(app.getByRole('heading', { name: 'Meetings', level: 1 })).toBeVisible();
    await app.keyboard.press('g');
    await app.keyboard.press('s');
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/settings/general`));
    await expect(app.getByRole('heading', { name: 'Settings', level: 1 })).toBeVisible();
    await app.keyboard.press('g');
    await app.keyboard.press('h');
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/home`));
    await expect(app.getByRole('heading', { name: /Welcome back/, level: 1 })).toBeVisible();
    await app.keyboard.press('c');
    await expect(app.getByRole('dialog', { name: 'Create meeting' })).toBeVisible();
    await app.keyboard.press('Escape');
    await expect(app.getByRole('dialog')).toHaveCount(0);
    await app.keyboard.press('j');
    await expect(app.getByRole('dialog', { name: 'Join a meeting' })).toBeVisible();
  });

  test('KBD-08 shortcuts are ignored while typing', async ({ app, workspace }) => {
    await app.goto(`/${workspace.slug}/meetings`);
    const search = app.getByRole('searchbox', { name: 'Search meetings' });
    await search.click();
    await app.keyboard.type('cgm');
    await expect(search).toHaveValue('cgm');
    await expect(app.getByRole('dialog')).toHaveCount(0);
    await expect(app).toHaveURL(new RegExp(`/${workspace.slug}/meetings`));
  });

  test('KBD-09 the command menu shows only real shortcuts', async ({ app }) => {
    await app.goto('/');
    await expect(app.getByTestId('current-workspace')).not.toHaveText('No workspace');
    await app.keyboard.press('ControlOrMeta+k');
    const dialog = app.getByRole('dialog', { name: 'Command menu' });
    await expect(dialog.getByText('G M')).toBeVisible();
    await expect(dialog.getByText('G R')).toHaveCount(0);
  });
});
