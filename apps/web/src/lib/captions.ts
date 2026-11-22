// Caption wire format (LiveKit data topic "captions", see apps/pipeline/src/downstream.rs):
//   {type:"stt"|"translation", segment_id, speaker_id, text, lang, is_early, translations:{lang:text}, emittedAtMs}
// Current shape adds: speakerName, sentenceIndex, isLastSentence and CUMULATIVE `translations` (lang -> full text so far).
// Legacy/simple shapes ({id,text,speaker,language,final}) and plain text are still accepted.

export interface CaptionLine {
  id: string;
  speaker: string;
  /** Original-language text. */
  text: string;
  lang?: string;
  /** Translations by target language code. */
  translations: Record<string, string>;
  final: boolean;
  /** Participant id (or identity) of the speaker as sent by the pipeline, when present. */
  speakerId?: string;
  /** User id of the speaker (LiveKit identity) when the pipeline sends it. */
  speakerUserId?: string;
  /** True when `speaker` is a real display name (not just the fallback label). */
  named?: boolean;
  /** Sentence this message carries (multi-sentence segments); undefined for single-shot messages. */
  sentenceIndex?: number;
  isLastSentence?: boolean;
  /** `translations` holds the whole segment so far (replace) instead of only this sentence (append). */
  cumulative?: boolean;
  /** Per-sentence original text, joined in order into `text`. */
  parts?: Record<number, string>;
  /** Per-sentence translations (legacy shape), joined in order into `translations`. */
  tparts?: Record<string, Record<number, string>>;
  /** Milliseconds between the pipeline's `emittedAtMs` and the moment the web app received the message. */
  latencyMs?: number;
}

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-/i;
/** Beyond this the clocks are skewed rather than the pipeline slow: do not show it. */
const MAX_LATENCY_MS = 60_000;

export function parseCaption(payload: Uint8Array, fallbackSpeaker: string, seq: number, now = Date.now()): CaptionLine | null {
  const raw = new TextDecoder().decode(payload).trim();
  if (!raw) return null;
  const plain: CaptionLine = { id: `c${seq}`, speaker: fallbackSpeaker, text: raw, translations: {}, final: true };
  let j: Record<string, unknown>;
  try {
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== 'object' || parsed === null) return plain;
    j = parsed as Record<string, unknown>;
  } catch {
    return plain;
  }
  const translations: Record<string, string> = {};
  if (typeof j.translations === 'object' && j.translations !== null) {
    for (const [k, v] of Object.entries(j.translations as Record<string, unknown>)) if (typeof v === 'string' && v) translations[k] = v;
  }
  const text = String(j.text ?? j.caption ?? '');
  if (!text && Object.keys(translations).length === 0) return null;
  const rawSpeakerId = typeof j.speakerId === 'string' ? j.speakerId : typeof j.speaker_id === 'string' ? j.speaker_id : undefined;
  const speakerId = rawSpeakerId && !UUID.test(rawSpeakerId) ? rawSpeakerId : undefined;
  const nameField = [j.speakerName, j.speaker_name, j.speaker, j.participantName].find((v) => typeof v === 'string' && v.trim());
  const speakerName = typeof nameField === 'string' ? nameField.trim() : undefined;
  const idxRaw = j.sentenceIndex ?? j.sentence_index ?? j.chunk_idx;
  const sentenceIndex = typeof idxRaw === 'number' && Number.isInteger(idxRaw) && idxRaw >= 0 ? idxRaw : undefined;
  const lastRaw = j.isLastSentence ?? j.is_last_sentence ?? j.is_final_chunk;
  const emitted = typeof j.emittedAtMs === 'number' ? j.emittedAtMs : undefined;
  const lat = emitted === undefined ? undefined : Math.max(0, now - emitted);
  const explicitFinal = j.final ?? j.isFinal;
  return {
    id: String(j.segment_id ?? j.segmentId ?? j.id ?? `c${seq}`),
    speaker: speakerName ?? speakerId ?? fallbackSpeaker,
    named: !!speakerName,
    speakerId: rawSpeakerId,
    speakerUserId: typeof j.speakerUserId === 'string' ? j.speakerUserId : undefined,
    sentenceIndex,
    isLastSentence: typeof lastRaw === 'boolean' ? lastRaw : undefined,
    // Current pipeline shape (camelCase + sentenceIndex) sends the cumulative map.
    cumulative: j.sentenceIndex !== undefined || j.isLastSentence !== undefined,
    text,
    lang: typeof j.lang === 'string' ? j.lang : typeof j.language === 'string' ? j.language : undefined,
    translations,
    final: explicitFinal === undefined ? j.is_early !== true : Boolean(explicitFinal),
    latencyMs: lat !== undefined && lat <= MAX_LATENCY_MS ? lat : undefined,
  };
}

function joinParts(p: Record<number, string>): string {
  return Object.keys(p).map(Number).sort((a, b) => a - b).map((k) => p[k]).join(' ');
}

/**
 * Same segment id updates the line in place: the original arrives first with empty translations,
 * translated captions follow (one message per target language). Multi-sentence segments arrive one message per
 * sentence: those are appended per `sentenceIndex` (original text and legacy per-sentence translations) or replaced
 * when `translations` is cumulative.
 */
export function mergeCaption(cur: CaptionLine[], line: CaptionLine, max = 200): CaptionLine[] {
  const i = cur.findIndex((c) => c.id === line.id);
  if (i < 0) {
    if (line.sentenceIndex === undefined) return [...cur, line].slice(-max);
    const first: CaptionLine = {
      ...line,
      parts: line.text ? { [line.sentenceIndex]: line.text } : {},
      tparts: line.cumulative ? {} : Object.fromEntries(Object.entries(line.translations).map(([k, v]) => [k, { [line.sentenceIndex as number]: v }])),
    };
    return [...cur, first].slice(-max);
  }
  const old = cur[i]!;
  const idx = line.sentenceIndex;
  let text = line.text && (line.final || !old.final) ? line.text : old.text;
  let translations = { ...old.translations, ...line.translations };
  let parts = old.parts;
  let tparts = old.tparts;
  if (idx !== undefined) {
    parts = { ...(old.parts ?? (old.text ? { 0: old.text } : {})) };
    if (line.text) {
      const prev = joinParts(parts);
      // A text that already contains everything before it is cumulative: replace instead of append.
      if (prev && idx > 0 && line.text.startsWith(prev)) parts = { [idx]: line.text };
      else if (line.final || !old.final || parts[idx] === undefined) parts[idx] = line.text;
    }
    text = joinParts(parts) || old.text;
    if (line.cumulative) {
      translations = { ...old.translations, ...line.translations };
    } else {
      tparts = { ...(old.tparts ?? {}) };
      translations = { ...old.translations };
      for (const [lang, v] of Object.entries(line.translations)) {
        const per = { ...(tparts[lang] ?? (old.translations[lang] && old.sentenceIndex === undefined ? { 0: old.translations[lang] } : {})) };
        per[idx] = v;
        tparts[lang] = per;
        translations[lang] = joinParts(per);
      }
    }
  }
  const merged: CaptionLine = {
    ...old,
    text,
    parts,
    tparts,
    lang: line.lang ?? old.lang,
    speaker: line.named || !old.named ? line.speaker || old.speaker : old.speaker,
    named: old.named || line.named,
    speakerId: line.speakerId ?? old.speakerId,
    speakerUserId: line.speakerUserId ?? old.speakerUserId,
    sentenceIndex: idx ?? old.sentenceIndex,
    isLastSentence: line.isLastSentence ?? old.isLastSentence,
    translations,
    final: old.final || line.final,
    latencyMs: line.latencyMs ?? old.latencyMs,
  };
  return cur.map((c, k) => (k === i ? merged : c));
}

export interface SpeakerLookup { id?: string | null; userId?: string | null; displayName: string }

/** `{userId}:{tag}` -> `{userId}` (secondary LiveKit connections of the same user). */
export function baseIdentity(identity: string): string {
  const k = identity.indexOf(':');
  return k < 0 ? identity : identity.slice(0, k);
}

/** A data-packet sender worth showing as a speaker: never the hidden pipeline agent. */
export function senderLabel(from?: { identity: string; name?: string }): string | undefined {
  if (!from) return undefined;
  if (/^pipeline[-_]/i.test(from.identity) || /^pipeline[-_]/i.test(from.name ?? '')) return undefined;
  return from.name || from.identity;
}

/** Fills in the speaker label of lines that carry no name: roster lookup by participant id or LiveKit identity. */
export function resolveSpeakers(lines: CaptionLine[], roster: SpeakerLookup[]): CaptionLine[] {
  if (roster.length === 0) return lines;
  return lines.map((l) => {
    if (l.named || !(l.speakerId || l.speakerUserId)) return l;
    const keys = [l.speakerId, l.speakerUserId].filter((k): k is string => !!k).map(baseIdentity);
    const hit = roster.find((r) => keys.some((k) => r.id === k || r.userId === k));
    return hit ? { ...l, speaker: hit.displayName, named: true } : l;
  });
}

/** The translation to show: the listener's language, or (original audio) the first one available. */
export function pickTranslation(l: CaptionLine, lang: string | undefined): string | undefined {
  if (lang && lang !== 'original') return l.translations[lang];
  return Object.values(l.translations)[0];
}
