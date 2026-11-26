"use client";
/**
 * `next/navigation` for the ported UI. The app is a static export (one catch-all page bundled into the
 * macOS app), so routing is done client-side by react-router; these hooks give the old pages the
 * Next.js API they were written against. Imports of "next/navigation" are rewritten to this module.
 */
import { useMemo } from "react";
import { useLocation, useNavigate, useParams as useRRParams, useSearchParams as useRRSearchParams } from "react-router-dom";

export interface AppRouterInstance {
  push(href: string, options?: { scroll?: boolean }): void;
  replace(href: string, options?: { scroll?: boolean }): void;
  back(): void;
  forward(): void;
  refresh(): void;
  prefetch(href: string): void;
}

function isExternal(href: string) {
  return /^[a-z][a-z0-9+.-]*:/i.test(href) && !href.startsWith(window.location.origin);
}

export function useRouter(): AppRouterInstance {
  const navigate = useNavigate();
  return useMemo<AppRouterInstance>(() => {
    const go = (href: string, replace: boolean, scroll = true) => {
      if (isExternal(href)) {
        if (replace) window.location.replace(href);
        else window.location.assign(href);
        return;
      }
      void navigate(href.startsWith(window.location.origin) ? href.slice(window.location.origin.length) : href, { replace });
      if (scroll) window.scrollTo(0, 0);
    };
    return {
      push: (href, o) => go(href, false, o?.scroll !== false),
      replace: (href, o) => go(href, true, o?.scroll !== false),
      back: () => { void navigate(-1); },
      forward: () => { void navigate(1); },
      // Server components do not exist here; a refresh re-renders through the query cache instead.
      refresh: () => { window.dispatchEvent(new Event("wt:router-refresh")); },
      prefetch: () => undefined,
    };
  }, [navigate]);
}

export function usePathname(): string {
  return useLocation().pathname;
}

export function useSearchParams(): URLSearchParams {
  const [params] = useRRSearchParams();
  return params;
}

export type ReadonlyURLSearchParams = URLSearchParams;

export function useParams<T extends Record<string, string | string[]> = Record<string, string>>(): T {
  return useRRParams() as unknown as T;
}

export function redirect(href: string): never {
  window.location.replace(href);
  throw new Error("redirect");
}

export function notFound(): never {
  throw new Error("NEXT_NOT_FOUND");
}
