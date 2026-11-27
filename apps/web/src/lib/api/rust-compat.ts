/**
 * Adapter between the ported warptalk-web UI (written against the .NET gateway) and the Rust API.
 *
 * The old screens and services keep calling the paths and reading the shapes they were written
 * for; this module rewrites the request on the way out and the response on the way back:
 * - paths: `/auth/me` -> `/me`, `/auth/settings` -> `/me/settings`, ... (`PATH_MAP`),
 * - auth: the Rust API returns the refresh token in the body (no HttpOnly cookie), and its
 *   AuthResponse has no `user`, so the user is read from `/me` and attached here,
 * - errors: Rust answers `{ code, message }`; old code reads `error` too, so it is mirrored.
 *
 * Endpoints the Rust API does not have are left alone: they come back 404 and the screen shows
 * its own error state. Screens that only make sense with such a backend are hidden from the nav.
 */
import axios, { type AxiosInstance, type AxiosResponse, type InternalAxiosRequestConfig } from "axios";
import { broadcastChat, knownChatTranslation, toLegacyChatMessage, type RustChatMessage } from "@/lib/realtime/meeting-chat";
import type { AuthResponse, UserDto } from "@/types/auth";
import { useWorkspaceStore } from "@/stores/workspace-store";

/** Backend origin WITHOUT `/api/v1`, same rule as the v2 app (`src/v2/lib/env.ts`). */
export function rustApiBaseUrl(): string {
  const exported = process.env.NEXT_PUBLIC_STATIC_EXPORT === "1";
  if (!exported) return "/api/v1";
  const native = typeof window === "undefined"
    ? undefined
    : (window as unknown as { warptalkNative?: { config?: { apiBaseUrl?: string } } }).warptalkNative?.config?.apiBaseUrl;
  const origin = (native || process.env.NEXT_PUBLIC_API_BASE_URL || "").replace(/\/+$/, "");
  // The macOS app serves the export as `warptalk://app/` and proxies `/api/` itself. axios's XHR
  // adapter rejects any scheme outside http/https/file/blob/data ("Unsupported protocol warptalk:")
  // before sending, which surfaced as "Cannot reach the WarpTalk server". A relative path resolves
  // against the page's own origin, so it reaches the same proxy without naming the scheme.
  if (/^warptalk:/i.test(origin)) return "/api/v1";
  return `${origin}/api/v1`;
}

// ── refresh token (body-based on the Rust API) ─────────────────────────────

const REFRESH_KEY = "warptalk.refreshToken";

export function readRefreshToken(): string | null {
  try {
    return localStorage.getItem(REFRESH_KEY);
  } catch {
    return null;
  }
}

export function writeRefreshToken(token: string | null) {
  try {
    if (token) localStorage.setItem(REFRESH_KEY, token);
    else localStorage.removeItem(REFRESH_KEY);
  } catch {
    // Private mode: the session then lasts one access-token lifetime, which is still usable.
  }
}

// ── LiveKit server ─────────────────────────────────────────────────────────

let liveKitUrl: string | null = null;

/**
 * The LiveKit server to connect to. The Rust API hands it out with every meeting token
 * (`serverUrl`); the old build baked it in as NEXT_PUBLIC_LIVEKIT_URL, kept as the fallback.
 */
export function liveKitServerUrl(): string | undefined {
  const url = liveKitUrl ?? process.env.NEXT_PUBLIC_LIVEKIT_URL;
  if (!url) return undefined;
  return typeof window === "undefined" ? url : url.replace("localhost", window.location.hostname);
}

// ── path mapping ───────────────────────────────────────────────────────────

const PATH_MAP: Array<[RegExp, string]> = [
  [/^\/auth\/me$/, "/me"],
  [/^\/auth\/settings$/, "/me/settings"],
  [/^\/auth\/profile$/, "/me/profile"],
  [/^\/workspaces\/invitations\/(pending|accept)$/, "/invitations/$1"],
  [/^\/workspaces\/invitations\/([^/]+)\/accept$/, "/invitations/$1/accept"],
  [/^\/workspaces\/([^/]+)\/members\/transfer-ownership$/, "/workspaces/$1/transfer-ownership"],
  [/^\/workspaces\/([^/]+)\/invitations\/([^/]+)\/retry-delivery$/, "/workspaces/$1/invitations/$2/resend"],
  [/^\/meetings\/rooms\/([^/]+)\/(join|end|bridge-token)$/, "/meetings/$1/$2"],
];

/** Request-body renames, keyed by the mapped path. */
const BODY_MAP: Array<[RegExp, (body: Record<string, unknown>) => Record<string, unknown>]> = [
  [/^\/workspaces\/[^/]+\/members\/[^/]+\/role$/, ({ roleName, ...rest }) => ({ role: roleName, ...rest })],
  [/^\/workspaces\/[^/]+\/invitations$/, ({ roleName, role, ...rest }) => ({ role: role ?? roleName, ...rest })],
  [/^\/workspaces\/[^/]+\/transfer-ownership$/, ({ newOwnerUserId, newOwnerId, ...rest }) => ({ newOwnerId: newOwnerId ?? newOwnerUserId, ...rest })],
  [/^\/translation-rooms\/join$/, ({ translationRoomCode, roomCode, ...rest }) => ({ roomCode: roomCode ?? translationRoomCode, ...rest })],
  // A bare Meet code (desktop auto-detect) is a Google Meet `code` on Rust.
  [/^\/translation-rooms\/bridge\/claim$/, ({ meetCode, provider, code, ...rest }) => ({
    ...rest,
    provider: provider ?? "google_meet",
    code: code ?? meetCode,
  })],
  // Rust takes `settings` as-is; the top-level copy keeps older API builds working.
  [/^\/translation-rooms$/, (body) => ({
    ...body,
    requiresApproval: body.requiresApproval ?? (body.settings as { requiresApproval?: boolean } | undefined)?.requiresApproval,
  })],
];

/**
 * Field aliases the old DTOs expect, added next to the Rust names (never replacing them):
 * `myRole` -> `role` (WorkspaceDto), `total` -> `totalCount` (the admin paging shape).
 */
const titleCase = (s: string) => (s ? s[0].toUpperCase() + s.slice(1).toLowerCase() : s);

function addLegacyAliases(data: unknown): unknown {
  if (Array.isArray(data)) return data.map(addLegacyAliases);
  if (!data || typeof data !== "object" || Object.getPrototypeOf(data) !== Object.prototype) return data;
  const o = data as Record<string, unknown>;
  for (const k of Object.keys(o)) o[k] = addLegacyAliases(o[k]);
  if ("myRole" in o && !("role" in o)) o.role = o.myRole;
  if ("total" in o && "items" in o && !("totalCount" in o)) o.totalCount = o.total;
  // MemberDto -> the old WorkspaceMemberDto names ("OWNER" -> "Owner", "internal" -> "Internal").
  if ("userId" in o && "joinedAt" in o && typeof o.role === "string" && !("roleName" in o)) {
    o.id ??= o.userId;
    o.roleName = titleCase(o.role);
    o.membershipType = typeof o.membershipType === "string" ? titleCase(o.membershipType) : "Internal";
    o.status ??= "Active";
  }
  // RoomDto -> the old TranslationRoomDto names.
  if ("roomCode" in o && "hostId" in o) {
    o.translationRoomCode ??= o.roomCode;
    o.translationRoomType ??= o.roomType;
    // External rooms carry the call link under `bridge`; the old screens read it flat.
    o.externalMeetingUrl ??= (o.bridge as { joinUrl?: string | null } | null | undefined)?.joinUrl ?? null;
    o.maxParticipants ??= 50;
    o.settings ??= { requiresApproval: Boolean(o.requiresApproval) };
  }
  return o;
}

// ── rooms ──────────────────────────────────────────────────────────────────

type Params = Record<string, unknown>;
type LegacyRoom = { id: string; status: string; title: string; scheduledAt?: string | null; createdAt: string };
type RoomPage = { rooms: LegacyRoom[]; total: number; page: number; pageSize: number };

const ROOM_LIST = /^\/translation-rooms$/;
const VALID_STATUS = new Set(["SCHEDULED", "WAITING", "OPEN", "IN_PROGRESS", "PAUSED", "ENDED", "CANCELLED", "EXPIRED", "FAILED"]);
const PAST = ["ENDED", "CANCELLED", "EXPIRED", "FAILED"];

/** The workspace a screen is in, for list calls that were scoped server-side by the old token. */
function withWorkspace(params: Params | undefined): Params {
  const p = { ...(params ?? {}) };
  if (!p.workspaceId) {
    const id = useWorkspaceStore.getState().activeWorkspaceId;
    if (id) p.workspaceId = id;
  }
  return p;
}

/**
 * The Rust list takes one exact status and no search/date range; the old screens send status
 * lists ("in_progress,waiting"), search and from/to. Those are applied here over the first 100
 * rooms, which is the most a workspace list asks for at once anyway.
 */
function filterRooms(page: RoomPage, wanted: Params): RoomPage {
  const statuses = String(wanted.status ?? "").split(",").map((v) => v.trim().toUpperCase()).filter(Boolean);
  const search = String(wanted.search ?? "").trim().toLowerCase();
  const from = wanted.from ? Date.parse(String(wanted.from)) : NaN;
  const to = wanted.to ? Date.parse(String(wanted.to)) : NaN;
  const rooms = page.rooms.filter((r) => {
    if (statuses.length && !statuses.includes(r.status.toUpperCase())) return false;
    if (search && !r.title.toLowerCase().includes(search)) return false;
    const at = Date.parse(r.scheduledAt ?? r.createdAt);
    if (!Number.isNaN(from) && at < from) return false;
    if (!Number.isNaN(to) && at > to) return false;
    return true;
  });
  return { ...page, rooms, total: rooms.length };
}

async function listRooms(client: AxiosInstance, params: Params): Promise<RoomPage> {
  const { data } = await client.get<RoomPage>("/translation-rooms", { params });
  return data;
}

// ── transcript + summary (Rust: /transcripts/{roomId}, /meetings/{roomId}/summary) ──────────

type RustSegment = {
  id: string;
  speakerParticipantId?: string | null;
  speakerName: string;
  originalText: string;
  originalLanguage: string;
  startTimeMs?: number | null;
  endTimeMs?: number | null;
  confidence?: number | null;
  sequenceOrder: number;
  isCorrected: boolean;
  translation?: { language: string; text: string; isStale: boolean } | null;
};
type RustTranscript = {
  id: string;
  workspaceId: string;
  translationRoomId: string;
  version: number;
  status: string;
  sourceLanguage: string;
  totalSegments: number;
  totalDurationMs: number;
  finalizedAt?: string | null;
  createdAt: string;
  segments: RustSegment[];
};

/** The Rust transcript is read by ROOM; the old screens page it by transcript id. */
const transcriptRoom = new Map<string, string>();

async function rustTranscript(client: AxiosInstance, roomId: string, lang?: string): Promise<RustTranscript> {
  const { data } = await client.get<RustTranscript>(`/transcripts/${roomId}`, { params: lang ? { lang } : undefined });
  transcriptRoom.set(data.id, roomId);
  return data;
}

function page<T>(items: T[], params: Params): { items: T[]; totalCount: number } {
  const skip = Number(params.skip ?? 0) || 0;
  const take = Number(params.take ?? items.length) || items.length;
  return { items: items.slice(skip, skip + take), totalCount: items.length };
}

function roomOfTranscript(id: string): string {
  const room = transcriptRoom.get(id);
  if (!room) throw compatError(404, "NOT_FOUND", "transcript not loaded");
  return room;
}

type RustSummary = {
  status: string;
  latest?: RustSummaryView | null;
  lastCompleted?: RustSummaryView | null;
};
type RustSummaryView = {
  id: string;
  status: string;
  createdAt: string;
  completedAt?: string | null;
  content?: {
    summaryShort: string;
    summaryDetailed: string;
    decisions: string[];
    actionItems: Array<{ task: string; owner?: string | null; due?: string | null }>;
    openQuestions: string[];
    topics: string[];
    language: string;
  } | null;
};

/** The Rust summary as the old `summary_export` artifact (its `content` is the summary JSON). */
async function summaryArtifact(client: AxiosInstance, roomId: string): Promise<unknown[]> {
  try {
    const { data } = await client.get<RustSummary>(`/meetings/${roomId}/summary`);
    const view = data.latest?.content ? data.latest : data.lastCompleted;
    const pending = data.status === "pending" || data.status === "running";
    if (!view?.content) {
      return pending
        ? [{ id: `summary-${roomId}`, translationRoomId: roomId, type: "SUMMARY_EXPORT", title: "Summary", status: "PROCESSING", containsRawAudio: false, containsRawVideo: false, consentRequired: false, createdAt: data.latest?.createdAt ?? new Date().toISOString() }]
        : [];
    }
    const c = view.content;
    const summary = [c.summaryShort, c.summaryDetailed].filter((s) => s?.trim()).join("\n\n");
    const content = {
      summary,
      templateKey: "general",
      summaryLanguage: c.language || undefined,
      insufficientData: !summary.trim() && !c.decisions.length && !c.actionItems.length,
      topics: c.topics,
      decisions: c.decisions,
      actionItems: c.actionItems.map((a) => ({ text: a.task, owner: a.owner ?? "" })),
      openQuestions: c.openQuestions,
    };
    return [{
      id: view.id,
      translationRoomId: roomId,
      type: "SUMMARY_EXPORT",
      title: "Summary",
      status: "COMPLETED",
      containsRawAudio: false,
      containsRawVideo: false,
      consentRequired: false,
      createdAt: view.completedAt ?? view.createdAt,
      updatedAt: view.completedAt ?? null,
      content: JSON.stringify(content),
    }];
  } catch {
    return [];
  }
}

/** history / my-meetings: room + roster + artifacts per row; Rust has rooms only. */
async function roomHistory(client: AxiosInstance, params: Params, pastOnly: boolean): Promise<unknown> {
  const status = params.status ?? (pastOnly ? PAST.join(",") : undefined);
  const page = await listRooms(client, { ...params, status });
  const rooms = await Promise.all(
    page.rooms.map(async (room) => ({
      room,
      participants: [],
      artifacts: room.status.toUpperCase() === "ENDED" ? await summaryArtifact(client, room.id) : [],
    })),
  );
  return { ...page, rooms };
}

function splitUrl(url: string): { path: string; query: string } {
  const i = url.indexOf("?");
  return i < 0 ? { path: url, query: "" } : { path: url.slice(0, i), query: url.slice(i) };
}

export function mapPath(url: string): string {
  const { path, query } = splitUrl(url);
  for (const [from, to] of PATH_MAP) if (from.test(path)) return `${path.replace(from, to)}${query}`;
  return url;
}

// ── response shapes ────────────────────────────────────────────────────────

type RustAuth = {
  userId: string;
  email: string;
  accessToken: string;
  accessTokenExpiresAt: string;
  refreshToken: string;
};

type RustMe = {
  userId: string;
  email: string;
  fullName: string;
  avatarUrl?: string | null;
  preferredLanguage?: string;
  timezone?: string;
  emailVerified: boolean;
  systemAdmin: boolean;
};

export function meToUser(me: RustMe): UserDto {
  return {
    id: me.userId,
    email: me.email,
    fullName: me.fullName,
    avatarUrl: me.avatarUrl ?? undefined,
    preferredLanguage: me.preferredLanguage,
    timezone: me.timezone,
    emailVerified: me.emailVerified,
    roles: me.systemAdmin ? ["admin"] : ["user"],
    hasPassword: true,
  };
}

function isRustAuth(data: unknown): data is RustAuth {
  return Boolean(data && typeof data === "object" && "accessToken" in data && "refreshToken" in data);
}

function isRustMe(data: unknown): data is RustMe {
  return Boolean(data && typeof data === "object" && "userId" in data && "systemAdmin" in data);
}

/** Turn a Rust AuthResponse into the old one, reading the user from `/me` with the new token. */
export async function toLegacyAuth(data: RustAuth, baseURL: string): Promise<AuthResponse> {
  writeRefreshToken(data.refreshToken);
  const { data: me } = await axios.get<RustMe>(`${baseURL}/me`, {
    headers: { Authorization: `Bearer ${data.accessToken}` },
  });
  return { accessToken: data.accessToken, expiresAt: data.accessTokenExpiresAt, user: meToUser(me) };
}

type RustWorkspace = { id: string; name: string; slug: string; myRole: string; defaultLanguage?: string | null };

/**
 * Endpoints the .NET gateway had and the Rust API answers differently, served by composing Rust
 * calls instead. Keyed by method + mapped path; each returns the old response body.
 */
type Body = Record<string, unknown> | undefined;
const SYNTHETIC: Array<[string, RegExp, (client: AxiosInstance, m: RegExpExecArray, params: Params, body: Body) => Promise<unknown>]> = [
  // Selecting a workspace minted a workspace-scoped token on .NET; Rust tokens are user-scoped,
  // so "select" is just reading the workspace.
  ["post", /^\/workspaces\/([^/]+)\/select$/, async (client, m) => {
    const { data } = await client.get<RustWorkspace>(`/workspaces/${m[1]}`);
    // Must match what the workspace layout derives from the list row (role, "Internal",
    // defaultLanguage || "en"), or it re-selects on every render.
    return {
      selectedWorkspaceId: data.id,
      name: data.name,
      slug: data.slug,
      role: data.myRole,
      membershipType: "Internal",
      defaultLanguage: data.defaultLanguage || "en",
      canCreateMeetings: true,
    };
  }],
  // No billing on the Rust API yet: every workspace behaves as an active, unmetered plan, so the
  // old plan gates stay open instead of locking features behind a 404.
  ["get", /^\/subscriptions\/workspace\/([^/]+)$/, async (_c, m) => ({
    id: `local-${m[1]}`, userId: null, workspaceId: m[1], planId: "unmetered", planName: "WarpTalk",
    price: 0, status: "active", creditsRemaining: 1_000_000, creditsUsedThisCycle: 0,
    currentPeriodStart: new Date().toISOString(), currentPeriodEnd: new Date(Date.now() + 365 * 864e5).toISOString(),
    autoRenew: true, cancelAtPeriodEnd: false, createdAt: new Date().toISOString(), cancelledAt: null,
  })],
  ["get", /^\/credits\/workspace\/([^/]+)$/, async (_c, m) => ({
    workspaceId: m[1], currentCredits: 1_000_000, creditsUsedThisCycle: 0, totalCredits: 1_000_000, status: "active",
    currentPeriodStart: new Date().toISOString(), currentPeriodEnd: new Date(Date.now() + 365 * 864e5).toISOString(),
  })],
  ["get", /^\/credits\/workspace\/([^/]+)\/frozen$/, async (_c, m) => ({ workspaceId: m[1], frozenCredits: 0, frozenAt: null, endedAt: null, dormantSince: null, graceEndsAt: null, hasActiveSubscription: true })],
  // No notification feed or platform switchboard yet: empty, and maintenance off.
  ["get", /^\/notifications$/, async () => ({ items: [], totalCount: 0, unreadCount: 0, page: 1, pageSize: 20 })],
  ["get", /^\/notifications\/announcements$/, async () => []],
  ["get", /^\/platform\/status$/, async () => ({ maintenance: { enabled: false } })],
  ["get", /^\/assistant\/plugins$/, async () => []],
  // Meeting chat: the Rust routes live under /meetings/{id}/chat/messages with their own DTO.
  ["get", /^\/meetings\/rooms\/([^/]+)\/chat$/, async (client, m) => {
    const { data } = await client.get<RustChatMessage[]>(`/meetings/${m[1]}/chat/messages`, { params: { limit: 100 } });
    return data
      .filter((msg) => !msg.hidden)
      .sort((a, b) => a.createdAt.localeCompare(b.createdAt))
      .map((msg) => toLegacyChatMessage(msg, m[1]));
  }],
  ["post", /^\/meetings\/rooms\/([^/]+)\/chat$/, async (client, m, _p, body) => {
    const content = String(body?.originalText ?? body?.content ?? "");
    const { data } = await client.post<RustChatMessage>(`/meetings/${m[1]}/chat/messages`, {
      content,
      clientMessageId: crypto.randomUUID(),
    });
    broadcastChat({ type: "chat.message", ...data });
    return toLegacyChatMessage(data, m[1]);
  }],
  // Chat is translated server-side and arrives with the message or as `chat.translated`; there
  // is no on-demand translation, so this answers from what is known or reports it as not ready.
  ["post", /^\/meetings\/rooms\/([^/]+)\/chat\/([^/]+)\/translate$/, async (client, m, _p, body) => {
    const target = String(body?.targetLanguage ?? "");
    let text = knownChatTranslation(m[2], target);
    if (!text) {
      const { data } = await client.get<RustChatMessage[]>(`/meetings/${m[1]}/chat/messages`, { params: { limit: 100 } });
      for (const msg of data) toLegacyChatMessage(msg, m[1]);
      text = knownChatTranslation(m[2], target);
    }
    if (!text) throw compatError(404, "TRANSLATION_NOT_READY", "This message has no translation yet");
    return { messageId: m[2], targetLanguage: target, translatedText: text, cached: true };
  }],
  ["post", /^\/meetings\/rooms\/([^/]+)\/chat\/([^/]+)\/moderate$/, async (client, m, _p, body) => {
    await client.post(`/meetings/${m[1]}/chat/messages/${m[2]}/hide`, { reason: body?.reason ?? null });
    broadcastChat({ type: "chat.hidden", messageId: m[2] });
    return null;
  }],
  // Room invitations ride on the room for its host (`invitedEmails`); the old screens list them.
  ["get", /^\/translation-rooms\/([^/]+)\/invitations$/, async (client, m) => {
    const { data } = await client.get<{ invitedEmails?: string[]; createdAt: string; updatedAt?: string }>(`/translation-rooms/${m[1]}`);
    return (data.invitedEmails ?? []).map((email) => ({
      id: `${m[1]}:${email}`, translationRoomId: m[1], email, status: "PENDING",
      createdAt: data.createdAt, updatedAt: data.updatedAt ?? data.createdAt,
    }));
  }],
  ["get", /^\/translation-rooms\/history$/, (client, _m, params) => roomHistory(client, params, true)],
  ["get", /^\/translation-rooms\/my-meetings$/, (client, _m, params) => roomHistory(client, params, false)],
  ["get", /^\/transcripts\/by-room\/([^/]+)\/pause-windows$/, async () => []],
  ["get", /^\/translation-rooms\/([^/]+)\/sessions$/, async () => []],
  ["get", /^\/transcripts\/by-room\/([^/]+)$/, async (client, m) => {
    const { segments: _s, ...t } = await rustTranscript(client, m[1]);
    return { ...t, updatedAt: t.finalizedAt ?? t.createdAt, timelineAnchorAt: null };
  }],
  ["get", /^\/transcripts\/([^/]+)\/segments$/, async (client, m, params) => {
    const t = await rustTranscript(client, roomOfTranscript(m[1]));
    const items = t.segments.map((s) => ({
      id: s.id,
      speakerParticipantId: s.speakerParticipantId ?? undefined,
      speakerName: s.speakerName,
      originalText: s.originalText,
      originalLanguage: s.originalLanguage,
      confidence: s.confidence ?? undefined,
      startTimeMs: s.startTimeMs ?? 0,
      endTimeMs: s.endTimeMs ?? s.startTimeMs ?? 0,
      sequenceOrder: s.sequenceOrder,
      isCorrected: s.isCorrected,
    }));
    return page(items, params);
  }],
  // One Rust read per language the room was translated into.
  ["get", /^\/transcripts\/([^/]+)\/translations$/, async (client, m, params) => {
    const roomId = roomOfTranscript(m[1]);
    const { data: room } = await client.get<{ targetLanguages?: string[]; sourceLanguage?: string }>(`/translation-rooms/${roomId}`);
    const langs = [...new Set((room.targetLanguages ?? []).filter((l) => l && l !== room.sourceLanguage))];
    const reads = await Promise.all(langs.map((lang) => rustTranscript(client, roomId, lang).catch(() => null)));
    const items = reads.flatMap((t) =>
      (t?.segments ?? []).flatMap((s) =>
        s.translation && s.translation.text.trim() && s.translation.language !== s.originalLanguage
          ? [{ id: `${s.id}:${s.translation.language}`, segmentId: s.id, targetLanguage: s.translation.language, translatedText: s.translation.text, translatorModel: "", isRetranslated: false }]
          : [],
      ),
    );
    return page(items, params);
  }],
];

/** Request bodies reach the adapter already serialized by axios. */
function parseBody(data: unknown): Body {
  if (typeof data === "string") {
    try {
      const v: unknown = JSON.parse(data);
      return v && typeof v === "object" ? (v as Record<string, unknown>) : undefined;
    } catch {
      return undefined;
    }
  }
  return data && typeof data === "object" ? (data as Record<string, unknown>) : undefined;
}

/** An error shaped like an API error response, for answers the compat layer refuses itself. */
function compatError(status: number, code: string, message: string): Error {
  return Object.assign(new Error(message), {
    isAxiosError: true,
    response: { status, data: { code, message, error: message } },
  });
}

const AUTH_ISSUING = /\/auth\/(login|register|refresh)$/;

/** Install the request/response rewrites on the old axios instance. Call before its own interceptors. */
export function installRustCompat(client: AxiosInstance) {
  client.interceptors.request.use((config: InternalAxiosRequestConfig) => {
    if (config.url) config.url = mapPath(config.url);
    const mapped = splitUrl(config.url ?? "").path;
    const isPlainBody = config.data && typeof config.data === "object" && Object.getPrototypeOf(config.data) === Object.prototype;
    const method = (config.method ?? "get").toLowerCase();
    for (const [verb, re, run] of SYNTHETIC) {
      const m = re.exec(mapped);
      if (m && verb === method) {
        config.adapter = async (cfg) => ({ data: await run(client, m, (cfg.params ?? {}) as Params, parseBody(cfg.data)), status: 200, statusText: "OK", headers: {}, config: cfg });
        break;
      }
    }
    for (const [re, fn] of BODY_MAP) if (isPlainBody && re.test(mapped)) config.data = fn(config.data);
    if (method === "get" && ROOM_LIST.test(mapped)) {
      const wanted = withWorkspace(config.params as Params | undefined);
      const exact = String(wanted.status ?? "").toUpperCase();
      const needsLocal = Boolean((wanted.status && !VALID_STATUS.has(exact)) || wanted.search || wanted.from || wanted.to);
      // eslint-disable-next-line @typescript-eslint/no-unused-vars -- dropped from the request
      const { status: _s, search: _q, from: _f, to: _t, groupBySeries: _g, scope: _sc, ...rest } = wanted;
      config.params = needsLocal
        ? { ...rest, page: 1, pageSize: 100 }
        : { ...rest, ...(exact ? { status: exact } : {}) };
      if (needsLocal) (config as { legacyFilter?: Params }).legacyFilter = wanted;
    }
    // Logout revokes the refresh token, which the Rust API takes in the body.
    if (config.url && splitUrl(config.url).path.endsWith("/auth/logout")) {
      config.data = { refreshToken: readRefreshToken() ?? "" };
      writeRefreshToken(null);
    }
    return config;
  });

  client.interceptors.response.use(
    async (response: AxiosResponse) => {
      const path = splitUrl(response.config.url ?? "").path;
      if (AUTH_ISSUING.test(path) && isRustAuth(response.data)) {
        response.data = await toLegacyAuth(response.data, client.defaults.baseURL ?? "");
      } else if (/\/me$/.test(path) && isRustMe(response.data)) {
        response.data = meToUser(response.data);
      } else {
        response.data = addLegacyAliases(response.data);
      }
      const method = (response.config.method ?? "get").toLowerCase();
      const body = response.data as Record<string, unknown> | null;
      if (method === "get" && ROOM_LIST.test(path) && body && Array.isArray(body.items)) {
        const page: RoomPage = { rooms: body.items as LegacyRoom[], total: Number(body.total), page: Number(body.page), pageSize: Number(body.pageSize) };
        const wanted = (response.config as { legacyFilter?: Params }).legacyFilter;
        response.data = wanted ? filterRooms(page, wanted) : page;
      }
      // Meeting tokens: remember the server, and give the room name its old key.
      if (/^\/meetings\/[^/]+\/(?:join|bridge-token)$/.test(path) && body && typeof body.token === "string") {
        if (typeof body.serverUrl === "string" && body.serverUrl) liveKitUrl = body.serverUrl;
        body.providerRoomName ??= body.roomName;
      }
      // A bridge claim answers the room flat with the lease beside it; the old screens read { room, ... }.
      if (method === "post" && /^\/translation-rooms\/bridge\/claim$/.test(path) && body && "id" in body && !("room" in body)) {
        const { created, bridgeRole, heartbeatIntervalSeconds, leaseSeconds, leaseExpiresAt, serverTime, ...room } = body;
        response.data = {
          room,
          participant: null,
          created,
          // Rust says `viewer` for a seat that does not capture; the old client calls it `member`.
          bridgeRole: bridgeRole === "capturer" ? "capturer" : "member",
          capturerHeartbeatIntervalSeconds: heartbeatIntervalSeconds,
          capturerLeaseSeconds: leaseSeconds,
          leaseExpiresAt,
          serverTime,
        };
      }
      // Join answers the participant flat with a room summary; the old screens read { room, participant }.
      if (method === "post" && /^\/translation-rooms\/(?:[^/]+\/)?join$/.test(path) && body && "roomId" in body && !("participant" in body)) {
        // eslint-disable-next-line @typescript-eslint/no-unused-vars -- dropped from the request
        const { roomId, room: _summary, ...participant } = body;
        const { data: room } = await client.get(`/translation-rooms/${String(roomId)}`);
        response.data = { room, participant: { ...participant, translationRoomId: roomId } };
      }
      return response;
    },
    (error: unknown) => {
      if (axios.isAxiosError(error)) {
        const body = error.response?.data as { code?: string; message?: string; error?: string } | undefined;
        if (body && typeof body === "object" && body.message && !body.error) body.error = body.message;
        // Old screens branch on .NET codes; the one with a dedicated flow is the unverified sign-in.
        if (body?.code === "EMAIL_NOT_VERIFIED") body.code = "ACCOUNT_PENDING";
      }
      return Promise.reject(error);
    },
  );
}
