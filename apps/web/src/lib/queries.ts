import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { api, apiResponse, apiUpload, ApiError } from './api';
import {
  acceptResult, document as documentSchema, voiceEnrollResult, voiceStatus, aiConfig, aiCredential, aiTest, invitation, joinGrant, knowledgeSearch, member, page, participant, room, routes, settingsRead,
  settingsWrite, workspace, workspaceSettings,
  type AiProvider, type WorkspaceRole, type WorkspaceSettings,
} from './schemas';

// ---- workspaces
export function useMembers(wsId: string | undefined) {
  return useQuery({
    queryKey: ['members', wsId],
    enabled: !!wsId,
    queryFn: () => api(`/workspaces/${wsId}/members`, { query: { pageSize: 100 }, schema: page(member) }),
  });
}

export function useChangeRole(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { userId: string; role: Exclude<WorkspaceRole, 'OWNER'> }) =>
      api(`/workspaces/${wsId}/members/${v.userId}/role`, { method: 'PUT', body: { role: v.role } }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['members', wsId] }),
  });
}
export function useRemoveMember(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (userId: string) => api(`/workspaces/${wsId}/members/${userId}`, { method: 'DELETE' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['members', wsId] }),
  });
}
/** Owner-only: the target becomes OWNER and the caller drops to ADMIN. */
export function useTransferOwnership(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (newOwnerId: string) => api(`/workspaces/${wsId}/transfer-ownership`, { method: 'POST', body: { newOwnerId } }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['members', wsId] });
      void qc.invalidateQueries({ queryKey: ['workspaces'] });
    },
  });
}

// ---- invitations (members page)
export function useInvitations(wsId: string | undefined, enabled: boolean) {
  return useQuery({
    queryKey: ['invitations', wsId],
    enabled: !!wsId && enabled,
    retry: false,
    queryFn: () => api(`/workspaces/${wsId}/invitations`, { schema: z.array(invitation) }),
  });
}
export function useInvite(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { email: string; role: Exclude<WorkspaceRole, 'OWNER'> }) =>
      api(`/workspaces/${wsId}/invitations`, { method: 'POST', body: b, schema: invitation }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['invitations', wsId] }),
  });
}
export function useResendInvitation(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api(`/workspaces/${wsId}/invitations/${id}/resend`, { method: 'POST', schema: invitation }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['invitations', wsId] }),
  });
}
export function useRevokeInvitation(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api(`/workspaces/${wsId}/invitations/${id}`, { method: 'DELETE' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['invitations', wsId] }),
  });
}
/** Invitations addressed to the signed-in user's own email. */
export function usePendingInvitations(userId: string | null) {
  return useQuery({
    queryKey: ['invitations-pending', userId],
    enabled: !!userId,
    queryFn: () => api('/invitations/pending', { schema: z.array(invitation) }),
    staleTime: 15_000,
  });
}
export function useAcceptInvitation() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api(`/invitations/${id}/accept`, { method: 'POST', schema: acceptResult }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['workspaces'] });
      void qc.invalidateQueries({ queryKey: ['invitations-pending'] });
    },
  });
}

// ---- knowledge
export function useKnowledgeSearch(wsId: string | undefined, q: string, sourceType?: string) {
  return useQuery({
    queryKey: ['knowledge', wsId, q, sourceType ?? 'all'],
    enabled: !!wsId && q.trim().length > 0,
    retry: false,
    staleTime: 30_000,
    queryFn: () => api('/knowledge/search', { query: { workspaceId: wsId, q: q.trim(), sourceType, limit: 20 }, schema: knowledgeSearch }),
  });
}

// ---- bring-your-own AI
export function useAiConfig(wsId: string | undefined, enabled: boolean) {
  return useQuery({
    queryKey: ['ai-config', wsId],
    enabled: !!wsId && enabled,
    retry: false,
    queryFn: () => api(`/ai/workspaces/${wsId}/config`, { schema: aiConfig }),
  });
}
export interface AiPurposeInput {
  provider: string; model: string; baseUrl?: string | null; reasoning?: string | null; dataBoundary?: string; fallbackPolicy?: string; enabled: boolean;
}
export function usePutAiConfig(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { purposes: Record<string, AiPurposeInput | null>; acknowledgeWorkspaceControlled?: boolean }) =>
      api(`/ai/workspaces/${wsId}/config`, { method: 'PUT', body: b, schema: aiConfig }),
    onSuccess: (d) => qc.setQueryData(['ai-config', wsId], d),
  });
}
export function usePutCredential(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { provider: AiProvider; apiKey: string; baseUrl?: string }) =>
      api(`/ai/workspaces/${wsId}/credentials/${v.provider}`, { method: 'PUT', body: { apiKey: v.apiKey, baseUrl: v.baseUrl || undefined }, schema: aiCredential }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['ai-config', wsId] }),
  });
}
export function useDeleteCredential(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (provider: AiProvider) => api(`/ai/workspaces/${wsId}/credentials/${provider}`, { method: 'DELETE' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['ai-config', wsId] }),
  });
}
export function useTestCredential(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (provider: AiProvider) => api(`/ai/workspaces/${wsId}/credentials/${provider}/test`, { method: 'POST', schema: aiTest }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['ai-config', wsId] }),
  });
}

export function useCreateWorkspace() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { name: string; description?: string }) => api('/workspaces', { method: 'POST', body: b, schema: workspace }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['workspaces'] }),
  });
}
export function useWorkspaceSettings(wsId: string | undefined) {
  return useQuery({
    queryKey: ['workspace-settings', wsId],
    enabled: !!wsId,
    queryFn: () => api(`/workspaces/${wsId}/settings`, { schema: workspaceSettings }),
  });
}
export function useUpdateWorkspaceSettings(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: WorkspaceSettings) => api(`/workspaces/${wsId}/settings`, { method: 'PUT', body: b, schema: workspaceSettings }),
    onSuccess: (d) => qc.setQueryData(['workspace-settings', wsId], d),
  });
}

// ---- rooms
export function useRooms(wsId: string | undefined, status?: string) {
  return useQuery({
    queryKey: ['rooms', wsId, status ?? 'all'],
    enabled: !!wsId,
    queryFn: () => api('/translation-rooms', { query: { workspaceId: wsId, pageSize: 100, status }, schema: page(room) }),
    refetchInterval: 8000,
  });
}
export const ROOM_PAGE = 50;
/** Paged room list (newest first): `loadMore` follows `total` so nothing silently disappears. */
export function useRoomList(wsId: string | undefined, status?: string) {
  return useInfiniteQuery({
    queryKey: ['rooms', wsId, 'list', status ?? 'all'],
    enabled: !!wsId,
    initialPageParam: 1,
    queryFn: ({ pageParam }) => api('/translation-rooms', { query: { workspaceId: wsId, pageSize: ROOM_PAGE, page: pageParam, status }, schema: page(room) }),
    getNextPageParam: (last) => (last.page * last.pageSize < last.total ? last.page + 1 : undefined),
    refetchInterval: 8000,
  });
}
export function useRoom(id: string | undefined) {
  return useQuery({
    queryKey: ['room', id],
    enabled: !!id,
    queryFn: () => api(`/translation-rooms/${id}`, { schema: room }),
    refetchInterval: 3000,
    retry: false,
  });
}
export function useParticipants(id: string | undefined, enabled = true) {
  return useQuery({
    queryKey: ['participants', id],
    enabled: !!id && enabled,
    queryFn: () => api(`/translation-rooms/${id}/participants`, { schema: z.array(participant) }),
    refetchInterval: 3000,
    retry: false,
  });
}
export function useRoutes(id: string | undefined, enabled = true) {
  return useQuery({
    queryKey: ['routes', id],
    enabled: !!id && enabled,
    queryFn: () => api(`/translation-rooms/${id}/audio-routes`, { schema: routes }),
    refetchInterval: 5000,
    retry: false,
  });
}
export interface CreateRoomInput {
  workspaceId: string; title: string; sourceLanguage: string; targetLanguages: string[]; requiresApproval: boolean; displayName?: string;
}
export function useCreateRoom() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: CreateRoomInput) => api('/translation-rooms', { method: 'POST', body: b, schema: room }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['rooms'] }),
  });
}
export type RoomAction = 'start' | 'pause' | 'resume' | 'translation/stop' | 'end' | 'cancel';
export function useRoomAction(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (a: RoomAction) => api(`/translation-rooms/${id}/${a}`, { method: 'POST', schema: room }),
    onSuccess: (r) => {
      qc.setQueryData(['room', id], r);
      void qc.invalidateQueries({ queryKey: ['rooms'] });
      void qc.invalidateQueries({ queryKey: ['routes', id] });
    },
  });
}
export function useParticipantAction(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { pid: string; action: 'admit' | 'kick' }) =>
      api(`/translation-rooms/${id}/participants/${v.pid}/${v.action}`, { method: 'PUT', schema: participant }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['participants', id] }),
  });
}
export function useGenerateRoutes(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api(`/translation-rooms/${id}/audio-routes/generate`, { method: 'POST', schema: routes }),
    onSuccess: (d) => qc.setQueryData(['routes', id], d),
  });
}

const joinResult = participant.extend({ roomId: z.string().optional() });
export interface JoinInput { displayName?: string; speakLanguage: string; listenLanguage: string }
export function useJoinById(roomId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: JoinInput) => api(`/translation-rooms/${roomId}/join`, { method: 'POST', body: b, schema: joinResult }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['participants', roomId] });
      void qc.invalidateQueries({ queryKey: ['room', roomId] });
    },
  });
}
export function useJoinByCode() {
  return useMutation({
    mutationFn: (b: JoinInput & { roomCode: string }) => api('/translation-rooms/join', { method: 'POST', body: b, schema: joinResult }),
  });
}
/** Mid-meeting language change. `speakLanguage: 'none'` makes the person listen-only. */
export function useUpdateMyLanguages(roomId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: { speakLanguage?: string; listenLanguage?: string }) =>
      api(`/translation-rooms/${roomId}/participants/me/languages`, { method: 'PUT', body: b, schema: participant }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['participants', roomId] }),
  });
}
export function useLeaveRoom(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api(`/translation-rooms/${id}/participants/me/leave`, { method: 'PUT', schema: participant }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['participants', id] });
      void qc.invalidateQueries({ queryKey: ['room', id] });
    },
  });
}

// ---- meetings
/** Secondary connections of the same user: subscribe-only token, identity `{userId}:{clientTag}`. */
export type ClientTag = 'listener' | 'bridge-listener';
export function useMeetingJoin(roomId: string) {
  return useMutation({
    mutationFn: (v: { displayName?: string; clientTag?: ClientTag } = {}) =>
      api(`/meetings/${roomId}/join`, {
        method: 'POST',
        body: { ...(v.displayName ? { displayName: v.displayName } : {}), ...(v.clientTag ? { clientTag: v.clientTag } : {}) },
        schema: joinGrant,
      }),
  });
}

// ---- runtime settings
export type Namespace = 'suggestion' | 'pipeline';
export type ScopeKind = 'workspace' | 'room';
export function useEffectiveSettings(ns: Namespace, scope: ScopeKind, id: string | undefined, workspaceId: string | undefined) {
  return useQuery({
    queryKey: ['settings-effective', ns, scope, id],
    enabled: !!id && !!workspaceId,
    queryFn: () =>
      api(`/settings/${ns}`, {
        query: scope === 'room' ? { effective: true, roomId: id, workspaceId } : { effective: true, workspaceId: id },
        schema: settingsRead,
      }),
  });
}
/** Stored override at this scope (admin only: 403 for plain members). */
export function useScopedSettings(ns: Namespace, scope: ScopeKind, id: string | undefined, enabled: boolean) {
  return useQuery({
    queryKey: ['settings-scoped', ns, scope, id],
    enabled: !!id && enabled,
    retry: false,
    queryFn: () => api(`/settings/${ns}`, { query: { scope, id }, schema: settingsRead }),
  });
}
export function usePutSettings(ns: Namespace, scope: ScopeKind, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (value: unknown) => api(`/settings/${ns}`, { method: 'PUT', query: { scope, id }, body: value, schema: settingsWrite }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['settings-scoped', ns, scope, id] });
      void qc.invalidateQueries({ queryKey: ['settings-effective', ns] });
    },
  });
}
export function useResetSettings(ns: Namespace, scope: ScopeKind, id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api(`/settings/${ns}`, { method: 'DELETE', query: { scope, id } }),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ['settings-scoped', ns, scope, id] });
      void qc.invalidateQueries({ queryKey: ['settings-effective', ns] });
    },
  });
}

export function useEndMeeting(roomId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async () => {
      // The meeting (LiveKit) session only exists once somebody joined and LiveKit is configured:
      // 404 "meeting has not started" / 503 mean there is nothing to tear down, so end the room itself.
      try {
        await api(`/meetings/${roomId}/end`, { method: 'POST' });
        // The meeting endpoint ends the room itself.
        return await api(`/translation-rooms/${roomId}`, { schema: room });
      } catch (e) {
        if (!(e instanceof ApiError) || ![404, 503].includes(e.status)) throw e;
      }
      // Legacy backends: no meeting session yet, end the room directly.
      try {
        return await api(`/translation-rooms/${roomId}/end`, { method: 'POST', schema: room });
      } catch (e) {
        if (e instanceof ApiError && e.status === 409) return api(`/translation-rooms/${roomId}`, { schema: room });
        throw e;
      }
    },
    onSuccess: (r) => {
      qc.setQueryData(['room', roomId], r);
      void qc.invalidateQueries({ queryKey: ['rooms'] });
    },
  });
}

// ---- documents
export interface DocumentFilters { page: number; search: string; category: string }
export const DOCUMENT_PAGE_SIZE = 20;
/** Polls every 3s while any listed document is still being indexed. */
export function useDocuments(wsId: string | undefined, f: DocumentFilters) {
  return useQuery({
    queryKey: ['documents', wsId, f.page, f.search, f.category],
    enabled: !!wsId,
    retry: false,
    placeholderData: (prev) => prev,
    refetchInterval: (q) => (q.state.data?.items.some((d) => d.ingestionStatus === 'pending') ? 3000 : false),
    queryFn: () => api(`/workspaces/${wsId}/documents`, {
      query: { page: f.page, pageSize: DOCUMENT_PAGE_SIZE, search: f.search.trim() || undefined, category: f.category === 'all' ? undefined : f.category },
      schema: page(documentSchema),
    }),
  });
}
export interface UploadDocumentInput { file: File; title: string; category: string; accessLevel: string; aiRetrievalAllowed: boolean }
export function useUploadDocument(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: UploadDocumentInput & { onProgress?: (f: number) => void }) => {
      const form = new FormData();
      if (v.title.trim()) form.set('title', v.title.trim());
      form.set('category', v.category);
      form.set('accessLevel', v.accessLevel);
      form.set('aiRetrievalAllowed', String(v.aiRetrievalAllowed));
      form.set('file', v.file, v.file.name);
      return apiUpload(`/workspaces/${wsId}/documents`, form, documentSchema, v.onProgress);
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['documents', wsId] }),
  });
}
export function usePatchDocument(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { id: string; title?: string; category?: string; accessLevel?: string; aiRetrievalAllowed?: boolean }) => {
      const { id, ...body } = v;
      return api(`/workspaces/${wsId}/documents/${id}`, { method: 'PATCH', body, schema: documentSchema });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['documents', wsId] }),
  });
}
export function useArchiveDocument(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api(`/workspaces/${wsId}/documents/${id}`, { method: 'DELETE' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['documents', wsId] }),
  });
}
/** Downloads through the authenticated API (the file is not publicly addressable) and hands it to the browser. */
export async function downloadDocument(wsId: string, doc: { id: string; fileName: string }) {
  const res = await apiResponse(`/workspaces/${wsId}/documents/${doc.id}/download`);
  const blob = await res.blob();
  const href = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = href;
  a.download = doc.fileName;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(href), 10_000);
}

// ---- voice profile
export function useVoiceStatus(userId: string | null) {
  return useQuery({
    queryKey: ['voice', userId],
    enabled: !!userId,
    retry: false,
    queryFn: () => api('/me/voice', { schema: voiceStatus }),
  });
}
export function useGrantVoiceConsent(userId: string | null) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { consentVersion: string; acknowledgementHash: string }) =>
      api('/me/voice/consent', { method: 'POST', body: { granted: true, ...v }, schema: voiceStatus }),
    onSuccess: (d) => qc.setQueryData(['voice', userId], d),
  });
}
export function useRevokeVoiceConsent(userId: string | null) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api('/me/voice/consent', { method: 'DELETE', schema: voiceStatus }),
    onSuccess: (d) => qc.setQueryData(['voice', userId], d),
  });
}
export function useEnrollVoice(userId: string | null) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (sampleBase64: string) =>
      api('/me/voice/enroll', { method: 'POST', body: { sampleBase64, format: 'wav' }, schema: voiceEnrollResult }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['voice', userId] }),
  });
}
