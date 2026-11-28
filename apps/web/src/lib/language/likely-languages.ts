import { normalizeLanguageCode } from "./languages";

// Where the user is says what they probably speak, whatever language their OS runs in. Only
// coarse, local signals (time zone, OS region): no GPS, no IP lookup.
const REGION_LANGUAGE: Record<string, string> = {
  VN: "vi", JP: "ja", KR: "ko", CN: "zh", TW: "zh", HK: "zh", SG: "zh", TH: "th", ID: "id",
  MY: "ms", PH: "tl", IN: "hi", FR: "fr", DE: "de", AT: "de", ES: "es", MX: "es", AR: "es",
  IT: "it", PT: "pt", BR: "pt", RU: "ru", NL: "nl", PL: "pl", TR: "tr", SA: "ar", AE: "ar",
};

const TIME_ZONE_LANGUAGE: Record<string, string> = {
  "Asia/Ho_Chi_Minh": "vi", "Asia/Saigon": "vi", "Asia/Tokyo": "ja", "Asia/Seoul": "ko",
  "Asia/Shanghai": "zh", "Asia/Taipei": "zh", "Asia/Hong_Kong": "zh", "Asia/Bangkok": "th",
  "Asia/Jakarta": "id", "Asia/Kuala_Lumpur": "ms", "Asia/Manila": "tl", "Asia/Kolkata": "hi",
  "Europe/Paris": "fr", "Europe/Berlin": "de", "Europe/Vienna": "de", "Europe/Madrid": "es",
  "America/Mexico_City": "es", "Europe/Rome": "it", "Europe/Lisbon": "pt",
  "America/Sao_Paulo": "pt", "Europe/Moscow": "ru", "Europe/Amsterdam": "nl",
  "Europe/Warsaw": "pl", "Europe/Istanbul": "tr", "Asia/Riyadh": "ar", "Asia/Dubai": "ar",
};

export interface LanguageSignals {
  /** OS preferred languages, most preferred first. */
  preferred?: readonly string[];
  /** ISO 3166 region code, e.g. "VN". */
  region?: string;
  /** IANA time zone, e.g. "Asia/Ho_Chi_Minh". */
  timeZone?: string;
}

/** Signals available in this browser (the macOS app adds the full OS list and region). */
export function browserLanguageSignals(): LanguageSignals {
  let timeZone: string | undefined;
  try {
    timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  } catch {
    timeZone = undefined;
  }
  const nav = typeof navigator !== "undefined" ? navigator : undefined;
  return { preferred: nav?.languages?.length ? nav.languages : nav?.language ? [nav.language] : [], timeZone };
}

/**
 * Languages the user likely speaks or hears, most likely first, as bare codes: OS preferred
 * languages, then the region's and the time zone's language. Deduplicated; may be empty.
 */
export function likelyLanguages(...signals: LanguageSignals[]): string[] {
  const out: string[] = [];
  const add = (code?: string) => {
    const c = normalizeLanguageCode(code);
    if (c && c !== "auto" && !out.includes(c)) out.push(c);
  };
  for (const s of signals) s.preferred?.forEach(add);
  for (const s of signals) {
    add(s.region ? REGION_LANGUAGE[s.region.toUpperCase()] : undefined);
    add(s.timeZone ? TIME_ZONE_LANGUAGE[s.timeZone] : undefined);
  }
  return out;
}
