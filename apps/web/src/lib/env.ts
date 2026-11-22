/**
 * The single place that reads build-time environment and the native host config. Components import
 * from here, never from `process.env` directly, so the bundler can change without touching UI code.
 */
export const IS_DEV: boolean = process.env.NODE_ENV !== 'production';

/** True in the static export (`next build`), false under `next dev`. */
const IS_EXPORT: boolean = process.env.NEXT_PUBLIC_STATIC_EXPORT === '1';

/**
 * Backend origin WITHOUT `/api/v1`. Empty = same origin (`next dev` rewrites `/api/**` to the backend).
 * Static export: the macOS host injects `window.warptalkNative.config.apiBaseUrl`; otherwise
 * `NEXT_PUBLIC_API_BASE_URL`; otherwise same origin (a reverse proxy in front of `out/`).
 */
export function apiBaseUrl(): string {
  if (!IS_EXPORT) return '';
  const native = typeof window === 'undefined' ? undefined : (window as unknown as { warptalkNative?: { config?: { apiBaseUrl?: string } } }).warptalkNative?.config?.apiBaseUrl;
  return (native || process.env.NEXT_PUBLIC_API_BASE_URL || '').replace(/\/+$/, '');
}

/** Absolute-or-relative URL of an API route, `path` starting with `/` (e.g. `/auth/refresh`). */
export function apiUrl(path: string): string {
  return `${apiBaseUrl()}/api/v1${path}`;
}
