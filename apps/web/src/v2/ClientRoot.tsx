'use client';

import { useSyncExternalStore } from 'react';
import { AppRoot } from './AppRoot';

const noop = () => () => {};

/**
 * Next prerenders the page on the server (static export), but the SPA needs `window`, `localStorage` and the
 * browser history, so the server (and the hydration pass) render nothing and the real app mounts right after.
 * It is a normal static import, not `dynamic(ssr:false)`, so the code ships in the first script batch and is
 * running by the `load` event (keyboard/E2E can interact immediately after navigation).
 */
export function ClientRoot() {
  const browser = useSyncExternalStore(noop, () => true, () => false);
  return browser ? <AppRoot /> : null;
}
