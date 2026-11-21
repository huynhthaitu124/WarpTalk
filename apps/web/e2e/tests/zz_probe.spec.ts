import { expect, test } from '../fixtures';
test('probe', async ({ app }) => {
  await app.setViewportSize({ width: 390, height: 800 });
  await app.goto('/');
  await expect(app.getByTestId('account-menu')).toBeHidden();
  const opener = app.getByRole('button', { name: 'Open navigation' });
  await opener.focus();
  await app.keyboard.press('Enter');
  await expect(app.getByRole('dialog', { name: 'Navigation' })).toBeVisible();
  await app.waitForTimeout(300);
  const out: string[] = [await app.evaluate(() => 'before: ' + document.activeElement?.outerHTML.slice(0, 90))];
  for (let i = 0; i < 4; i++) { await app.keyboard.press('Tab'); out.push(await app.evaluate(() => document.activeElement?.outerHTML.slice(0, 90) + ' | ' + !!document.activeElement?.closest('[role="dialog"]'))); }
  expect(out.join('\n')).toBe('x');
});
