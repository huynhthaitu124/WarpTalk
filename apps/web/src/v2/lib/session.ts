// Session store: access token in memory, refresh token in localStorage (rotated on every refresh).
import { authResponse, type AuthResponse } from './schemas';
import { apiUrl } from '@v2/lib/env';

// Shared with the legacy UI (`rust-compat.ts` REFRESH_KEY): both roots run in the same app, and the
// refresh token rotates, so they must read and write one copy or the second root sees no session.
const RT_KEY = 'warptalk.refreshToken';

type Listener = () => void;

export interface SessionInfo {
  userId: string;
  email: string;
  expiresAt: number;
}

let accessToken: string | null = null;
/** Bumped on every set()/clear(): lets callers tell whether the session rotated while a request was in flight. */
let generation = 0;
let info: SessionInfo | null = null;
const listeners = new Set<Listener>();
let inflight: Promise<AuthResponse | null> | null = null;

function safeGet(): string | null {
  try { return localStorage.getItem(RT_KEY); } catch { return null; }
}
function safeSet(v: string | null) {
  try {
    if (v === null) localStorage.removeItem(RT_KEY);
    else localStorage.setItem(RT_KEY, v);
  } catch { /* storage unavailable: session lives in memory only */ }
}

export const session = {
  get accessToken() { return accessToken; },
  get generation() { return generation; },
  get info() { return info; },
  get refreshToken() { return safeGet(); },
  subscribe(l: Listener) { listeners.add(l); return () => { listeners.delete(l); }; },
  set(r: AuthResponse) {
    generation += 1;
    accessToken = r.accessToken;
    info = { userId: r.userId, email: r.email, expiresAt: Date.parse(r.accessTokenExpiresAt) };
    safeSet(r.refreshToken);
    listeners.forEach((l) => l());
  },
  clear() {
    generation += 1;
    accessToken = null;
    info = null;
    safeSet(null);
    listeners.forEach((l) => l());
  },
  /**
   * Single-flight refresh with rotation: concurrent callers share one request, so the
   * rotated refresh token is never spent twice. Resolves null when there is no valid session.
   * A network failure rejects and keeps the stored refresh token.
   */
  refresh(): Promise<AuthResponse | null> {
    if (inflight) return inflight;
    const rt = safeGet();
    if (!rt) return Promise.resolve(null);
    inflight = (async () => {
      try {
        const res = await fetch(apiUrl('/auth/refresh'), {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ refreshToken: rt }),
        });
        if (res.status === 401 || res.status === 403 || res.status === 400) {
          // A parallel tab may already have rotated: if storage changed, use it next time.
          if (safeGet() !== rt) return null;
          session.clear();
          return null;
        }
        if (!res.ok) throw new Error(`refresh failed: ${res.status}`);
        const parsed = authResponse.parse(await res.json());
        session.set(parsed);
        return parsed;
      } finally {
        inflight = null;
      }
    })();
    return inflight;
  },
};
