// Data-channel payloads of the meeting UI. Topic "captions" is owned by the pipeline (lib/captions.ts).
// Chat is peer-to-peer only: there is no backend chat persistence yet, so history is lost on reload
// and people who join later do not see earlier messages.

export interface ChatMessage { id: string; from: string; fromIdentity?: string; text: string; ts: number; mine: boolean }

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

export function parseChat(payload: Uint8Array): { id: string; text: string; ts: number } | null {
  const j = json(payload);
  if (!j || typeof j.text !== 'string' || !j.text.trim()) return null;
  return {
    id: typeof j.id === 'string' ? j.id : `${Date.now()}-${Math.random()}`,
    text: j.text.slice(0, 2000),
    ts: typeof j.ts === 'number' ? j.ts : Date.now(),
  };
}
