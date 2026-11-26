/**
 * WarpBot's live channel on the Rust backend.
 *
 * The .NET system streamed replies over the AssistantHub (SignalR). The Rust API serves the same
 * events as Server-Sent Events on `GET /assistant/conversations/{id}/events`: event name = hub
 * event name, data = its JSON payload, id = a resume cursor. This module puts that stream behind
 * the hub surface the UI was written against (`on`, `invoke("JoinConversation")`, reconnect
 * callbacks), so the chat components keep one event model.
 *
 * The stream is read with `fetch`, not `EventSource`, so the access token travels in the
 * Authorization header and never in a URL. A dropped stream reconnects with `Last-Event-ID`, so
 * no chunk is lost or repeated across a blip.
 */
import { HubConnectionState } from "@microsoft/signalr";
import { getUsableAccessToken } from "@/lib/api/client";
import { rustApiBaseUrl } from "@/lib/api/rust-compat";
import { RealtimeNotSupportedError, type RealtimeConnection } from "./meeting-transport";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Handler = (...args: any[]) => void;

/** The server refused the stream for good (not a member, conversation gone). */
export class AssistantStreamRefusedError extends Error {
  constructor(readonly status: number) {
    super(`WarpBot stream refused (${status})`);
    this.name = "AssistantStreamRefusedError";
  }
}

const RETRY_DELAYS_MS = [1_000, 2_000, 5_000, 10_000, 30_000];

interface Stream {
  conversationId: string;
  abort: AbortController;
  lastEventId: string | null;
  attempt: number;
  live: boolean;
  /** Resolves when the server says the stream is live (`ready`). */
  ready: Promise<void>;
}

class AssistantChannel implements RealtimeConnection {
  state: HubConnectionState = HubConnectionState.Disconnected;
  private handlers = new Map<string, Set<Handler>>();
  private reconnecting: ((error?: Error) => void)[] = [];
  private reconnected: ((connectionId?: string) => void)[] = [];
  private closed: ((error?: Error) => void)[] = [];
  private stream: Stream | null = null;

  on(event: string, handler: Handler) {
    let set = this.handlers.get(event);
    if (!set) this.handlers.set(event, (set = new Set()));
    set.add(handler);
  }

  off(event: string, handler?: Handler) {
    if (!handler) this.handlers.delete(event);
    else this.handlers.get(event)?.delete(handler);
  }

  onreconnecting(cb: (error?: Error) => void) {
    this.reconnecting.push(cb);
  }

  onreconnected(cb: (connectionId?: string) => void) {
    this.reconnected.push(cb);
  }

  onclose(cb: (error?: Error) => void) {
    this.closed.push(cb);
  }

  /** No network until a conversation is joined: the stream is per conversation. */
  async start(): Promise<void> {
    this.state = HubConnectionState.Connected;
  }

  async stop(): Promise<void> {
    if (this.state === HubConnectionState.Disconnected) return;
    this.closeStream();
    this.state = HubConnectionState.Disconnected;
    for (const cb of this.closed) cb();
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  async invoke<T = any>(method: string, ...args: unknown[]): Promise<T> {
    if (method === "JoinConversation") {
      await this.join(String(args[0] ?? ""));
      return undefined as T;
    }
    if (method === "LeaveConversation") {
      if (this.stream?.conversationId === String(args[0] ?? "")) this.closeStream();
      return undefined as T;
    }
    throw new RealtimeNotSupportedError(method);
  }

  async send(method: string, ...args: unknown[]): Promise<void> {
    await this.invoke(method, ...args);
  }

  private emit(event: string, ...args: unknown[]) {
    for (const h of this.handlers.get(event) ?? []) {
      try {
        h(...args);
      } catch (error) {
        console.error(`[assistant] ${event} handler failed`, error);
      }
    }
  }

  private closeStream() {
    this.stream?.abort.abort();
    this.stream = null;
  }

  /**
   * Opens the conversation's stream and resolves once it is live, so a message sent afterwards
   * cannot outrun it. Joining the conversation already streaming is a no-op (the UI re-joins on
   * reconnect, and a fresh stream would drop the resume cursor).
   */
  private join(conversationId: string): Promise<void> {
    if (!conversationId) return Promise.reject(new Error("JoinConversation needs a conversation id"));
    if (this.state === HubConnectionState.Disconnected) {
      return Promise.reject(new Error("Connection is not started"));
    }
    if (this.stream?.conversationId === conversationId) return this.stream.ready;
    this.closeStream();

    let resolveReady!: () => void;
    let rejectReady!: (e: Error) => void;
    const ready = new Promise<void>((resolve, reject) => {
      resolveReady = resolve;
      rejectReady = reject;
    });
    // A caller that does not await the join must not surface an unhandled rejection.
    ready.catch(() => undefined);
    const stream: Stream = {
      conversationId,
      abort: new AbortController(),
      lastEventId: null,
      attempt: 0,
      live: false,
      ready,
    };
    this.stream = stream;
    void this.run(stream, resolveReady, rejectReady);
    return ready;
  }

  private async run(stream: Stream, resolveReady: () => void, rejectReady: (e: Error) => void) {
    let settled = false;
    const isCurrent = () => this.stream === stream && !stream.abort.signal.aborted;

    while (isCurrent()) {
      let failure: Error | null = null;
      try {
        await this.readOnce(stream, () => {
          stream.attempt = 0;
          if (!settled) {
            settled = true;
            resolveReady();
          } else if (this.state === HubConnectionState.Reconnecting) {
            this.state = HubConnectionState.Connected;
            for (const cb of this.reconnected) cb();
          }
        });
        failure = new Error("WarpBot stream ended");
      } catch (error) {
        if (!isCurrent()) return;
        failure = error instanceof Error ? error : new Error(String(error));
      }
      if (!isCurrent()) return;

      if (failure instanceof AssistantStreamRefusedError) {
        this.stream = null;
        if (!settled) {
          settled = true;
          rejectReady(failure);
        }
        // The conversation is gone or no longer ours: nothing will ever bring it back.
        this.state = HubConnectionState.Disconnected;
        for (const cb of this.closed) cb(failure);
        return;
      }

      if (!settled) {
        // First attempt failed: tell the caller now, keep trying in the background.
        settled = true;
        rejectReady(failure);
      }
      if (this.state === HubConnectionState.Connected) {
        this.state = HubConnectionState.Reconnecting;
        for (const cb of this.reconnecting) cb(failure);
      }
      const delay = RETRY_DELAYS_MS[Math.min(stream.attempt, RETRY_DELAYS_MS.length - 1)];
      stream.attempt += 1;
      await sleep(delay, stream.abort.signal);
    }
  }

  /** One HTTP stream, read until it ends. `onReady` fires on the server's `ready` event. */
  private async readOnce(stream: Stream, onReady: () => void): Promise<void> {
    const token = await getUsableAccessToken();
    const headers: Record<string, string> = { Accept: "text/event-stream" };
    if (token) headers.Authorization = `Bearer ${token}`;
    if (stream.lastEventId) headers["Last-Event-ID"] = stream.lastEventId;

    const url = `${rustApiBaseUrl()}/assistant/conversations/${encodeURIComponent(stream.conversationId)}/events`;
    const res = await fetch(url, { headers, signal: stream.abort.signal, cache: "no-store" });
    if (res.status === 403 || res.status === 404) throw new AssistantStreamRefusedError(res.status);
    if (!res.ok || !res.body) throw new Error(`WarpBot stream failed (${res.status})`);

    const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
    let buffer = "";
    let event = "message";
    let data: string[] = [];
    let id: string | null = null;

    const dispatch = () => {
      if (id !== null) stream.lastEventId = id;
      if (event === "ready") onReady();
      else if (data.length) {
        try {
          this.emit(event, JSON.parse(data.join("\n")));
        } catch (error) {
          console.error(`[assistant] bad ${event} payload`, error);
        }
      }
      event = "message";
      data = [];
      id = null;
    };

    for (;;) {
      const { value, done } = await reader.read();
      if (done) return;
      buffer += value;
      let nl: number;
      while ((nl = buffer.search(/\r\n|\r|\n/)) >= 0) {
        const line = buffer.slice(0, nl);
        buffer = buffer.slice(nl + (buffer.startsWith("\r\n", nl) ? 2 : 1));
        if (line === "") {
          dispatch();
          continue;
        }
        if (line.startsWith(":")) continue; // keepalive comment
        const colon = line.indexOf(":");
        const field = colon < 0 ? line : line.slice(0, colon);
        let val = colon < 0 ? "" : line.slice(colon + 1);
        if (val.startsWith(" ")) val = val.slice(1);
        if (field === "event") event = val;
        else if (field === "data") data.push(val);
        else if (field === "id") id = val;
      }
    }
  }
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
  return new Promise((resolve) => {
    const t = setTimeout(resolve, ms);
    signal.addEventListener(
      "abort",
      () => {
        clearTimeout(t);
        resolve();
      },
      { once: true },
    );
  });
}

export function createAssistantConnection(): RealtimeConnection {
  return new AssistantChannel();
}
