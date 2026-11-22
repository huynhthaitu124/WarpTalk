import type { Page } from '@playwright/test';
import type { HarnessOptions, MockMeeting } from './meeting';

const HARNESS = 'meeting';

/**
 * Mounts the real in-meeting UI (scripted media session, real backend for roster/host actions) on top of an
 * already signed-in app page. `window.__meeting` is the scripting handle: speak(), data(), setConnection()...
 */
export async function mountMeeting(page: Page, kind: 'live' | 'offline' | 'ended', opts: HarnessOptions) {
  await page.goto('/rooms');
  await page.waitForSelector('main');
  await page.evaluate(async ({ path, kind: k, o }) => {
    const m = await (async () => {
      for (let n = 0; n < 500 && !window.__wtHarness; n++) await new Promise((r) => setTimeout(r, 20));
      return window.__wtHarness![path as 'captions']();
    })();
    const el = document.createElement('div');
    el.id = 'harness';
    document.body.append(el);
    m.mount(el, k, o);
  }, { path: HARNESS, kind, o: opts });
}

/** Runs a method of the scripted session inside the page. */
export function script<K extends keyof MockMeeting>(page: Page, method: K, ...args: MockMeeting[K] extends (...a: infer A) => unknown ? A : never) {
  return page.evaluate(({ m, a }) => (window as unknown as { __meeting: Record<string, (...x: unknown[]) => unknown> }).__meeting[m]!(...a), { m: method as string, a: args as unknown[] });
}

export async function sent(page: Page): Promise<{ topic: string; payload: Record<string, unknown> }[]> {
  return page.evaluate(() => (window as unknown as { __meeting: { sent: never[] } }).__meeting.sent);
}
