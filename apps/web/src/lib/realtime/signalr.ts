import * as signalR from "@microsoft/signalr";
import { createMeetingChatConnection, createTranslationRoomConnection } from "./meeting-transport";
import { createAssistantConnection } from "./assistant-transport";

/** True when a hub failure was the gateway refusing the token rather than a transport blip. */
export function isUnauthorizedHubError(error: unknown): boolean {
  if (!error) return false;

  const statusCode = (error as { statusCode?: unknown }).statusCode;
  if (statusCode === 401 || statusCode === 403) return true;

  const message = error instanceof Error ? error.message : String(error);
  return /Status code '(401|403)'/.test(message);
}

type HubPath =
  | "/hubs/translation-room"
  | "/hubs/notification"
  | "/hubs/billing"
  | "/api/v1/meetings/chat-hub"
  | "/api/v1/assistant/chat-hub";

/**
 * Hubs the gateway reaches through its YARP proxy, i.e. hosted by another service that runs
 * several replicas behind a Kubernetes Service. Nothing pins a client to one of those pods:
 * negotiate and the WebSocket upgrade are two separate proxied requests, can land on two
 * different replicas, and the second one has never heard of the connection id the first issued.
 *
 * So these connect WebSockets-only with `skipNegotiation`: the WebSocket request itself creates
 * the connection on whichever replica receives it. The servers accept only WebSockets on these
 * paths (warptalk-backend #428, SignalRBackplaneExtensions.UseWebSocketsOnly), and their Redis
 * backplane delivers sends made on any replica.
 *
 * The gateway's own hubs are NOT in this list on purpose: Traefik's sticky cookie keeps negotiate
 * and connect on one gateway pod, and they keep the Long Polling fallback for networks that block
 * WebSockets.
 */
export const SKIP_NEGOTIATION_HUBS: readonly HubPath[] = [
  "/api/v1/meetings/chat-hub",
  "/api/v1/assistant/chat-hub",
];

/** Transport options for one hub; see SKIP_NEGOTIATION_HUBS. */
export function hubTransportOptions(
  hubPath: HubPath
): Pick<signalR.IHttpConnectionOptions, "transport" | "skipNegotiation"> {
  if (SKIP_NEGOTIATION_HUBS.includes(hubPath)) {
    return { transport: signalR.HttpTransportType.WebSockets, skipNegotiation: true };
  }
  return { transport: signalR.HttpTransportType.WebSockets | signalR.HttpTransportType.LongPolling };
}

export class RealtimeUnavailableError extends Error {
  constructor(readonly hubPath: string) {
    super(`No realtime channel for ${hubPath} on this backend`);
    this.name = "RealtimeUnavailableError";
  }
}

/**
 * A hub with no equivalent on the Rust backend (notifications, billing). It never
 * connects: `start()` rejects once with RealtimeUnavailableError and the features that need it
 * fall back to REST or stay hidden. No network traffic, no retries.
 */
class UnavailableHub {
  readonly state = signalR.HubConnectionState.Disconnected;
  constructor(private readonly hubPath: string) {}
  on() {}
  off() {}
  onreconnecting() {}
  onreconnected() {}
  onclose() {}
  async start(): Promise<void> {
    throw new RealtimeUnavailableError(this.hubPath);
  }
  async stop(): Promise<void> {}
  async invoke(): Promise<never> {
    throw new RealtimeUnavailableError(this.hubPath);
  }
  async send(): Promise<never> {
    throw new RealtimeUnavailableError(this.hubPath);
  }
}

/** Whether a hub has a realtime channel on this backend. */
export function hubAvailable(hubPath: HubPath): boolean {
  return (
    hubPath === "/hubs/translation-room" ||
    hubPath === "/api/v1/meetings/chat-hub" ||
    hubPath === "/api/v1/assistant/chat-hub"
  );
}

/**
 * Realtime connection for one of the hubs the UI was written against.
 *
 * The Rust backend has no SignalR. The meeting hubs are served by the LiveKit room plus REST
 * (lib/realtime/meeting-transport.ts) and WarpBot by a per-conversation SSE stream
 * (lib/realtime/assistant-transport.ts), both behind the same `on`/`invoke` surface; the others
 * are unavailable (see UnavailableHub).
 */
export function createHubConnection(hubPath: HubPath): signalR.HubConnection {
  let connection: unknown;
  if (hubPath === "/hubs/translation-room") connection = createTranslationRoomConnection();
  else if (hubPath === "/api/v1/meetings/chat-hub") connection = createMeetingChatConnection();
  else if (hubPath === "/api/v1/assistant/chat-hub") connection = createAssistantConnection();
  else connection = new UnavailableHub(hubPath);
  // Callers are typed against HubConnection; every member they use is implemented.
  return connection as signalR.HubConnection;
}
