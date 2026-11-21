// Caption wire format (LiveKit data topic "captions", see apps/pipeline/src/downstream.rs):
//   {type:"stt"|"translation", segment_id, speaker_id, text, lang, is_early, translations:{lang:text}, emittedAtMs}
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
  const speakerId = typeof j.speaker_id === 'string' && !UUID.test(j.speaker_id) ? j.speaker_id : undefined;
  const emitted = typeof j.emittedAtMs === 'number' ? j.emittedAtMs : undefined;
  const lat = emitted === undefined ? undefined : Math.max(0, now - emitted);
  const explicitFinal = j.final ?? j.isFinal;
  return {
    id: String(j.segment_id ?? j.segmentId ?? j.id ?? `c${seq}`),
    speaker: String(j.speaker ?? j.speakerName ?? j.participantName ?? speakerId ?? fallbackSpeaker),
    text,
    lang: typeof j.lang === 'string' ? j.lang : typeof j.language === 'string' ? j.language : undefined,
    translations,
    final: explicitFinal === undefined ? j.is_early !== true : Boolean(explicitFinal),
    latencyMs: lat !== undefined && lat <= MAX_LATENCY_MS ? lat : undefined,
  };
}

/**
 * Same segment id updates the line in place: the original arrives first with empty translations,
 * translated captions follow (one message per target language).
 */
export function mergeCaption(cur: CaptionLine[], line: CaptionLine, max = 200): CaptionLine[] {
  const i = cur.findIndex((c) => c.id === line.id);
  if (i < 0) return [...cur, line].slice(-max);
  const old = cur[i]!;
  const merged: CaptionLine = {
    ...old,
    text: line.text && (line.final || !old.final) ? line.text : old.text,
    lang: line.lang ?? old.lang,
    speaker: line.speaker || old.speaker,
    translations: { ...old.translations, ...line.translations },
    final: old.final || line.final,
    latencyMs: line.latencyMs ?? old.latencyMs,
  };
  return cur.map((c, k) => (k === i ? merged : c));
}

/** The translation to show: the listener's language, or (original audio) the first one available. */
export function pickTranslation(l: CaptionLine, lang: string | undefined): string | undefined {
  if (lang && lang !== 'original') return l.translations[lang];
  return Object.values(l.translations)[0];
}
