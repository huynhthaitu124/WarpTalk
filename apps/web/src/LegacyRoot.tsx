"use client";

/**
 * Entry of the ported warptalk-web UI. The old app was a server-rendered Next.js app (middleware gate,
 * next-intl server config); this product ships as a static export bundled into the macOS app, so:
 * - routing: react-router over the generated table from `src/screens` (scripts/gen-routes.mjs),
 * - i18n: the catalog is loaded client-side (i18n/messages-loader.ts),
 * - the old `proxy.ts` gate runs here (RouteGate).
 */
import { useEffect, useState, useSyncExternalStore, type ReactNode } from "react";
import { BrowserRouter, Navigate, useLocation, useRoutes } from "react-router-dom";
import { IntlClientProvider } from "@/i18n/intl-client-provider";
import { DEFAULT_LOCALE, LOCALE_COOKIE, isSupportedLocale, type Locale } from "@/i18n/locale-constants";
import { loadMessages } from "@/i18n/messages-loader";
import type { Messages } from "@/i18n/message-fallback";
import { Providers } from "@/screens/providers";
import { routes } from "@/routes.generated";
import { hasClientSession, landingPathForSession } from "@/lib/auth/client-session";
import { isHiddenAdminPath } from "@/lib/feature-flags";

const noop = () => () => {};

function readCookie(name: string): string | null {
  for (const part of document.cookie.split("; ")) if (part.startsWith(`${name}=`)) return decodeURIComponent(part.slice(name.length + 1));
  return null;
}

function initialLocale(): Locale {
  const fromCookie = readCookie(LOCALE_COOKIE);
  if (isSupportedLocale(fromCookie)) return fromCookie;
  const nav = navigator.language?.slice(0, 2);
  return isSupportedLocale(nav) ? nav : DEFAULT_LOCALE;
}

function IntlRoot({ children }: { children: ReactNode }) {
  const [locale, setLocale] = useState<Locale>(initialLocale);
  const [messages, setMessages] = useState<{ locale: Locale; messages: Messages } | null>(null);
  useEffect(() => {
    const on = (e: Event) => {
      const next = (e as CustomEvent<string>).detail;
      if (isSupportedLocale(next)) setLocale(next);
    };
    window.addEventListener("wt:locale", on);
    return () => window.removeEventListener("wt:locale", on);
  }, []);
  useEffect(() => {
    let live = true;
    void loadMessages(locale).then((m) => { if (live) setMessages({ locale, messages: m }); });
    document.documentElement.lang = locale;
    return () => { live = false; };
  }, [locale]);
  if (!messages) return null;
  return (
    <IntlClientProvider locale={messages.locale} messages={messages.messages} timeZone={Intl.DateTimeFormat().resolvedOptions().timeZone}>
      {children}
    </IntlClientProvider>
  );
}

const PUBLIC_ROUTES = [
  "/", "/login", "/desktop-login", "/register", "/forgot-password", "/verify-email", "/reset-password",
  "/invitations", "/terms", "/privacy", "/download", "/minutes/shared", "/payment-cancelled",
  "/workspace/payment/plans", "/workspace/payment/success",
];
const AUTH_ROUTES = ["/login", "/register", "/forgot-password", "/verify-email", "/reset-password"];
const under = (list: string[], p: string) => list.some((r) => p === r || p.startsWith(`${r}/`));

/** The old middleware (`proxy.ts`): sign-in gate, signed-in landing and retired-address forwards. */
function RouteGate() {
  const { pathname, search } = useLocation();
  const element = useRoutes(routes);

  const moved =
    (/^\/([^/]+)\/billing\/?$/.exec(pathname)?.[1] ?? "admin") !== "admin" ? pathname.replace(/\/billing\/?$/, "/settings/billing")
    : /^\/[^/]+\/settings\/billing\/payments\/?$/.test(pathname) ? pathname.replace(/\/payments\/?$/, "/invoices")
    : /^\/[^/]+\/settings\/billing\/usage\/?$/.test(pathname) ? pathname.replace(/\/settings\/billing\/usage\/?$/, "/settings/billing")
    : /^\/[^/]+\/settings\/audit-log\/?$/.test(pathname) ? pathname.replace(/\/audit-log\/?$/, "")
    : /^\/[^/]+\/tasks\/?$/.test(pathname) ? pathname.replace(/\/tasks\/?$/, "/home")
    : /^\/admin\/(?:meetings|outbox)(?:\/.*)?$/.test(pathname) ? "/admin"
    : /^\/admin\/glossary-templates(?:\/.*)?$/.test(pathname) ? "/admin/global-glossary"
    : null;
  if (moved) return <Navigate to={moved} replace />;

  const session = hasClientSession();
  if (session && (under(AUTH_ROUTES, pathname) || pathname === "/" || pathname === "/dashboard" || isHiddenAdminPath(pathname))) {
    return <Navigate to={landingPathForSession()} replace />;
  }
  if (!session && !under(PUBLIC_ROUTES, pathname)) {
    return <Navigate to={`/login?redirect=${encodeURIComponent(pathname + search)}`} replace />;
  }
  return element;
}

export function LegacyRoot() {
  // Static export: the server pass renders nothing, the SPA mounts in the browser (see v2/ClientRoot).
  const browser = useSyncExternalStore(noop, () => true, () => false);
  if (!browser) return null;
  return (
    <BrowserRouter>
      <IntlRoot>
        <Providers>
          <RouteGate />
        </Providers>
      </IntlRoot>
    </BrowserRouter>
  );
}
