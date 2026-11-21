import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { api, ApiError } from './api';
import {
  joinGrant, member, page, participant, room, routes, settingsRead, settingsWrite, workspace, workspaceSettings,
  type WorkspaceSettings,
} from './schemas';

// ---- workspaces
export function useMembers(wsId: string | undefined) {
  return useQuery({
    queryKey: ['members', wsId],
    enabled: !!wsId,
    queryFn: () => api(`/workspaces/${wsId}/members`, { query: { pageSize: 100 }, schema: page(member) }),
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
export function useRoom(id: string | undefined) {
  return useQuery({
    queryKey: ['room', id],
    enabled: !!id,
    queryFn: () => api(`/translation-rooms/${id}`, { schema: room }),
    refetchInterval: 3000,
    retry: false,
  });
}
export function useParticipants(id: string | undefined) {
  return useQuery({
    queryKey: ['participants', id],
    enabled: !!id,
    queryFn: () => api(`/translation-rooms/${id}/participants`, { schema: z.array(participant) }),
    refetchInterval: 3000,
    retry: false,
  });
}
export function useRoutes(id: string | undefined) {
  return useQuery({
    queryKey: ['routes', id],
    enabled: !!id,
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
    onSuccess: () => qc.invalidateQueries({ queryKey: ['participants', roomId] }),
  });
}
export function useJoinByCode() {
  return useMutation({
    mutationFn: (b: JoinInput & { roomCode: string }) => api('/translation-rooms/join', { method: 'POST', body: b, schema: joinResult }),
  });
}
export function useLeaveRoom(id: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: () => api(`/translation-rooms/${id}/participants/me/leave`, { method: 'PUT', schema: participant }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['participants', id] }),
  });
}

// ---- meetings
export function useMeetingJoin(roomId: string) {
  return useMutation({
    mutationFn: (displayName?: string) =>
      api(`/meetings/${roomId}/join`, { method: 'POST', body: displayName ? { displayName } : {}, schema: joinGrant }),
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
      } catch (e) {
        if (!(e instanceof ApiError) || ![404, 503].includes(e.status)) throw e;
      }
      try {
        return await api(`/translation-rooms/${roomId}/end`, { method: 'POST', schema: room });
      } catch (e) {
        // The meeting-end event may already have ended the room.
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
