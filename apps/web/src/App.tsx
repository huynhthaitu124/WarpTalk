import { lazy, Suspense, useEffect } from 'react';
import { Navigate, Outlet, Route, Routes, useLocation, useNavigate } from 'react-router-dom';
import { useAuth } from '@/lib/auth';
import { useWorkspace, WorkspaceProvider } from '@/lib/workspace';
import { Padded, Skeleton } from '@/ui';
import { Shell, SkipLink } from '@/features/Shell';
import { ForgotPasswordPage, LoginPage, RegisterPage, ResetPasswordPage, VerifyEmailPage } from '@/features/Auth';
import { RoomDetailPage } from '@/features/RoomDetail';
import { JoinByCodePage } from '@/features/Join';
import { MeetingsPage } from '@/features/Meetings';
import { HomePage } from '@/features/Home';
import { MembersPage } from '@/features/Members';
import { GlossaryDetailPage, GlossaryListPage } from '@/features/Glossary';
import { KnowledgePage } from '@/features/Knowledge';
import { DocumentsPage } from '@/features/Documents';
import { MeetingPage } from '@/features/Meeting';
import { ExternalPage } from '@/features/External';
import { SettingsPage } from '@/features/Settings';
import { WorkspacesPage } from '@/features/Workspaces';
import { SessionDetailPage, SessionsPage } from '@/features/Sessions';
import { FeatureRoute } from '@/lib/features';

const DesignPage = lazy(() => import('@/features/Design').then((m) => ({ default: m.DesignPage })));

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
 * Old unprefixed list URLs (`/rooms`, `/meetings`, `/settings`, `/`) forward to the current workspace, keeping the
 * query and hash. Search/hash are read from the live URL when the redirect fires (not at render time): the redirect
 * effect can run a beat after the page is interactive, and a hash set meanwhile (skip link) must survive it.
 */
function ToWorkspace({ to }: { to: string }) {
  const { current, loading, wsPath } = useWorkspace();
  const navigate = useNavigate();
  const target = current ? wsPath(to) : '/workspaces';
  useEffect(() => {
    if (loading) return;
    navigate(`${target}${window.location.search}${window.location.hash}`, { replace: true });
  }, [loading, target, navigate]);
  return loading ? <div className="p-8"><Skeleton className="h-8 w-48" /></div> : null;
}

export function App() {
  return (
    <>
      <SkipLink />
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/register" element={<RegisterPage />} />
        <Route path="/forgot-password" element={<ForgotPasswordPage />} />
        <Route path="/reset-password" element={<ResetPasswordPage />} />
        <Route path="/verify-email" element={<VerifyEmailPage />} />
        <Route path="/design" element={<Suspense fallback={null}><DesignPage /></Suspense>} />
        <Route element={<Protected />}>
          <Route element={<Shell />}>
            <Route index element={<ToWorkspace to="/home" />} />
            {/* Workspace pages live under /:workspaceSlug (shareable links). */}
            <Route path="/:workspaceSlug/home" element={<HomePage />} />
            <Route path="/:workspaceSlug/meetings" element={<FeatureRoute name="translationMeetings"><MeetingsPage /></FeatureRoute>} />
            <Route path="/:workspaceSlug/sessions" element={<FeatureRoute name="copilot"><SessionsPage /></FeatureRoute>} />
            <Route path="/:workspaceSlug/members" element={<MembersPage />} />
            <Route path="/:workspaceSlug/glossary" element={<GlossaryListPage />} />
            <Route path="/:workspaceSlug/glossary/:glossaryId" element={<GlossaryDetailPage />} />
            <Route path="/:workspaceSlug/knowledge" element={<KnowledgePage />} />
            <Route path="/:workspaceSlug/documents" element={<DocumentsPage />} />
            <Route path="/:workspaceSlug/settings" element={<SettingsPage />} />
            <Route path="/:workspaceSlug/settings/:section" element={<SettingsPage />} />
            {/* Id-addressed pages, valid for people who are not in the room's workspace. */}
            <Route path="/rooms" element={<FeatureRoute name="translationMeetings"><ToWorkspace to="/meetings" /></FeatureRoute>} />
            <Route path="/rooms/:roomId" element={<FeatureRoute name="translationMeetings"><RoomDetailPage /></FeatureRoute>} />
            <Route path="/join" element={<FeatureRoute name="translationMeetings"><Padded><JoinByCodePage /></Padded></FeatureRoute>} />
            <Route path="/meetings" element={<FeatureRoute name="translationMeetings"><ToWorkspace to="/meetings" /></FeatureRoute>} />
            <Route path="/meetings/:roomId" element={<FeatureRoute name="translationMeetings"><MeetingPage /></FeatureRoute>} />
            <Route path="/sessions" element={<FeatureRoute name="copilot"><ToWorkspace to="/sessions" /></FeatureRoute>} />
            <Route path="/sessions/:sessionId" element={<FeatureRoute name="copilot"><SessionDetailPage /></FeatureRoute>} />
            <Route path="/external" element={<FeatureRoute name="externalBridge"><Padded><ExternalPage /></Padded></FeatureRoute>} />
            <Route path="/workspaces" element={<Padded><WorkspacesPage /></Padded>} />
            <Route path="/settings" element={<ToWorkspace to="/settings" />} />
          </Route>
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </>
  );
}
