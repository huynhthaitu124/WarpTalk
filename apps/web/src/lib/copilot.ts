// Copilot sessions (ADR 0001): a session is a room of type COPILOT_SESSION owned by a workspace.
// Backend shapes are coded against and parsed tolerantly; a 404/501/503 means "not deployed yet" and reads as empty.
//   GET  /copilot/sessions?workspaceId=            -> CopilotSession[] | { items: CopilotSession[] }
//   GET  /copilot/sessions/{id}                    -> CopilotSession   (falls back to GET /translation-rooms/{id})
//   POST /copilot/sessions {workspaceId,title,platform,meetingUrl,languages{me,others}} -> CopilotSession
//   GET  /transcripts/{roomId}                     -> { status, segments: [{ id, speakerName, originalText, startTimeMs, sequenceOrder, isFinal }] }
//   POST /copilot/sessions/{id}/end                -> session (status ENDED, idempotent)
//   GET  /meetings/{roomId}/summary                -> { status: none|pending|running|completed|failed, latest, lastCompleted, content{summaryShort,summaryDetailed,decisions,openQuestions,...}, errorCode }
//   POST /meetings/{roomId}/summary/regenerate     -> 202 (409 SUMMARY_IN_PROGRESS)
//   GET  /meetings/{roomId}/action-items           -> { items: [{ id, task, owner, due, status: open|done }] }
//   PATCH /meetings/{roomId}/action-items/{id} {status:'open'|'done'}
//   GET  /workspaces/{id}/action-items?status=open&mine=true
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { api, ApiError } from './api';
import { room, type Room } from './schemas';

export const PLATFORMS = [
  { value: 'google_meet', label: 'Google Meet' },
  { value: 'zoom', label: 'Zoom' },
  { value: 'teams', label: 'Microsoft Teams' },
  { value: 'in_person', label: 'In person' },
  { value: 'other', label: 'Other' },
] as const;
export const platformLabel = (v?: string | null) => PLATFORMS.find((p) => p.value === v)?.label ?? (v ? v.replace(/_/g, ' ') : 'Meeting');

export interface CopilotSession {
  id: string;
  title: string;
  status: string;
  platform?: string;
  meetingUrl?: string;
  languages?: { me?: string; others?: string };
  startedAt?: string;
  endedAt?: string;
  createdAt?: string;
}

const str = (v: unknown) => (typeof v === 'string' && v ? v : undefined);
const obj = (v: unknown): Record<string, unknown> | undefined => (typeof v === 'object' && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : undefined);
const listOf = (v: unknown, ...keys: string[]): unknown[] => {
  if (Array.isArray(v)) return v;
  const o = obj(v);
  if (!o) return [];
  for (const k of ['items', ...keys]) if (Array.isArray(o[k])) return o[k] as unknown[];
  return [];
};
const GONE = new Set([404, 405, 501, 503]);
const gone = (e: unknown) => e instanceof ApiError && GONE.has(e.status);

export function parseSession(v: unknown): CopilotSession | null {
  const o = obj(v);
  if (!o) return null;
  const id = str(o.id) ?? str(o.sessionId) ?? str(o.roomId);
  if (!id) return null;
  const lang = obj(o.languages);
  return {
    id,
    title: str(o.title) ?? 'Untitled session',
    status: str(o.status) ?? 'WAITING',
    platform: str(o.platform),
    meetingUrl: str(o.meetingUrl),
    languages: lang ? { me: str(lang.me), others: str(lang.others) } : undefined,
    startedAt: str(o.startedAt),
    endedAt: str(o.endedAt),
    createdAt: str(o.createdAt),
  };
}

export const IN_PROGRESS = 'IN_PROGRESS';
export const sessionIsLive = (s?: Pick<CopilotSession, 'status'> | null) => s?.status === IN_PROGRESS;

export function useSessions(wsId: string | undefined) {
  return useQuery({
    queryKey: ['copilot-sessions', wsId],
    enabled: !!wsId,
    retry: false,
    queryFn: async (): Promise<CopilotSession[]> => {
      try {
        const json = await api('/copilot/sessions', { query: { workspaceId: wsId }, schema: z.unknown() });
        return listOf(json, 'sessions').map(parseSession).filter((s): s is CopilotSession => !!s);
      } catch (e) { if (gone(e)) return []; throw e; }
    },
  });
}

export function useSession(id: string | undefined) {
  return useQuery({
    queryKey: ['copilot-session', id],
    enabled: !!id,
    retry: false,
    // Keep polling while live so the status flips to ended and the summary appears.
    refetchInterval: (q) => (sessionIsLive(q.state.data) ? 5_000 : false),
    queryFn: async (): Promise<CopilotSession> => {
      try {
        const s = parseSession(await api(`/copilot/sessions/${id}`, { schema: z.unknown() }));
        if (s) return s;
      } catch (e) { if (!gone(e)) throw e; }
      const r: Room = await api(`/translation-rooms/${id}`, { schema: room });
      return { id: r.id, title: r.title, status: r.status, startedAt: r.startedAt ?? undefined, endedAt: r.endedAt ?? undefined, createdAt: r.createdAt };
    },
  });
}

export interface CreateSessionInput {
  workspaceId: string; title: string; platform: string; meetingUrl?: string; languages: { me: string; others: string };
}
export function useCreateSession() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (b: CreateSessionInput) => {
      const s = parseSession(await api('/copilot/sessions', { method: 'POST', body: b, schema: z.unknown() }));
      if (!s) throw new Error('The server did not return the new session.');
      return s;
    },
    onSuccess: (_s, v) => qc.invalidateQueries({ queryKey: ['copilot-sessions', v.workspaceId] }),
  });
}

// ---- transcript
export interface TranscriptLine { id: string; speaker: string; text: string; atMs?: number; final: boolean; mine: boolean }

export function parseTranscript(json: unknown): TranscriptLine[] {
  const o = obj(json);
  const lines = listOf(o?.segments ?? json, 'segments').flatMap((e, n): TranscriptLine[] => {
    const s = obj(e);
    if (!s) return [];
    const text = str(s.text) ?? str(s.originalText) ?? str(s.content);
    if (!text) return [];
    const channel = str(s.channel) ?? str(s.speaker);
    const speaker = str(s.speakerName) ?? str(s.speakerLabel) ?? (channel === 'me' ? 'Me' : channel === 'others' ? 'Others' : channel) ?? 'Speaker';
    const at = s.startTimeMs ?? s.startMs ?? s.atMs;
    return [{
      id: str(s.id) ?? str(s.segmentId) ?? `seg-${n}`,
      speaker: speaker === 'me' ? 'Me' : speaker === 'others' ? 'Others' : speaker,
      text,
      atMs: typeof at === 'number' ? at : undefined,
      final: s.isFinal !== false,
      mine: channel === 'me' || speaker === 'Me',
    }];
  });
  return lines;
}

/** Polls every 3 s while the session is live (`live`), once otherwise. */
export function useTranscript(roomId: string | undefined, live: boolean) {
  return useQuery({
    queryKey: ['transcript', roomId],
    enabled: !!roomId,
    retry: false,
    refetchInterval: live ? 3_000 : false,
    queryFn: async () => {
      try { return parseTranscript(await api(`/transcripts/${roomId}`, { schema: z.unknown() })); } catch (e) { if (gone(e)) return []; throw e; }
    },
  });
}

// ---- summary and action items
export interface MeetingSummary {
  /** `none` | `pending` | `running` | `completed` | `failed` (backend job state). */
  status: string;
  short: string;
  detailed: string;
  decisions: string[];
  openQuestions: string[];
  errorCode?: string;
  generatedAt?: string;
}

const strList = (v: unknown) => listOf(v).flatMap((d) => {
  const t = typeof d === 'string' ? d : str(obj(d)?.text) ?? str(obj(d)?.decision);
  return t ? [t] : [];
});

/**
 * `GET /meetings/{id}/summary` -> `{ status, latest, lastCompleted }`; the text lives in `content`
 * (`summaryShort`, `summaryDetailed`, `decisions[]`, `openQuestions[]`). Older flat shapes (`summary`) are accepted too.
 */
export function parseSummary(json: unknown): MeetingSummary | null {
  const o = obj(json);
  if (!o) return null;
  const content = obj(o.content) ?? obj(obj(o.lastCompleted)?.content) ?? obj(obj(o.latest)?.content) ?? o;
  const short = str(content.summaryShort) ?? str(content.summary) ?? str(content.text) ?? '';
  const detailed = str(content.summaryDetailed) ?? str(content.minutes) ?? '';
  const status = (str(o.status) ?? (short || detailed ? 'completed' : 'none')).toLowerCase();
  const latest = obj(o.latest);
  const errorCode = str(o.errorCode) ?? str(latest?.errorCode);
  if (!short && !detailed && status === 'none') return null;
  return {
    status, short, detailed, decisions: strList(content.decisions ?? content.keyDecisions), openQuestions: strList(content.openQuestions), errorCode,
    generatedAt: str(o.generatedAt) ?? str(latest?.completedAt) ?? str(obj(o.lastCompleted)?.completedAt) ?? str(o.createdAt),
  };
}

export const summaryBusy = (s?: MeetingSummary | null) => s?.status === 'pending' || s?.status === 'running';

export function useSummary(roomId: string | undefined, enabled: boolean, poll: boolean) {
  return useQuery({
    queryKey: ['summary', roomId],
    enabled: !!roomId && enabled,
    retry: false,
    refetchInterval: (q) => (summaryBusy(q.state.data) || (poll && !q.state.data) ? 4_000 : false),
    queryFn: async () => {
      try { return parseSummary(await api(`/meetings/${roomId}/summary`, { schema: z.unknown() })); } catch (e) { if (gone(e)) return null; throw e; }
    },
  });
}
export function useRegenerateSummary(roomId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      try { await api(`/meetings/${roomId}/summary/regenerate`, { method: 'POST' }); } catch (e) {
        if (e instanceof ApiError && e.code === 'SUMMARY_IN_PROGRESS') return; // already running: just keep polling
        throw e;
      }
    },
    onSuccess: () => { void qc.invalidateQueries({ queryKey: ['summary', roomId] }); void qc.invalidateQueries({ queryKey: ['action-items', roomId] }); },
  });
}

/** `POST /copilot/sessions/{id}/end` (idempotent): ends the session; the backend then writes the summary. */
export function useEndSession(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api(`/copilot/sessions/${id}/end`, { method: 'POST', schema: z.unknown() }),
    onSuccess: () => { void qc.invalidateQueries({ queryKey: ['copilot-session', id] }); void qc.invalidateQueries({ queryKey: ['copilot-sessions'] }); },
  });
}

export interface ActionItem { id: string; text: string; owner?: string; deadline?: string; done: boolean }

export function parseActionItems(json: unknown): ActionItem[] {
  return listOf(json, 'actionItems').flatMap((e, n): ActionItem[] => {
    const o = obj(e);
    if (!o) return typeof e === 'string' && e ? [{ id: `a${n}`, text: e, done: false }] : [];
    const text = str(o.task) ?? str(o.text) ?? str(o.title) ?? str(o.content) ?? str(o.description);
    if (!text) return [];
    const status = str(o.status)?.toUpperCase();
    return [{
      id: str(o.id) ?? str(o.itemId) ?? `a${n}`,
      text,
      owner: str(o.owner) ?? str(o.assignee) ?? str(o.assigneeName),
      deadline: str(o.deadline) ?? str(o.dueDate) ?? str(o.due),
      done: o.done === true || status === 'DONE' || status === 'COMPLETED' || status === 'CLOSED',
    }];
  });
}

export function useActionItems(roomId: string | undefined, enabled = true) {
  return useQuery({
    queryKey: ['action-items', roomId],
    enabled: !!roomId && enabled,
    retry: false,
    queryFn: async () => {
      try { return parseActionItems(await api(`/meetings/${roomId}/action-items`, { schema: z.unknown() })); } catch (e) { if (gone(e)) return []; throw e; }
    },
  });
}
export function useSetActionItemDone(roomId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { id: string; done: boolean }) => api(`/meetings/${roomId}/action-items/${v.id}`, { method: 'PATCH', body: { status: v.done ? 'done' : 'open' } }),
    onSuccess: () => { void qc.invalidateQueries({ queryKey: ['action-items', roomId] }); void qc.invalidateQueries({ queryKey: ['action-items', 'mine'] }); },
  });
}

/** The signed-in person's open action items across the workspace (`GET /workspaces/{id}/action-items/mine`). */
export function useMyActionItems(wsId: string | undefined) {
  return useQuery({
    queryKey: ['action-items', 'mine', wsId],
    enabled: !!wsId,
    retry: false,
    queryFn: async () => {
      try {
        const json = await api(`/workspaces/${wsId}/action-items`, { query: { status: 'open', mine: true }, schema: z.unknown() });
        return listOf(json, 'actionItems').flatMap((e) => {
          const o = obj(e);
          const roomId = str(o?.roomId) ?? str(o?.translationRoomId) ?? str(o?.sessionId);
          return parseActionItems([e]).map((a) => ({ ...a, roomId }));
        });
      } catch (e) { if (gone(e)) return []; throw e; }
    },
  });
}
export function useSetMyActionItemDone(wsId: string | undefined) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { id: string; roomId?: string; done: boolean }) => {
      if (!v.roomId) throw new Error('This action item has no session to update.');
      return api(`/meetings/${v.roomId}/action-items/${v.id}`, { method: 'PATCH', body: { status: v.done ? 'done' : 'open' } });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['action-items', 'mine', wsId] }),
  });
}
