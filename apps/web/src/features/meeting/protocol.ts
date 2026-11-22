// Data-channel payloads of the meeting UI. Topic "captions" is owned by the pipeline (lib/captions.ts).
// Chat: the REST API (`/meetings/{id}/chat/messages`) is the source of truth and stores history.
// Topic "chat" only carries instant copies of what was just posted (and server push events such as
// `chat.translated`); receivers dedupe by messageId and reconcile with REST.

export type ChatStatus = 'sent' | 'sending' | 'failed';
export interface ChatMessage {
  /** Server message id, or the clientMessageId while the message is still being sent. */
  id: string;
  clientId?: string;
  senderId?: string | null;
  from: string;
  text: string;
  ts: number;
  mine: boolean;
  hidden: boolean;
  translations: Record<string, string>;
  status: ChatStatus;
  error?: string;
}

export const REACTIONS = [
  { emoji: '👍', label: 'Thumbs up' },
  { emoji: '👏', label: 'Applause' },
  { emoji: '❤️', label: 'Heart' },
  { emoji: '😂', label: 'Laugh' },
  { emoji: '🎉', label: 'Celebrate' },
  { emoji: '🙏', label: 'Thanks' },
] as const;

export type ReactionPayload = { t: 'reaction'; emoji: string } | { t: 'hand'; raised: boolean };

const dec = new TextDecoder();

function json(payload: Uint8Array): Record<string, unknown> | null {
  try {
    const v: unknown = JSON.parse(dec.decode(payload));
    return typeof v === 'object' && v !== null ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

export function parseReaction(payload: Uint8Array): ReactionPayload | null {
  const j = json(payload);
  if (!j) return null;
  if (j.t === 'hand' && typeof j.raised === 'boolean') return { t: 'hand', raised: j.raised };
  if (j.t === 'reaction' && typeof j.emoji === 'string' && j.emoji.length > 0 && j.emoji.length <= 8) return { t: 'reaction', emoji: j.emoji };
  return null;
}

/** What travels on the "chat" topic. */
export type ChatWire =
  | { kind: 'message'; message: { messageId: string; senderId: string | null; senderName: string; content: string; createdAt: string; hidden: boolean; translations: Record<string, string> } }
  | { kind: 'translated'; messageId: string; translations: Record<string, string> }
  | { kind: 'hidden'; messageId: string };

function strMap(v: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  if (typeof v === 'object' && v !== null) for (const [k, t] of Object.entries(v as Record<string, unknown>)) if (typeof t === 'string' && t) out[k] = t;
  return out;
}

export function parseChat(payload: Uint8Array): ChatWire | null {
  const j = json(payload);
  if (!j) return null;
  if (j.type === 'chat.translated' && typeof j.messageId === 'string') return { kind: 'translated', messageId: j.messageId, translations: strMap(j.translations) };
  if (j.type === 'chat.hidden' && typeof j.messageId === 'string') return { kind: 'hidden', messageId: j.messageId };
  if (typeof j.messageId !== 'string' || typeof j.content !== 'string' || !j.content.trim()) return null;
  const created = typeof j.createdAt === 'string' && !Number.isNaN(Date.parse(j.createdAt)) ? j.createdAt : new Date().toISOString();
  return {
    kind: 'message',
    message: {
      messageId: j.messageId,
      senderId: typeof j.senderId === 'string' ? j.senderId : null,
      senderName: typeof j.senderName === 'string' && j.senderName ? j.senderName : 'Someone',
      content: j.content.slice(0, 1000),
      createdAt: created,
      hidden: false,
      translations: strMap(j.translations),
    },
  };
}
