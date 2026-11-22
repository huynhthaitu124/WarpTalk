// Meeting chat over REST (source of truth). LiveKit's "chat" data topic only speeds delivery up.
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { api } from './api';
import { chatMessage, type ChatMessageDto } from './schemas';

export const CHAT_PAGE = 50;
export const CHAT_MAX_CHARS = 1000;

export const chatKey = (roomId: string) => ['chat', roomId] as const;

export function fetchChat(roomId: string, o: { before?: string; limit?: number } = {}) {
  return api(`/meetings/${roomId}/chat/messages`, { query: { before: o.before, limit: o.limit ?? CHAT_PAGE }, schema: z.array(chatMessage) });
}

/** Idempotent on `clientMessageId`: the same id can be re-sent safely (201 first time, 200 on replay). */
export function postChat(roomId: string, content: string, clientMessageId: string) {
  return api(`/meetings/${roomId}/chat/messages`, { method: 'POST', body: { content, clientMessageId }, schema: chatMessage });
}

/** Newest page, oldest first. `refetchInterval` is chosen by the caller (poll while the panel is open). */
export function useChatLatest(roomId: string, refetchInterval: number | false) {
  return useQuery({
    queryKey: chatKey(roomId),
    queryFn: () => fetchChat(roomId),
    refetchInterval,
    retry: false,
    staleTime: 0,
  });
}

export function useHideChat(roomId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (v: { id: string; reason?: string }) =>
      api(`/meetings/${roomId}/chat/messages/${v.id}/hide`, { method: 'POST', body: v.reason ? { reason: v.reason } : {}, schema: chatMessage }),
    onSuccess: () => qc.invalidateQueries({ queryKey: chatKey(roomId) }),
  });
}

export type { ChatMessageDto };
