import { createContext, useCallback, useContext, useEffect, useMemo, useSyncExternalStore, useState, type ReactNode } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { api } from './api';
import { authResponse, profile, registerResponse } from './schemas';
import { session } from './session';

type Status = 'loading' | 'authed' | 'anon';

interface AuthCtx {
  status: Status;
  userId: string | null;
  email: string | null;
  login(email: string, password: string): Promise<void>;
  loginWithGoogle(idToken: string): Promise<void>;
  register(input: { email: string; password: string; fullName: string }): Promise<string>;
  verifyEmail(token: string): Promise<void>;
  logout(): Promise<void>;
}

const Ctx = createContext<AuthCtx | null>(null);

function useSessionInfo() {
  return useSyncExternalStore(session.subscribe, () => session.info);
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const qc = useQueryClient();
  const sInfo = useSessionInfo();
  const [booted, setBooted] = useState(() => !session.refreshToken);

  // Silent refresh on boot (single-flight, rotates the refresh token).
  useEffect(() => {
    if (booted) return;
    let alive = true;
    session.refresh().catch(() => undefined).finally(() => { if (alive) setBooted(true); });
    return () => { alive = false; };
  }, [booted]);

  // Proactive refresh one minute before the access token expires.
  const expiresAt = sInfo?.expiresAt;
  useEffect(() => {
    if (!expiresAt) return;
    const ms = Math.max(5_000, expiresAt - Date.now() - 60_000);
    const t = setTimeout(() => { session.refresh().catch(() => undefined); }, ms);
    return () => clearTimeout(t);
  }, [expiresAt]);

  const login = useCallback(async (email: string, password: string) => {
    const r = await api('/auth/login', { method: 'POST', body: { email, password }, schema: authResponse, auth: false });
    session.set(r);
  }, []);
  const loginWithGoogle = useCallback(async (idToken: string) => {
    const r = await api('/auth/google', { method: 'POST', body: { idToken }, schema: authResponse, auth: false });
    session.set(r);
  }, []);
  const register = useCallback(async (input: { email: string; password: string; fullName: string }) => {
    const r = await api('/auth/register', { method: 'POST', body: input, schema: registerResponse, auth: false });
    return r.userId;
  }, []);
  const verifyEmail = useCallback(async (token: string) => {
    await api('/auth/verify-email', { method: 'POST', body: { token }, auth: false });
  }, []);
  const logout = useCallback(async () => {
    const rt = session.refreshToken;
    session.clear();
    qc.clear();
    if (rt) await api('/auth/logout', { method: 'POST', body: { refreshToken: rt }, auth: false }).catch(() => undefined);
  }, [qc]);

  const status: Status = !booted ? 'loading' : sInfo ? 'authed' : 'anon';
  const value = useMemo<AuthCtx>(
    () => ({ status, userId: sInfo?.userId ?? null, email: sInfo?.email ?? null, login, loginWithGoogle, register, verifyEmail, logout }),
    [status, sInfo, login, loginWithGoogle, register, verifyEmail, logout],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useAuth() {
  const c = useContext(Ctx);
  if (!c) throw new Error('useAuth outside AuthProvider');
  return c;
}

export function useProfile() {
  const { userId } = useAuth();
  return useQuery({
    queryKey: ['profile', userId],
    enabled: !!userId,
    queryFn: () => api('/me', { schema: profile }),
    staleTime: 60_000,
  });
}
