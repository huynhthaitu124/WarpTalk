'use client';

import { useEffect } from 'react';

/**
 * E2E only (`NEXT_PUBLIC_E2E_HARNESS=1`, set by playwright.config.ts for `next dev`): exposes the test
 * harness modules to Playwright as `window.__wtHarness.<name>()` (a dynamic import), replacing the raw
 * module URLs the Vite dev server used to serve. Statically dead code (and its imports) in real builds.
 */
export function HarnessBridge() {
  useEffect(() => {
    if (process.env.NEXT_PUBLIC_E2E_HARNESS !== '1') return;
    (window as unknown as { __wtHarness: unknown }).__wtHarness = {
      captions: () => import('../../e2e/harness/captions'),
      meeting: () => import('../../e2e/harness/meeting'),
      audio: () => import('../../e2e/harness/audio'),
      suggestions: () => import('@v2/lib/suggestions'),
    };
  }, []);
  return null;
}
