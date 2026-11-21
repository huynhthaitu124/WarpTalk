import type { ConnectionLabel, QualityLabel } from '../MeetingParts';

/** Something a `<video>` element can show (a LiveKit track in production, a canvas stream in tests). */
export interface VideoSource {
  attach(el: HTMLVideoElement): void;
  detach(el: HTMLVideoElement): void;
}

export interface SessionParticipant {
  /** LiveKit identity; equals the user id for people who joined through the API. */
  identity: string;
  name: string;
  isLocal: boolean;
  speaking: boolean;
  micOn: boolean;
  camOn: boolean;
  sharing: boolean;
  video?: VideoSource;
  screen?: VideoSource;
}

export interface DataMessage {
  payload: Uint8Array;
  from?: { identity: string; name: string };
}

export type DataTopic = 'chat' | 'reactions' | 'captions';

/**
 * The slice of a LiveKit room the meeting UI needs. LiveKitStage adapts a real room to it and the
 * e2e harness (e2e/harness/meeting.tsx) provides a scripted one, so the whole in-meeting UI is
 * testable without a media server.
 */
export interface MeetingSession {
  connection: ConnectionLabel;
  quality: QualityLabel;
  participants: SessionParticipant[];
  toggleMic(): Promise<void>;
  toggleCamera(): Promise<void>;
  toggleShare(): Promise<void>;
  /** Publishes JSON on a data topic to everybody else in the room. */
  send(topic: Exclude<DataTopic, 'captions'>, payload: unknown): Promise<void>;
  subscribe(topic: DataTopic, handler: (m: DataMessage) => void): () => void;
  canShare: boolean;
}
