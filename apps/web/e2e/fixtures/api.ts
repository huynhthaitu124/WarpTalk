// Direct API helpers (node fetch) used to arrange state quickly; the UI is what the tests assert on.
import { randomBytes } from 'node:crypto';
import { execFileSync } from 'node:child_process';

export const API = process.env.E2E_API ?? 'http://127.0.0.1:18080/api/v1';

export interface TestUser { email: string; password: string; fullName: string; userId: string }
export interface Tokens { accessToken: string; refreshToken: string }

export const rnd = () => randomBytes(4).toString('hex');

export async function call<T = unknown>(path: string, o: { method?: string; token?: string; body?: unknown } = {}): Promise<{ status: number; body: T }> {
  const res = await fetch(`${API}${path}`, {
    method: o.method ?? (o.body ? 'POST' : 'GET'),
    headers: { 'content-type': 'application/json', ...(o.token ? { authorization: `Bearer ${o.token}` } : {}) },
    body: o.body === undefined ? undefined : JSON.stringify(o.body),
  });
  const text = await res.text();
  return { status: res.status, body: (text ? JSON.parse(text) : undefined) as T };
}

export async function ok<T>(p: Promise<{ status: number; body: T }>): Promise<T> {
  const r = await p;
  if (r.status >= 300) throw new Error(`API ${r.status}: ${JSON.stringify(r.body)}`);
  return r.body;
}

const REDIS = process.env.E2E_REDIS ?? 'redis://localhost:6379';

/**
 * Raw token the API "emailed": the test backend (DEV_MAIL_CAPTURE=1) stores it in Redis under
 * `dev:mail:{verify|workspace-invite}:{email}` for an hour. TEST BACKEND ONLY.
 */
export async function devMail(kind: 'verify' | 'workspace-invite', email: string): Promise<string> {
  const key = `dev:mail:${kind}:${email.toLowerCase()}`;
  for (let i = 0; i < 20; i++) {
    const v = execFileSync('redis-cli', ['-u', REDIS, '--no-raw', 'GET', key], { encoding: 'utf8' }).trim();
    if (v && v !== '(nil)') return v.replace(/^"|"$/g, '');
    await new Promise((r) => setTimeout(r, 150));
  }
  throw new Error(`no captured ${kind} mail for ${email} (is the API running with DEV_MAIL_CAPTURE=1?)`);
}

/** Verify an account the way a person does: read the mailed token, POST /auth/verify-email. */
export async function verifyEmail(email: string) {
  await ok(call('/auth/verify-email', { body: { token: await devMail('verify', email) } }));
}

export async function createUser(label = 'user', verified = true): Promise<TestUser> {
  const id = rnd();
  const u = { email: `e2e-${label}-${id}@example.com`, password: 'Passw0rd!e2e', fullName: `${label[0]!.toUpperCase()}${label.slice(1)} ${id}` };
  const r = await ok(call<{ userId: string }>('/auth/register', { body: u }));
  if (verified) await verifyEmail(u.email);
  return { ...u, userId: r.userId };
}

export async function login(u: TestUser): Promise<Tokens> {
  return ok(call<Tokens>('/auth/login', { body: { email: u.email, password: u.password } }));
}

export interface Ws { id: string; name: string }
export const createWorkspace = (token: string, name = `Acme ${rnd()}`) =>
  ok(call<Ws>('/workspaces', { token, body: { name } }));

export interface RoomDto { id: string; roomCode: string; title: string; status: string; workspaceId: string }
export const createRoom = (token: string, workspaceId: string, o: { title?: string; requiresApproval?: boolean; displayName?: string } = {}) =>
  ok(call<RoomDto>('/translation-rooms', {
    token,
    body: { workspaceId, title: o.title ?? `Room ${rnd()}`, sourceLanguage: 'vi', targetLanguages: ['en', 'ja'], requiresApproval: o.requiresApproval ?? false, displayName: o.displayName },
  }));

/** Join a room as `token` by code (the only join path for people outside the workspace). */
export const joinByCode = (token: string, roomCode: string, o: { displayName?: string; speak?: string; listen?: string } = {}) =>
  ok(call<{ id: string; status: string; displayName: string }>('/translation-rooms/join', {
    token,
    body: { roomCode, displayName: o.displayName, speakLanguage: o.speak ?? 'en', listenLanguage: o.listen ?? 'vi' },
  }));

export interface InvitationDto { id: string; email: string; role: string; status: string; workspaceId: string }
export const inviteMember = (token: string, workspaceId: string, email: string, role: 'MEMBER' | 'ADMIN' = 'MEMBER') =>
  ok(call<InvitationDto>(`/workspaces/${workspaceId}/invitations`, { token, body: { email, role } }));

/** Real invitation flow: owner invites, the invitee accepts with the mailed token. */
export async function inviteAndAccept(ownerToken: string, workspaceId: string, user: TestUser, role: 'MEMBER' | 'ADMIN' = 'MEMBER') {
  await inviteMember(ownerToken, workspaceId, user.email, role);
  const token = await devMail('workspace-invite', user.email);
  const { accessToken } = await login(user);
  await ok(call('/invitations/accept', { token: accessToken, body: { token } }));
}
