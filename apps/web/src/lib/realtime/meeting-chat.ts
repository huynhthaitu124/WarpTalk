/**
 * Meeting chat on the Rust API, shared by the REST compat layer and the realtime channel.
 * No imports of the API client here: lib/api/rust-compat.ts imports this module.
 */
import type { ChatMessageDto } from "@/types/realtime";

/** `ChatMessageDto` of the Rust meeting module. */
export interface RustChatMessage {
  messageId: string;
  senderId: string | null;
  senderName: string;
  content: string;
  createdAt: string;
  hidden: boolean;
  translations: Record<string, string>;
}

export function isRustChatMessage(v: unknown): v is RustChatMessage {
  const o = v as Partial<RustChatMessage> | null;
  return !!o && typeof o.messageId === "string" && typeof o.content === "string";
}

/** Translations of chat messages seen so far, by message id (filled from REST and `chat.translated`). */
const chatTranslations = new Map<string, Record<string, string>>();

export function rememberChatTranslations(messageId: string, translations: Record<string, string>): void {
  if (Object.keys(translations).length === 0) return;
  chatTranslations.set(messageId, { ...chatTranslations.get(messageId), ...translations });
}

export function knownChatTranslation(messageId: string, language: string): string | undefined {
  const map = chatTranslations.get(messageId);
  if (!map) return undefined;
  const short = language.slice(0, 2).toLowerCase();
  return map[language] ?? Object.entries(map).find(([k]) => k.slice(0, 2).toLowerCase() === short)?.[1];
}

export function toLegacyChatMessage(m: RustChatMessage, roomId: string): ChatMessageDto {
  rememberChatTranslations(m.messageId, m.translations ?? {});
  return {
    id: m.messageId,
    meetingRoomId: roomId,
    senderUserId: m.senderId ?? undefined,
    senderDisplayName: m.senderName,
    senderType: m.senderId ? "User" : "System",
    messageType: "Text",
    originalLanguage: "",
    originalText: m.content,
    translationEnabled: true,
    createdAt: m.createdAt,
  };
}

type ChatPublisher = (payload: Record<string, unknown>) => Promise<void>;
let publisher: ChatPublisher | null = null;

/** Set by the meeting chat channel while it is joined to a LiveKit room. */
export function setChatPublisher(p: ChatPublisher | null): void {
  publisher = p;
}

/**
 * Sends a change the current user just made over REST to everyone else in the room. Best
 * effort: the REST write is the source of truth, and the others read it on their next refresh.
 */
export function broadcastChat(payload: Record<string, unknown>): void {
  publisher?.(payload).catch((error) => console.warn("[realtime] chat broadcast failed", error));
}
