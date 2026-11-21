// Loaded lazily: livekit-client is large and only needed once a meeting is joined.
// This file only adapts a real LiveKit room to the MeetingSession the UI consumes (meeting/InMeeting.tsx).
import { useEffect, useMemo, useReducer } from 'react';
import { LiveKitRoom, RoomAudioRenderer, useConnectionQualityIndicator, useConnectionState, useRoomContext } from '@livekit/components-react';
import { DisconnectReason, RoomEvent, Track, type Participant, type RemoteTrackPublication, type TrackPublication } from 'livekit-client';
import { useToast } from '@/ui';
import type { JoinGrant, Room } from '@/lib/schemas';
import { parseCaption } from '@/lib/captions';
import type { ConnectionLabel, QualityLabel } from './MeetingParts';
import { InMeeting } from './meeting/InMeeting';
import type { DataMessage, DataTopic, MeetingSession, SessionParticipant, VideoSource } from './meeting/types';

export { parseCaption };

export interface JoinMedia { mic: boolean; cam: boolean; micId?: string; camId?: string }

function connectionLabel(s: string): ConnectionLabel {
  if (s === 'connected') return 'connected';
  if (s === 'connecting') return 'connecting';
  if (s === 'reconnecting' || s === 'signalReconnecting') return 'reconnecting';
  return 'disconnected';
}

const TICK_EVENTS = [
  RoomEvent.ActiveSpeakersChanged, RoomEvent.TrackMuted, RoomEvent.TrackUnmuted, RoomEvent.TrackPublished, RoomEvent.TrackUnpublished,
  RoomEvent.TrackSubscribed, RoomEvent.TrackUnsubscribed, RoomEvent.LocalTrackPublished, RoomEvent.LocalTrackUnpublished,
  RoomEvent.ParticipantConnected, RoomEvent.ParticipantDisconnected, RoomEvent.ParticipantNameChanged, RoomEvent.ConnectionStateChanged,
] as const;

const enc = new TextEncoder();

function source(pub: TrackPublication | undefined): VideoSource | undefined {
  const t = pub?.track;
  if (!t || pub?.isMuted) return undefined;
  return { attach: (el) => { t.attach(el); }, detach: (el) => { t.detach(el); } };
}

function toParticipant(p: Participant): SessionParticipant {
  const cam = p.getTrackPublication(Track.Source.Camera);
  const scr = p.getTrackPublication(Track.Source.ScreenShare);
  const screen = source(scr);
  return {
    identity: p.identity,
    name: p.name || p.identity,
    isLocal: p.isLocal,
    speaking: p.isSpeaking,
    micOn: p.isMicrophoneEnabled,
    camOn: p.isCameraEnabled,
    sharing: !!screen,
    video: source(cam),
    screen,
  };
}

function Stage({ listen, onListen, onLeave, onEnded, roomInfo, captionLang, selfUserId, selfName }: {
  listen: string; onListen(v: string): void; onLeave(): void; onEnded(): void; roomInfo: Room; captionLang: string; selfUserId: string | null; selfName: string;
}) {
  const room = useRoomContext();
  const connection = useConnectionState();
  const { quality } = useConnectionQualityIndicator({ participant: room.localParticipant });
  const [tick, bump] = useReducer((n: number) => n + 1, 0);

  useEffect(() => {
    for (const e of TICK_EVENTS) room.on(e, bump);
    return () => { for (const e of TICK_EVENTS) room.off(e, bump); };
  }, [room]);

  // Subscribe to the original mix, or to exactly one dubbed track `dub-{lang}`.
  useEffect(() => {
    const apply = () => {
      for (const p of room.remoteParticipants.values()) {
        for (const pub of p.trackPublications.values() as IterableIterator<RemoteTrackPublication>) {
          if (pub.kind === Track.Kind.Audio) {
            const dub = pub.trackName.startsWith('dub-');
            pub.setSubscribed(listen === 'original' ? !dub : pub.trackName === `dub-${listen}`);
          } else {
            pub.setSubscribed(true);
          }
        }
      }
    };
    apply();
    room.on(RoomEvent.TrackPublished, apply).on(RoomEvent.ParticipantConnected, apply);
    return () => { room.off(RoomEvent.TrackPublished, apply).off(RoomEvent.ParticipantConnected, apply); };
  }, [room, listen]);

  const actions = useMemo(() => {
    const lp = room.localParticipant;
    return {
      canShare: typeof navigator !== 'undefined' && !!navigator.mediaDevices?.getDisplayMedia,
      toggleMic: async () => { await lp.setMicrophoneEnabled(!lp.isMicrophoneEnabled); },
      toggleCamera: async () => { await lp.setCameraEnabled(!lp.isCameraEnabled); },
      toggleShare: async () => { await lp.setScreenShareEnabled(!lp.isScreenShareEnabled); },
      send: async (topic: 'chat' | 'reactions', payload: unknown) => { await lp.publishData(enc.encode(JSON.stringify(payload)), { reliable: true, topic }); },
      subscribe: (topic: DataTopic, handler: (m: DataMessage) => void) => {
        const on = (payload: Uint8Array, from?: { identity: string; name?: string }, _kind?: unknown, t?: string) => {
          if (t === topic) handler({ payload, from: from ? { identity: from.identity, name: from.name || from.identity } : undefined });
        };
        room.on(RoomEvent.DataReceived, on);
        return () => { room.off(RoomEvent.DataReceived, on); };
      },
    };
  }, [room]);
  const session = useMemo<MeetingSession>(() => ({
    ...actions,
    connection: connectionLabel(connection),
    quality: quality as QualityLabel,
    participants: [room.localParticipant, ...room.remoteParticipants.values()].map(toParticipant),
    // `tick` re-derives participants on every room event.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }), [actions, room, connection, quality, tick]);

  return (
    <>
      <InMeeting room={roomInfo} session={session} selfUserId={selfUserId} selfName={selfName} listen={listen} onListen={onListen} captionLang={captionLang} onLeave={onLeave} onEnded={onEnded} />
      <RoomAudioRenderer />
    </>
  );
}

export default function LiveKitStage({ grant, media, listen, onListen, onLeave, onEnded, onDisconnected, room, captionLang, selfUserId, selfName }: {
  grant: JoinGrant; media: JoinMedia; listen: string; onListen(v: string): void; onLeave(): void; onEnded(): void;
  /** Connection closed without the user leaving (host ended it, removed, network). */
  onDisconnected(kind: 'ended' | 'removed' | 'lost'): void; room: Room; captionLang: string; selfUserId: string | null; selfName: string;
}) {
  const toast = useToast();
  return (
    <LiveKitRoom
      serverUrl={grant.serverUrl}
      token={grant.token}
      connect
      audio={media.mic ? (media.micId ? { deviceId: media.micId } : true) : false}
      video={media.cam ? (media.camId ? { deviceId: media.camId } : true) : false}
      connectOptions={{ autoSubscribe: false }}
      onError={(e) => toast({ tone: 'danger', title: 'Connection problem', description: e.message })}
      onMediaDeviceFailure={() => toast({ tone: 'warning', title: 'Camera or microphone unavailable', description: 'You joined without it. Check browser permissions.' })}
      onDisconnected={(reason) => onDisconnected(reason === DisconnectReason.ROOM_DELETED ? 'ended' : reason === DisconnectReason.PARTICIPANT_REMOVED ? 'removed' : 'lost')}
      data-lk-theme="default"
    >
      <Stage listen={listen} onListen={onListen} onLeave={onLeave} onEnded={onEnded} roomInfo={room} captionLang={captionLang} selfUserId={selfUserId} selfName={selfName} />
    </LiveKitRoom>
  );
}
