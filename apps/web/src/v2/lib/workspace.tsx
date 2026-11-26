import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';
import { useQueries, useQuery } from '@tanstack/react-query';
import { api } from './api';
import { page, room, workspace, type Workspace } from './schemas';
import { useAuth } from './auth';

const KEY = 'wt.workspace';

interface Ctx {
  workspaces: Workspace[];
  current: Workspace | null;
  /** True until the workspace list is loaded and a default has been chosen. */
  loading: boolean;
  select(id: string): void;
  /** App path inside the current workspace: `wsPath('/members')` -> `/acme/members`. */
  wsPath(path?: string): string;
}
const WsCtx = createContext<Ctx | null>(null);

function readSaved() {
  try { return localStorage.getItem(KEY); } catch { return null; }
}
function writeSaved(id: string) {
  try { localStorage.setItem(KEY, id); } catch { /* ignore */ }
}

/**
 * The current workspace comes from, in order: the `/:workspaceSlug/...` URL, the persisted
 * last-used workspace, then a default (the one that actually has rooms, else the first). Whatever
 * is resolved is persisted, so the choice is stable across reloads (FN-02, IA-03).
 */
export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const { userId } = useAuth();
  const loc = useLocation();
  const navigate = useNavigate();
  const [saved, setSaved] = useState<string | null>(readSaved);
  const q = useQuery({
    queryKey: ['workspaces', userId],
    queryFn: () => api('/workspaces', { query: { pageSize: 100 }, schema: page(workspace) }),
    enabled: !!userId,
  });
  const workspaces = useMemo(() => q.data?.items ?? [], [q.data]);

  const seg = loc.pathname.split('/')[1] ?? '';
  const fromUrl = workspaces.find((w) => w.slug === seg) ?? null;
  const fromSaved = workspaces.find((w) => w.id === saved) ?? null;

  // No usable choice and several workspaces: prefer the one that has rooms.
  const needDefault = !fromUrl && !fromSaved && workspaces.length > 1;
  const counts = useQueries({
    queries: workspaces.map((w) => ({
      queryKey: ['rooms', w.id, 'count'],
      enabled: needDefault,
      staleTime: 60_000,
      queryFn: () => api('/translation-rooms', { query: { workspaceId: w.id, pageSize: 1 }, schema: page(room) }),
    })),
  });
  const counting = needDefault && counts.some((c) => c.isPending);
  const busiest = useMemo(() => {
    if (!needDefault || counting) return null;
    let best = workspaces[0] ?? null;
    let bestTotal = -1;
    workspaces.forEach((w, i) => {
      const t = counts[i]?.data?.total ?? 0;
      if (t > bestTotal) { best = w; bestTotal = t; }
    });
    return best;
  }, [needDefault, counting, workspaces, counts]);

  const current = fromUrl ?? fromSaved ?? busiest ?? (workspaces.length === 1 ? workspaces[0]! : null);

  useEffect(() => {
    if (current && current.id !== saved) { setSaved(current.id); writeSaved(current.id); }
  }, [current, saved]);

  const select = useCallback((id: string) => {
    setSaved(id);
    writeSaved(id);
    const w = workspaces.find((x) => x.id === id);
    // Staying on a workspace page: move to the same page of the other workspace.
    if (w && fromUrl && w.id !== fromUrl.id) navigate(`${loc.pathname.replace(`/${fromUrl.slug}`, `/${w.slug}`)}${loc.search}`);
  }, [workspaces, fromUrl, navigate, loc.pathname, loc.search]);

  const wsPath = useCallback((p = '/home') => (current ? `/${current.slug}${p}` : '/workspaces'), [current]);
  // A refetch with nothing resolved yet (e.g. right after accepting an invitation) is still "loading".
  const loading = q.isLoading || counting || (!current && q.isFetching);
  const value = useMemo(() => ({ workspaces, current, loading, select, wsPath }), [workspaces, current, loading, select, wsPath]);
  return <WsCtx.Provider value={value}>{children}</WsCtx.Provider>;
}

export function useWorkspace() {
  const c = useContext(WsCtx);
  if (!c) throw new Error('useWorkspace outside WorkspaceProvider');
  return c;
}

/** Like `useWorkspace` but returns null outside a provider (shared components, test harnesses). */
export function useWorkspaceOptional() {
  return useContext(WsCtx);
}

export const canAdmin = (w: Workspace | null) => w?.myRole === 'OWNER' || w?.myRole === 'ADMIN';
