import type { NextConfig } from 'next';
import { PHASE_DEVELOPMENT_SERVER } from 'next/constants';

/**
 * - `next dev`: same-origin `/api/**` is rewritten to the backend (`API_PROXY_TARGET`, default :8080;
 *   the E2E suite sets :18080), so the browser never needs CORS.
 * - `next build`: static export to `out/` (the macOS app bundles it and serves it as `warptalk://app/`;
 *   it injects `window.warptalkNative.config.apiBaseUrl`). Rewrites do not exist in a static export.
 */
export default function config(phase: string): NextConfig {
  const dev = phase === PHASE_DEVELOPMENT_SERVER;
  const target = process.env.API_PROXY_TARGET ?? 'http://127.0.0.1:8080';
  return {
    reactStrictMode: true,
    poweredByHeader: false,
    ...(dev ? {} : { output: 'export' as const }),
    env: { NEXT_PUBLIC_STATIC_EXPORT: dev ? '0' : '1' },
    // Separate output dirs let the E2E dev server and the export smoke build run next to `npm run dev`.
    ...(process.env.NEXT_DIST_DIR ? { distDir: process.env.NEXT_DIST_DIR } : {}),
    devIndicators: false,
    images: { unoptimized: true },
    turbopack: { root: process.cwd() },
    ...(dev
      ? {
          // The SSE streams must not be buffered/compressed by the dev proxy.
          compress: false,
          async rewrites() {
            return [{ source: '/api/:path*', destination: `${target}/api/:path*` }];
          },
        }
      : {}),
  };
}
