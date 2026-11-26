/**
 * Realtime transport of the meeting room on the Rust backend.
 *
 * The .NET gateway pushed meeting events over SignalR hubs. The Rust system has no hub: its
 * realtime surface is
 *   - the LiveKit room itself: presence, track mute state, disconnect reasons;
 *   - LiveKit data topics (warptalk_contracts::media): `captions` (published by the pipeline),
 *     `chat` (instant copies of messages posted over REST, plus server `chat.translated`) and
 *     `reactions` (hand raises and emoji, published by clients);
 *   - REST for everything stateful: participant languages, the roster, room lifecycle.
 *
 * The meeting UI was written against the hub surface (`on(event)`, `invoke(method)`,
 * reconnect callbacks). This module implements that surface on top of the sources above so the
 * UI keeps one event model. Every event is produced from real data; a hub method the Rust
 * backend has no equivalent for rejects with `RealtimeNotSupportedError` and the UI hides the
 * control that would call it (see `realtimeSupports`).
 */
import { HubConnectionState } from "@microsoft/signalr";
import {
  ConnectionState,
  DisconnectReason,
  ParticipantEvent,
  RoomEvent,
  Track,
  type Participant,
  type RemoteParticipant,
  type Room,
  type TrackPublication,
} from "livekit-client";
import apiClient from "@/lib/api/client";
import { useAuthStore } from "@/stores/auth-store";
import { isRustChatMessage, rememberChatTranslations, setChatPublisher, toLegacyChatMessage } from "./meeting-chat";
import { followSuggestions } from "./meeting-suggestions";
import type {
  ParticipantInfoDto,
  TranscriptInterimDto,
  TranscriptSegmentDto,
  TranslationRoomStateDto,
  TranslationTextDto,
} from "@/types/realtime";

// ---------------------------------------------------------------------------------------------
// Room binding: the LiveKit Room is created by <LiveKitRoom>, so a child of it registers here.
// ---------------------------------------------------------------------------------------------

type RoomListener = (room: Room | null) => void;
let boundRoom: Room | null = null;
const roomListeners = new Set<RoomListener>();

/** Called from inside <LiveKitRoom> with its Room, and with null when it unmounts. */
export function bindMeetingRoom(room: Room | null): void {
  if (boundRoom === room) return;
  boundRoom = room;
  for (const l of roomListeners) l(room);
}

function onRoomBound(listener: RoomListener): () => void {
  roomListeners.add(listener);
  listener(boundRoom);
  return () => roomListeners.delete(listener);
}

// ---------------------------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------------------------

export class RealtimeNotSupportedError extends Error {
  constructor(readonly method: string) {
    super(`${method} is not supported by the meeting backend`);
    this.name = "RealtimeNotSupportedError";
  }
}

const TRANSLATION_ROOM_METHODS = new Set([
  "JoinTranslationRoom",
  "SetListenLanguage",
  "SetSpeakLanguage",
  "ToggleMute",
  "RaiseHand",
  "SendReaction",
]);
const CHAT_METHODS = new Set(["JoinMeetingRoom"]);

/** Whether the meeting backend implements this hub method; controls that need it are hidden otherwise. */
export function realtimeSupports(method: string): boolean {
  return TRANSLATION_ROOM_METHODS.has(method) || CHAT_METHODS.has(method);
}

// ---------------------------------------------------------------------------------------------
// Hub-compatible base
// ---------------------------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Handler = (...args: any[]) => void;

/** The part of signalR.HubConnection the meeting UI uses. */
export interface RealtimeConnection {
  readonly state: HubConnectionState;
  on(event: string, handler: Handler): void;
  off(event: string, handler?: Handler): void;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  invoke<T = any>(method: string, ...args: any[]): Promise<T>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  send(method: string, ...args: any[]): Promise<void>;
  start(): Promise<void>;
  stop(): Promise<void>;
  onreconnecting(cb: (error?: Error) => void): void;
  onreconnected(cb: (connectionId?: string) => void): void;
  onclose(cb: (error?: Error) => void): void;
}

abstract class Channel implements RealtimeConnection {
  private handlers = new Map<string, Set<Handler>>();
  private reconnecting: ((error?: Error) => void)[] = [];
  private reconnected: ((connectionId?: string) => void)[] = [];
  private closed: ((error?: Error) => void)[] = [];
  private unbindRoom: (() => void) | null = null;
  private detachRoom: (() => void) | null = null;
  protected room: Room | null = null;
  state: HubConnectionState = HubConnectionState.Disconnected;

  on(event: string, handler: Handler) {
    let set = this.handlers.get(event);
    if (!set) this.handlers.set(event, (set = new Set()));
    set.add(handler);
  }

  off(event: string, handler?: Handler) {
    if (!handler) this.handlers.delete(event);
    else this.handlers.get(event)?.delete(handler);
  }

  protected emit(event: string, ...args: unknown[]) {
    for (const h of this.handlers.get(event) ?? []) {
      try {
        h(...args);
      } catch (error) {
        console.error(`[realtime] ${event} handler failed`, error);
      }
    }
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

  async start() {
    if (this.state === HubConnectionState.Connected) return;
    this.state = HubConnectionState.Connected;
    this.unbindRoom = onRoomBound((room) => this.setRoom(room));
  }

  async stop() {
    if (this.state === HubConnectionState.Disconnected) return;
    this.state = HubConnectionState.Disconnected;
    this.unbindRoom?.();
    this.unbindRoom = null;
    this.setRoom(null);
    this.onStop();
    for (const cb of this.closed) cb();
  }

  async send(method: string, ...args: unknown[]) {
    await this.invoke(method, ...args);
  }

  abstract invoke<T = unknown>(method: string, ...args: unknown[]): Promise<T>;
  /** Subscribe to the Room's events; returns the unsubscribe. */
  protected abstract attach(room: Room): () => void;
  protected onStop() {}

  private setRoom(room: Room | null) {
    if (room === this.room) return;
    this.detachRoom?.();
    this.detachRoom = null;
    this.room = room;
    if (!room) return;
    const onReconnecting = () => {
      this.state = HubConnectionState.Reconnecting;
      for (const cb of this.reconnecting) cb();
    };
    const onReconnected = () => {
      this.state = HubConnectionState.Connected;
      for (const cb of this.reconnected) cb();
    };
    room.on(RoomEvent.Reconnecting, onReconnecting);
    room.on(RoomEvent.Reconnected, onReconnected);
    const detach = this.attach(room);
    this.detachRoom = () => {
      room.off(RoomEvent.Reconnecting, onReconnecting);
      room.off(RoomEvent.Reconnected, onReconnected);
      detach();
    };
  }

  protected requireConnected() {
    if (this.state === HubConnectionState.Disconnected) throw new Error("Realtime connection is not started");
  }
}

const enc = new TextEncoder();
const dec = new TextDecoder();

function parseJson(payload: Uint8Array): Record<string, unknown> | null {
  try {
    const v: unknown = JSON.parse(dec.decode(payload));
    return typeof v === "object" && v !== null ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

const str = (v: unknown): string | undefined => (typeof v === "string" && v ? v : undefined);
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** Human participants only: the identity of a meeting participant is their user id. */
function isUserIdentity(identity: string): boolean {
  return UUID.test(identity);
}

async function publish(room: Room | null, topic: string, payload: unknown) {
  if (!room || room.state !== ConnectionState.Connected) throw new Error("Not connected to the meeting");
  await room.localParticipant.publishData(enc.encode(JSON.stringify(payload)), { reliable: true, topic });
}

// ---------------------------------------------------------------------------------------------
// Translation room
// ---------------------------------------------------------------------------------------------

interface RustParticipant {
  id: string;
  userId?: string | null;
  displayName: string;
  role: string;
  speakLanguage: string;
  listenLanguage: string;
  status: string;
  joinedAt?: string | null;
}

interface RustRoom {
  id: string;
  roomCode?: string;
  status: string;
  translationActive?: boolean;
  effectiveHostId?: string | null;
}

const ENDED = new Set(["ENDED", "CANCELLED", "EXPIRED", "FAILED"]);
/** Room lifecycle has no push channel on the Rust API; it is read at this interval. */
const LIFECYCLE_POLL_MS = 5000;
const LIFECYCLE_MAX_BACKOFF_MS = 30000;

class TranslationRoomChannel extends Channel {
  private roomId: string | null = null;
  private selfUserId: string | null = null;
  private timer: number | null = null;
  private failures = 0;
  private lastRoom: RustRoom | null = null;
  private roster = new Map<string, RustParticipant>();
  private selfStatus: string | null = null;
  private endedEmitted = false;
  private rosterRefresh: Promise<void> | null = null;
  /** The room's AI-suggestion stream (SSE), re-emitted as `AiSuggestionReceived`. */
  private suggestions: AbortController | null = null;

  async invoke<T>(method: string, ...args: unknown[]): Promise<T> {
    this.requireConnected();
    switch (method) {
      case "JoinTranslationRoom": {
        const [roomId, , speak, listen] = args as [string, string, string, string];
        this.useRoom(roomId);
        await this.putLanguages({ speakLanguage: speak || undefined, listenLanguage: listen || undefined });
        await this.refreshRoster(true);
        this.schedule(0);
        return undefined as T;
      }
      case "SetListenLanguage":
        await this.putLanguages({ listenLanguage: String(args[1] ?? "") || undefined });
        return undefined as T;
      case "SetSpeakLanguage":
        await this.putLanguages({ speakLanguage: String(args[1] ?? "") || undefined });
        return undefined as T;
      case "ToggleMute":
        // Mute state reaches every participant through the published track itself
        // (TrackMuted/TrackUnmuted below); there is nothing else to tell the server.
        return undefined as T;
      case "RaiseHand": {
        const raised = Boolean(args[1]);
        await publish(this.room, "reactions", { t: "hand", raised });
        // LiveKit does not echo data back to its sender.
        this.emit("HandRaised", this.room?.localParticipant.identity ?? "", raised);
        return undefined as T;
      }
      case "SendReaction": {
        const emoji = String(args[1] ?? "");
        await publish(this.room, "reactions", { t: "reaction", emoji });
        this.emit("ReactionReceived", this.room?.localParticipant.identity ?? "", emoji);
        return undefined as T;
      }
      default:
        throw new RealtimeNotSupportedError(method);
    }
  }

  protected onStop() {
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = null;
    this.suggestions?.abort();
    this.suggestions = null;
    this.roomId = null;
    this.lastRoom = null;
    this.roster.clear();
    this.selfStatus = null;
    this.endedEmitted = false;
  }

  private useRoom(roomId: string) {
    if (this.roomId === roomId) return;
    this.onStop();
    this.roomId = roomId;
    const ctl = new AbortController();
    this.suggestions = ctl;
    void followSuggestions(roomId, (s) => this.emit("AiSuggestionReceived", s), ctl.signal);
  }

  private async putLanguages(body: { speakLanguage?: string; listenLanguage?: string }) {
    if (!this.roomId) throw new Error("JoinTranslationRoom has not been called");
    if (!body.speakLanguage && !body.listenLanguage) return;
    await apiClient.put(`/translation-rooms/${this.roomId}/participants/me/languages`, body);
  }

  private schedule(delay: number) {
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => void this.tick(), delay);
  }

  private async tick() {
    this.timer = null;
    if (!this.roomId || this.state === HubConnectionState.Disconnected) return;
    try {
      await Promise.all([this.pollRoom(), this.refreshRoster(false)]);
      this.failures = 0;
    } catch {
      this.failures += 1;
    }
    if (!this.roomId || this.endedEmitted) return;
    const backoff = Math.min(LIFECYCLE_POLL_MS * 2 ** this.failures, LIFECYCLE_MAX_BACKOFF_MS);
    this.schedule(this.failures ? backoff : LIFECYCLE_POLL_MS);
  }

  private async pollRoom() {
    const roomId = this.roomId;
    if (!roomId) return;
    const { data } = await apiClient.get<RustRoom>(`/translation-rooms/${roomId}`);
    if (roomId !== this.roomId) return;
    const prev = this.lastRoom;
    this.lastRoom = data;
    if (ENDED.has(data.status)) {
      if (!this.endedEmitted) {
        this.endedEmitted = true;
        this.emit("TranslationRoomEnded", roomId);
      }
      return;
    }
    if (!prev) return; // first read is the baseline, not a transition
    if (!prev.translationActive && data.translationActive) this.emit("TranslationRoomStarted", this.stateDto(data));
    if (prev.translationActive && !data.translationActive) this.emit("TranslationStopped", roomId);
    if (data.effectiveHostId && prev.effectiveHostId !== data.effectiveHostId) this.emit("HostChanged", data.effectiveHostId);
  }

  private stateDto(room: RustRoom): TranslationRoomStateDto {
    return {
      translationRoomId: room.id,
      translationRoomCode: room.roomCode ?? "",
      status: room.status,
      participants: this.connectedInfos(),
    };
  }

  private connectedInfos(): ParticipantInfoDto[] {
    return [...this.roster.values()].filter((p) => p.status === "CONNECTED" && p.userId).map((p) => this.info(p));
  }

  private info(p: RustParticipant): ParticipantInfoDto {
    const lk = p.userId ? this.room?.getParticipantByIdentity(p.userId) : undefined;
    const mic = lk?.getTrackPublication(Track.Source.Microphone);
    return {
      userId: p.userId ?? p.id,
      displayName: p.displayName,
      speakLanguage: p.speakLanguage,
      listenLanguage: p.listenLanguage,
      isMuted: mic ? mic.isMuted : true,
      joinedAt: p.joinedAt ?? new Date().toISOString(),
      role: p.role === "HOST" ? "host" : "participant",
    };
  }

  /** Re-reads the roster; emits the full roster on `initial`, otherwise the changes. Coalesced. */
  private refreshRoster(initial: boolean): Promise<void> {
    if (this.rosterRefresh && !initial) return this.rosterRefresh;
    const run = (async () => {
      const roomId = this.roomId;
      if (!roomId) return;
      const { data } = await apiClient.get<RustParticipant[]>(`/translation-rooms/${roomId}/participants`);
      if (roomId !== this.roomId) return;
      const next = new Map<string, RustParticipant>();
      for (const p of data) if (p.userId) next.set(p.userId, p);
      const prev = this.roster;
      this.roster = next;
      this.trackSelf(next);
      if (initial) {
        this.emit("ParticipantRoster", this.connectedInfos());
        return;
      }
      for (const [userId, p] of next) {
        const was = prev.get(userId);
        if (p.status === "CONNECTED" && was?.status !== "CONNECTED") this.emit("ParticipantJoined", this.info(p));
        if (was?.status === "CONNECTED" && p.status !== "CONNECTED") this.emit("ParticipantLeft", userId);
      }
      for (const [userId, was] of prev) if (!next.has(userId) && was.status === "CONNECTED") this.emit("ParticipantLeft", userId);
    })();
    this.rosterRefresh = run.finally(() => {
      if (this.rosterRefresh === run) this.rosterRefresh = null;
    });
    return run;
  }

  private trackSelf(roster: Map<string, RustParticipant>) {
    const self = this.selfUserId ?? this.room?.localParticipant.identity ?? null;
    if (!self) return;
    const status = roster.get(self)?.status ?? null;
    const prev = this.selfStatus;
    this.selfStatus = status;
    if (prev === status || prev === null) return;
    if (prev === "WAITING" && status === "CONNECTED") this.emit("ParticipantAdmitted", self);
    if (status === "KICKED") this.emit("ParticipantKicked", self);
    if (status === "REJECTED") this.emit("ParticipantRejected", self);
  }

  /** The signed-in user's id, so admission is tracked before a LiveKit identity exists. */
  setSelfUserId(userId: string | null) {
    this.selfUserId = userId;
  }

  protected attach(room: Room): () => void {
    let seq = 0;
    const onData = (payload: Uint8Array, participant?: RemoteParticipant, _kind?: unknown, topic?: string) => {
      if (topic === "captions") {
        seq += 1;
        this.onCaption(payload, seq);
      } else if (topic === "reactions") {
        const j = parseJson(payload);
        const from = participant?.identity;
        if (!j || !from) return;
        if (j.t === "hand" && typeof j.raised === "boolean") this.emit("HandRaised", from, j.raised);
        if (j.t === "reaction" && typeof j.emoji === "string" && j.emoji.length > 0 && j.emoji.length <= 8) {
          this.emit("ReactionReceived", from, j.emoji);
        }
      }
    };
    const onPresence = (p: Participant) => {
      if (isUserIdentity(p.identity)) void this.refreshRoster(false).catch(() => undefined);
    };
    const onMute = (pub: TrackPublication, p: Participant) => {
      if (pub.source === Track.Source.Microphone && isUserIdentity(p.identity)) {
        this.emit("ParticipantMuteChanged", p.identity, pub.isMuted);
      }
    };
    const onDisconnected = (reason?: DisconnectReason) => {
      if (reason === DisconnectReason.ROOM_DELETED && !this.endedEmitted) {
        this.endedEmitted = true;
        this.emit("TranslationRoomEnded", this.roomId);
      } else if (reason === DisconnectReason.PARTICIPANT_REMOVED) {
        this.emit("ParticipantKicked", room.localParticipant.identity);
      } else if (reason === DisconnectReason.DUPLICATE_IDENTITY) {
        this.emit("ForceDisconnected", "duplicate_session");
      }
    };
    room.on(RoomEvent.DataReceived, onData);
    room.on(RoomEvent.ParticipantConnected, onPresence);
    room.on(RoomEvent.ParticipantDisconnected, onPresence);
    room.on(RoomEvent.TrackMuted, onMute);
    room.on(RoomEvent.TrackUnmuted, onMute);
    room.on(RoomEvent.Disconnected, onDisconnected);
    const local = room.localParticipant;
    const onLocalMute = (pub: TrackPublication) => onMute(pub, local);
    local.on(ParticipantEvent.TrackMuted, onLocalMute);
    local.on(ParticipantEvent.TrackUnmuted, onLocalMute);
    return () => {
      room.off(RoomEvent.DataReceived, onData);
      room.off(RoomEvent.ParticipantConnected, onPresence);
      room.off(RoomEvent.ParticipantDisconnected, onPresence);
      room.off(RoomEvent.TrackMuted, onMute);
      room.off(RoomEvent.TrackUnmuted, onMute);
      room.off(RoomEvent.Disconnected, onDisconnected);
      local.off(ParticipantEvent.TrackMuted, onLocalMute);
      local.off(ParticipantEvent.TrackUnmuted, onLocalMute);
    };
  }

  /** Pipeline caption (apps/pipeline/src/downstream.rs) -> the transcript events of the UI. */
  private onCaption(payload: Uint8Array, seq: number) {
    const j = parseJson(payload);
    if (!j) return;
    const segmentId = str(j.segment_id) ?? str(j.segmentId);
    if (!segmentId) return;
    const speakerId = str(j.speakerUserId) ?? str(j.speakerId) ?? str(j.speaker_id) ?? "";
    const speakerName = str(j.speakerName) ?? "";
    const text = typeof j.text === "string" ? j.text : "";
    const lang = str(j.lang) ?? "";
    const at = typeof j.ts_ms === "number" ? j.ts_ms : typeof j.emittedAtMs === "number" ? j.emittedAtMs : Date.now();

    if (j.type === "stt") {
      if (!text.trim()) return;
      if (j.is_early === true) {
        const interim: TranscriptInterimDto = { speakerId, speakerName, itemId: segmentId, text, language: lang };
        this.emit("TranscriptInterimReceived", interim);
        return;
      }
      const segment: TranscriptSegmentDto = {
        segmentId,
        speakerId,
        speakerName,
        originalText: text,
        originalLanguage: lang,
        translations: {},
        confidence: 1,
        startTimeMs: at,
        endTimeMs: at,
        receivedAt: Date.now(),
        sequenceOrder: typeof j.seq === "number" ? j.seq : seq,
      } as TranscriptSegmentDto;
      this.emit("TranscriptSegmentReceived", segment);
      return;
    }

    if (j.type === "translation") {
      const perSentence = (j.sentenceTranslations ?? {}) as Record<string, unknown>;
      const cumulative = (j.translations ?? {}) as Record<string, unknown>;
      const chunk = typeof j.sentenceIndex === "number" ? j.sentenceIndex : undefined;
      const wireId = str(j.wire_segment_id);
      for (const [targetLang, value] of Object.entries(chunk !== undefined && wireId ? perSentence : cumulative)) {
        if (typeof value !== "string" || !value) continue;
        const translation: TranslationTextDto = {
          segmentId: chunk !== undefined && wireId ? wireId : segmentId,
          sourceSegmentId: segmentId,
          speakerId,
          originalText: text,
          translatedText: value,
          sourceLang: lang,
          targetLang,
          startTimeMs: at,
          endTimeMs: at,
          chunkIndex: chunk,
        };
        this.emit("TranslationTextReceived", translation);
      }
    }
  }
}

// ---------------------------------------------------------------------------------------------
// Meeting chat
// ---------------------------------------------------------------------------------------------

class MeetingChatChannel extends Channel {
  private roomId: string | null = null;

  async invoke<T>(method: string, ...args: unknown[]): Promise<T> {
    this.requireConnected();
    if (method === "JoinMeetingRoom") {
      this.roomId = String(args[0] ?? "") || null;
      setChatPublisher((payload) => publish(this.room, "chat", payload));
      return undefined as T;
    }
    throw new RealtimeNotSupportedError(method);
  }

  protected onStop() {
    setChatPublisher(null);
    this.roomId = null;
  }

  protected attach(room: Room): () => void {
    const onData = (payload: Uint8Array, _p?: RemoteParticipant, _k?: unknown, topic?: string) => {
      if (topic !== "chat" || !this.roomId) return;
      const j = parseJson(payload);
      if (!j) return;
      if (j.type === "chat.translated" && typeof j.messageId === "string") {
        const t = (j.translations ?? {}) as Record<string, unknown>;
        const map: Record<string, string> = {};
        for (const [k, v] of Object.entries(t)) if (typeof v === "string" && v) map[k] = v;
        rememberChatTranslations(j.messageId, map);
        this.emit("ChatMessageTranslated", j.messageId, map);
        return;
      }
      if (j.type === "chat.hidden" && typeof j.messageId === "string") {
        this.emit("ChatMessageHidden", j.messageId);
        return;
      }
      if (isRustChatMessage(j) && j.content.trim()) {
        this.emit("ChatMessageReceived", toLegacyChatMessage({ ...j, hidden: false, translations: j.translations ?? {} }, this.roomId));
      }
    };
    room.on(RoomEvent.DataReceived, onData);
    return () => room.off(RoomEvent.DataReceived, onData);
  }
}

// ---------------------------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------------------------

export function createTranslationRoomConnection(): RealtimeConnection {
  const channel = new TranslationRoomChannel();
  channel.setSelfUserId(useAuthStore.getState().user?.id ?? null);
  return channel;
}

export function createMeetingChatConnection(): RealtimeConnection {
  return new MeetingChatChannel();
}
