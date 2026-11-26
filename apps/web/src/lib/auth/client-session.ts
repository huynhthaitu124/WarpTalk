/**
 * Client-side session checks used by the route gate (src/LegacyRoot.tsx), which replaces the old
 * server middleware. Same rules as the old `proxy.ts`: a live access-token cookie or the session
 * marker counts as signed in, unless the client has declared the session dead.
 */
import { ACCESS_TOKEN_COOKIE, SESSION_DEAD_COOKIE, hasRedeemableSession, isLiveAccessToken } from "@/lib/auth/session-cookie";
import { isPlatformAdminToken } from "@/lib/api/token-lifecycle";
import { ADMIN_CONSOLE_ENABLED } from "@/lib/feature-flags";
import { normalizeWorkspaceSlug } from "@/lib/workspace/workspace-slug";

function cookie(name: string): string | null {
  for (const part of document.cookie.split("; ")) if (part.startsWith(`${name}=`)) return decodeURIComponent(part.slice(name.length + 1));
  return null;
}

export function hasClientSession(): boolean {
  if (cookie(SESSION_DEAD_COOKIE)) return false;
  const token = cookie(ACCESS_TOKEN_COOKIE);
  return isLiveAccessToken(token) || hasRedeemableSession(null);
}

export function landingPathForSession(): string {
  const slug = normalizeWorkspaceSlug(cookie("active_workspace_slug"));
  if (slug) return `/${slug}/dashboard`;
  if (ADMIN_CONSOLE_ENABLED && isPlatformAdminToken(cookie(ACCESS_TOKEN_COOKIE))) return "/admin";
  return "/workspace";
}
