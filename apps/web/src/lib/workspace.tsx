import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { api } from './api';
import { page, workspace, type Workspace } from './schemas';
import { useAuth } from './auth';

const KEY = 'wt.workspace';

interface Ctx {
  workspaces: Workspace[];
  current: Workspace | null;
  loading: boolean;
  select(id: string): void;
}
const WsCtx = createContext<Ctx | null>(null);

function readSaved() {
  try { return localStorage.getItem(KEY); } catch { return null; }
}

export function WorkspaceProvider({ children }: { children: ReactNode }) {
  const { userId } = useAuth();
  const [selected, setSelected] = useState<string | null>(readSaved);
  const q = useQuery({
    queryKey: ['workspaces', userId],
    queryFn: () => api('/workspaces', { query: { pageSize: 100 }, schema: page(workspace) }),
    enabled: !!userId,
  });
  const workspaces = useMemo(() => q.data?.items ?? [], [q.data]);
  const current = workspaces.find((w) => w.id === selected) ?? workspaces[0] ?? null;

  useEffect(() => {
    if (current && current.id !== selected) setSelected(current.id);
  }, [current, selected]);

  const select = useCallback((id: string) => {
    setSelected(id);
    try { localStorage.setItem(KEY, id); } catch { /* ignore */ }
  }, []);
  const value = useMemo(() => ({ workspaces, current, loading: q.isLoading, select }), [workspaces, current, q.isLoading, select]);
  return <WsCtx.Provider value={value}>{children}</WsCtx.Provider>;
}

export function useWorkspace() {
  const c = useContext(WsCtx);
  if (!c) throw new Error('useWorkspace outside WorkspaceProvider');
  return c;
}

export const canAdmin = (w: Workspace | null) => w?.myRole === 'OWNER' || w?.myRole === 'ADMIN';
