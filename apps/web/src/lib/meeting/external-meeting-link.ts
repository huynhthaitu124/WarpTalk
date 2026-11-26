/**
 * A pasted Google Meet / Zoom / Teams link (or bare code), read the way the server reads it.
 *
 * Client-side mirror of `normalize_meeting_ref` (crates/modules/room/src/domain/bridge.rs) plus
 * provider detection, so the Create Room dialog can say what is wrong before submitting. The
 * server stays the authority: it normalises again and refuses anything it does not understand.
 */

export type ExternalMeetingProvider = "google_meet" | "zoom" | "teams";

export const EXTERNAL_PROVIDER_LABEL: Record<ExternalMeetingProvider, string> = {
  google_meet: "Google Meet",
  zoom: "Zoom",
  teams: "Microsoft Teams",
};

/** Message keys under `rooms.create.externalLink.errors`. */
export type ExternalMeetingLinkError = "empty" | "unrecognised" | "badMeet" | "badZoom" | "unsupported";

export type ParsedExternalMeetingLink =
  | { ok: true; provider: ExternalMeetingProvider; code: string; url?: string }
  | { ok: false; error: ExternalMeetingLinkError };

const MEET_CODE = /^[a-z]{3}-[a-z]{4}-[a-z]{3}$/;
const ZOOM_NUMBER = /^\d{9,11}$/;

export function parseExternalMeetingLink(input: string): ParsedExternalMeetingLink {
  const raw = input.trim();
  if (!raw) return { ok: false, error: "empty" };
  const clean = raw.split(/[?#]/)[0] ?? raw;

  if (MEET_CODE.test(clean.toLowerCase())) return { ok: true, provider: "google_meet", code: clean.toLowerCase() };
  const digits = clean.replace(/\s+/g, "");
  if (ZOOM_NUMBER.test(digits)) return { ok: true, provider: "zoom", code: digits };
  if (!/[/.]/.test(clean)) return { ok: false, error: "unrecognised" };

  const rest = clean.replace(/^https?:\/\//i, "");
  const slash = rest.indexOf("/");
  const host = (slash < 0 ? rest : rest.slice(0, slash)).toLowerCase();
  const path = (slash < 0 ? "" : rest.slice(slash + 1)).replace(/^\/+|\/+$/g, "");

  if (host === "meet.google.com") {
    const code = path.toLowerCase();
    return MEET_CODE.test(code) ? { ok: true, provider: "google_meet", code, url: raw } : { ok: false, error: "badMeet" };
  }
  if (host === "zoom.us" || host.endsWith(".zoom.us")) {
    const id = path.replace(/^j\//, "");
    return ZOOM_NUMBER.test(id) ? { ok: true, provider: "zoom", code: id, url: raw } : { ok: false, error: "badZoom" };
  }
  if ((host === "teams.microsoft.com" || host === "teams.live.com") && path) {
    return { ok: true, provider: "teams", code: `https://${host}/${path}`, url: raw };
  }
  return { ok: false, error: "unsupported" };
}
