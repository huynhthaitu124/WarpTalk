import { lazy, Suspense } from 'react';
import { Navigate, Outlet, Route, Routes, useLocation } from 'react-router-dom';
import { useAuth } from '@/lib/auth';
import { useWorkspace, WorkspaceProvider } from '@/lib/workspace';
import { Skeleton } from '@/ui';
import { Shell, SkipLink } from '@/features/Shell';
import { LoginPage, RegisterPage, VerifyEmailPage } from '@/features/Auth';
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

/** Old unprefixed list URLs (`/rooms`, `/meetings`, `/settings`, `/`) forward to the current workspace, keeping the query. */
function ToWorkspace({ to }: { to: string }) {
  const { current, loading, wsPath } = useWorkspace();
  const loc = useLocation();
  if (loading) return <div className="p-8"><Skeleton className="h-8 w-48" /></div>;
  if (!current) return <Navigate to={`/workspaces${loc.search}`} replace />;
  return <Navigate to={`${wsPath(to)}${loc.search}`} replace />;
}

export function App() {
  return (
    <>
      <SkipLink />
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/register" element={<RegisterPage />} />
        <Route path="/verify-email" element={<VerifyEmailPage />} />
        <Route path="/design" element={<Suspense fallback={null}><DesignPage /></Suspense>} />
        <Route element={<Protected />}>
          <Route element={<Shell />}>
            <Route index element={<ToWorkspace to="/home" />} />
            {/* Workspace pages live under /:workspaceSlug (shareable links). */}
            <Route path="/:workspaceSlug/home" element={<HomePage />} />
            <Route path="/:workspaceSlug/meetings" element={<MeetingsPage />} />
            <Route path="/:workspaceSlug/members" element={<MembersPage />} />
            <Route path="/:workspaceSlug/glossary" element={<GlossaryListPage />} />
            <Route path="/:workspaceSlug/glossary/:glossaryId" element={<GlossaryDetailPage />} />
            <Route path="/:workspaceSlug/knowledge" element={<KnowledgePage />} />
            <Route path="/:workspaceSlug/documents" element={<DocumentsPage />} />
            <Route path="/:workspaceSlug/settings" element={<SettingsPage />} />
            <Route path="/:workspaceSlug/settings/:section" element={<SettingsPage />} />
            {/* Id-addressed pages, valid for people who are not in the room's workspace. */}
            <Route path="/rooms" element={<ToWorkspace to="/meetings" />} />
            <Route path="/rooms/:roomId" element={<RoomDetailPage />} />
            <Route path="/join" element={<JoinByCodePage />} />
            <Route path="/meetings" element={<ToWorkspace to="/meetings" />} />
            <Route path="/meetings/:roomId" element={<MeetingPage />} />
            <Route path="/external" element={<ExternalPage />} />
            <Route path="/workspaces" element={<WorkspacesPage />} />
            <Route path="/settings" element={<ToWorkspace to="/settings" />} />
          </Route>
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </>
  );
}
