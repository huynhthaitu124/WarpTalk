import { lazy, Suspense, useEffect } from 'react';
import { Navigate, Outlet, Route, Routes, useLocation, useNavigate } from 'react-router-dom';
import { useAuth } from '@v2/lib/auth';
import { useWorkspace, WorkspaceProvider } from '@v2/lib/workspace';
import { Padded, Skeleton } from '@v2/ui';
import { Shell, SkipLink } from '@v2/features/Shell';
import { ForgotPasswordPage, LoginPage, RegisterPage, ResetPasswordPage, VerifyEmailPage } from '@v2/features/Auth';
import { RoomDetailPage } from '@v2/features/RoomDetail';
import { JoinByCodePage } from '@v2/features/Join';
import { MeetingsPage } from '@v2/features/Meetings';
import { HomePage } from '@v2/features/Home';
import { MembersPage } from '@v2/features/Members';
import { GlossaryDetailPage, GlossaryListPage } from '@v2/features/Glossary';
import { KnowledgePage } from '@v2/features/Knowledge';
import { DocumentsPage } from '@v2/features/Documents';
import { MeetingPage } from '@v2/features/Meeting';
import { ExternalPage } from '@v2/features/External';
import { BillingPage } from '@v2/features/Billing';
import { SettingsPage } from '@v2/features/Settings';
import { WorkspacesPage } from '@v2/features/Workspaces';
import { MyTasksPage } from '@v2/features/MyTasks';
import { QualityPage } from '@v2/features/Quality';
import { SessionDetailPage, SessionsPage } from '@v2/features/Sessions';
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
            <Route path="/:workspaceSlug/tasks" element={<MyTasksPage />} />
            <Route path="/:workspaceSlug/quality" element={<QualityPage />} />
            <Route path="/:workspaceSlug/members" element={<MembersPage />} />
            <Route path="/:workspaceSlug/glossary" element={<GlossaryListPage />} />
            <Route path="/:workspaceSlug/glossary/:glossaryId" element={<GlossaryDetailPage />} />
            <Route path="/:workspaceSlug/knowledge" element={<KnowledgePage />} />
            <Route path="/:workspaceSlug/documents" element={<DocumentsPage />} />
            <Route path="/:workspaceSlug/billing" element={<BillingPage />} />
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
