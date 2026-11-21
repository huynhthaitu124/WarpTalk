import { lazy, Suspense } from 'react';
import { Navigate, Outlet, Route, Routes, useLocation } from 'react-router-dom';
import { useAuth } from '@/lib/auth';
import { WorkspaceProvider } from '@/lib/workspace';
import { Skeleton } from '@/ui';
import { Shell, SkipLink } from '@/features/Shell';
import { LoginPage, RegisterPage, VerifyEmailPage } from '@/features/Auth';
import { RoomsPage } from '@/features/Rooms';
import { RoomDetailPage } from '@/features/RoomDetail';
import { JoinByCodePage } from '@/features/Join';
import { MeetingsPage } from '@/features/Meetings';
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
            <Route index element={<Navigate to="/rooms" replace />} />
            <Route path="/rooms" element={<RoomsPage />} />
            <Route path="/rooms/:roomId" element={<RoomDetailPage />} />
            <Route path="/join" element={<JoinByCodePage />} />
            <Route path="/meetings" element={<MeetingsPage />} />
            <Route path="/meetings/:roomId" element={<MeetingPage />} />
            <Route path="/external" element={<ExternalPage />} />
            <Route path="/workspaces" element={<WorkspacesPage />} />
            <Route path="/settings" element={<SettingsPage />} />
          </Route>
        </Route>
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
    </>
  );
}
