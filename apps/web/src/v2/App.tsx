import { lazy, Suspense, useEffect } from 'react';
import { Navigate, Outlet, Route, Routes, useLocation } from 'react-router-dom';
import { useAuth } from '@v2/lib/auth';
import { WorkspaceProvider } from '@v2/lib/workspace';
import { Padded, Skeleton } from '@v2/ui';
import { ForgotPasswordPage, LoginPage, RegisterPage, ResetPasswordPage, VerifyEmailPage } from '@v2/features/Auth';
import { ExternalPage } from '@v2/features/External';
import { FeatureRoute } from '@v2/lib/features';

const DesignPage = lazy(() => import('@v2/features/Design').then((m) => ({ default: m.DesignPage })));

function Protected() {
  const { status } = useAuth();
  const loc = useLocation();
  if (status === 'loading') return <div className="p-8"><Skeleton className="h-8 w-48" /></div>;
  if (status === 'anon') return <Navigate to="/login" replace state={{ from: loc.pathname + loc.search }} />;
  return (
    <WorkspaceProvider>
      <Outlet />
    </WorkspaceProvider>
  );
}

/**
 * Only `/external` (and `/overlay`, its own page) still run on this router; the product UI is the ported legacy
 * app. Any other location the v2 shell navigates to (Home, Copilot, Settings, Back to app...) is handed to the
 * legacy UI with a full page load, mapped to its matching legacy address.
 */
export function legacyPathFor(pathname: string): string {
  const m = /^\/([^/]+)\/(meetings|sessions|tasks|quality|settings\/[^/]+)\/?$/.exec(pathname);
  if (m) {
    const legacy: Record<string, string> = { meetings: 'rooms', sessions: 'rooms', tasks: 'home', quality: 'insights' };
    return `/${m[1]}/${legacy[m[2]!] ?? 'settings'}`;
  }
  if (/^\/(meetings|sessions)\/[^/]+/.test(pathname)) return pathname.replace(/^\/(meetings|sessions)/, '/rooms');
  if (pathname === '/workspaces') return '/workspace';
  if (pathname === '/meetings' || pathname === '/sessions') return '/';
  return pathname;
}

function LeaveToLegacy() {
  const { pathname, search, hash, state } = useLocation();
  const stay = pathname === '/external' || pathname === '/overlay';
  const from = (state as { from?: string } | null)?.from;
  useEffect(() => {
    if (stay) return;
    // Signed out on /external: the legacy login returns to it through `?redirect=`.
    const target = pathname === '/login' && from ? `/login?redirect=${encodeURIComponent(from)}` : `${legacyPathFor(pathname)}${search}${hash}`;
    // A full load on purpose: the legacy UI is a different root, not a route of this router.
    window.location.assign(target);
  }, [stay, pathname, search, hash, from]);
  return null;
}

export function App() {
  return (
    <>
      <LeaveToLegacy />
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/register" element={<RegisterPage />} />
        <Route path="/forgot-password" element={<ForgotPasswordPage />} />
        <Route path="/reset-password" element={<ResetPasswordPage />} />
        <Route path="/verify-email" element={<VerifyEmailPage />} />
        <Route path="/design" element={<Suspense fallback={null}><DesignPage /></Suspense>} />
        <Route element={<Protected />}>
          {/* The product UI is the legacy app; this router only keeps the external-meeting page, with no shell. */}
          <Route path="/external" element={<main id="main" className="h-dvh overflow-y-auto bg-canvas"><ExternalRoute /></main>} />
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </>
  );
}

/** `/external`: the translation bridge is flag-gated, but `auto=1` is the copilot pill (gated by `copilot`). */
function ExternalRoute() {
  const { search } = useLocation();
  const copilot = new URLSearchParams(search).get('auto') === '1';
  return <FeatureRoute name={copilot ? 'copilot' : 'externalBridge'}><Padded><ExternalPage /></Padded></FeatureRoute>;
}
