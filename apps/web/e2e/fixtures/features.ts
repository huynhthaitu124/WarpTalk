// Feature flags are served by `GET /api/v1/features?workspaceId=` (ADR 0001). Specs that exercise translation / dubbing /
// voice / bridge UI turn the flags on by mocking the endpoint; the fixtures below do that by default so legacy specs keep
// testing the legacy UI. FEAT-xx specs override it per test with `mockFeatures`.
import type { BrowserContext, Page } from '@playwright/test';

export interface FeatureFlags { translationMeetings: boolean; dubbing: boolean; voiceClone: boolean; externalBridge: boolean; copilot: boolean }

/** Platform defaults (what the backend answers when nothing is configured). */
export const FLAGS_DEFAULT: FeatureFlags = { translationMeetings: false, dubbing: false, voiceClone: false, externalBridge: false, copilot: true };
/** Everything on: the pre-ADR-0001 product. */
export const FLAGS_ALL: FeatureFlags = { translationMeetings: true, dubbing: true, voiceClone: true, externalBridge: true, copilot: true };

export interface FeaturesMock { calls: () => string[] }

/** Answer `GET /features` with `flags` (merged over the defaults), or with `404` when `flags` is 'missing'. */
export async function mockFeatures(target: Page | BrowserContext, flags: Partial<FeatureFlags> | 'missing'): Promise<FeaturesMock> {
  const calls: string[] = [];
  await target.route(/\/api\/v1\/features(\?|$)/, (route) => {
    calls.push(route.request().url());
    if (flags === 'missing') return route.fulfill({ status: 404, contentType: 'application/json', body: JSON.stringify({ code: 'NOT_FOUND', message: 'not found' }) });
    return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify({ ...FLAGS_DEFAULT, ...flags }) });
  });
  return { calls: () => calls };
}
