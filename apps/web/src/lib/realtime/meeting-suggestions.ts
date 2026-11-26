/**
 * Live AI suggestions for the meeting room on the Rust backend.
 *
 * The .NET gateway pushed `AiSuggestionReceived` over SignalR. The Rust API serves the same hints
 * as Server-Sent Events on `GET /meetings/{roomId}/suggestions/stream` (event `suggestion`, data =
 * a snake_case SuggestionResult). This module reads that stream with `fetch` (so the token stays in
 * the Authorization header, never in a URL) and maps each item to the `AiSuggestionDto` the
 * transcript panel already renders. A 404/501/503 means the feature is not deployed: the stream
 * stops quietly and the UI shows no hints.
 *
 * It also holds the dev-only test-mode calls (`/dev/test-mode`, `/meetings/{id}/dev/transcript`),
 * which the API answers only when it runs with `SUGGESTION_TEST_SCRIPT` in a debug build.
 */
import apiClient, { getUsableAccessToken } from "@/lib/api/client";
import { rustApiBaseUrl } from "@/lib/api/rust-compat";
import type { AiSuggestionDto } from "@/types/realtime";

const GONE = new Set([404, 405, 501, 503]);
const RETRY_DELAYS_MS = [1_500, 3_000, 6_000, 15_000, 30_000];

const str = (v: unknown) => (typeof v === "string" && v ? v : undefined);

export function toSuggestionDto(roomId: string, data: string): AiSuggestionDto | null {
  let j: unknown;
  try {
    j = JSON.parse(data);
  } catch {
    return null;
  }
  if (typeof j !== "object" || j === null) return null;
  const o = j as Record<string, unknown>;
  const segmentId = str(o.segment_id) ?? str(o.segmentId);
  const content = str(o.content);
  if (!segmentId || !content) return null;
  const ts = typeof o.timestamp_ms === "number" ? o.timestamp_ms : Date.now();
  const sources = Array.isArray(o.sources) && o.sources.length ? JSON.stringify(o.sources) : null;
  return {
    translationRoomId: roomId,
    segmentId,
    category: str(o.category) ?? "clarification",
    content,
    detail: str(o.detail) ?? null,
    confidence: typeof o.confidence === "number" ? o.confidence : 1,
    language: str(o.language) ?? "",
    createdAt: new Date(ts).toISOString(),
    sourcesJson: sources,
  };
}

/** Incremental SSE parser: feed text chunks, get `{event, data, id}` messages. */
function sseParser(onMessage: (m: { event: string; data: string; id?: string }) => void) {
  let buf = "";
  let event = "";
  let id: string | undefined;
  let data: string[] = [];
  return (chunk: string) => {
    buf += chunk;
    let nl: number;
    while ((nl = buf.search(/\r\n|\n|\r/)) >= 0) {
      const line = buf.slice(0, nl);
      buf = buf.slice(nl + (buf.startsWith("\r\n", nl) ? 2 : 1));
      if (line === "") {
        if (data.length) onMessage({ event: event || "message", data: data.join("\n"), id });
        event = "";
        data = [];
      } else if (!line.startsWith(":")) {
        const k = line.indexOf(":");
        const field = k < 0 ? line : line.slice(0, k);
        const value = k < 0 ? "" : line.slice(k + 1).replace(/^ /, "");
        if (field === "event") event = value;
        else if (field === "data") data.push(value);
        else if (field === "id") id = value;
      }
    }
  };
}

/**
 * Follows the room's suggestion stream until `signal` aborts, reconnecting with `Last-Event-ID`
 * after a drop. Resolves when the stream is aborted or refused for good.
 */
export async function followSuggestions(
  roomId: string,
  onSuggestion: (s: AiSuggestionDto) => void,
  signal: AbortSignal,
): Promise<void> {
  let lastId: string | undefined;
  let attempt = 0;
  while (!signal.aborted) {
    try {
      const token = await getUsableAccessToken();
      const headers: Record<string, string> = { accept: "text/event-stream" };
      if (token) headers.authorization = `Bearer ${token}`;
      if (lastId) headers["last-event-id"] = lastId;
      const url = `${rustApiBaseUrl()}/meetings/${encodeURIComponent(roomId)}/suggestions/stream`;
      const res = await fetch(url, { headers, signal, cache: "no-store" });
      if (GONE.has(res.status) || res.status === 403) return;
      if (res.ok && res.body) {
        attempt = 0;
        const feed = sseParser((m) => {
          if (m.id) lastId = m.id;
          if (m.event !== "suggestion" && m.event !== "message") return;
          const dto = toSuggestionDto(roomId, m.data);
          if (dto) onSuggestion(dto);
        });
        const reader = res.body.getReader();
        const dec = new TextDecoder();
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          feed(dec.decode(value, { stream: true }));
        }
      }
    } catch {
      if (signal.aborted) return;
    }
    const delay = RETRY_DELAYS_MS[Math.min(attempt++, RETRY_DELAYS_MS.length - 1)];
    await new Promise((r) => setTimeout(r, delay));
  }
}

// ---------------------------------------------------------------------------------------------
// Dev test mode (scripted suggestions). The API refuses these routes unless test mode is on.
// ---------------------------------------------------------------------------------------------

export interface TestTranscriptLine {
  segmentId: string;
  speakerId: string;
  speakerName?: string | null;
  text: string;
  language: string;
  timestampMs: number;
}

let testModeProbe: Promise<boolean> | null = null;

/** True when the API runs with a suggestion test script (dev only). Cached per page load. */
export function suggestionTestModeEnabled(): Promise<boolean> {
  testModeProbe ??= apiClient
    .get<{ suggestionScript?: boolean }>("/dev/test-mode")
    .then((r) => r.data?.suggestionScript === true)
    .catch(() => false);
  return testModeProbe;
}

/** Publishes one typed line as a final transcript segment, so the real suggestion engine sees it. */
export async function sendTestTranscriptLine(
  roomId: string,
  body: { text: string; language?: string; speaker?: string },
): Promise<TestTranscriptLine> {
  const { data } = await apiClient.post<TestTranscriptLine>(`/meetings/${encodeURIComponent(roomId)}/dev/transcript`, body);
  return data;
}
