// Loaded lazily: livekit-client is large and only needed once a meeting is joined.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  LiveKitRoom, RoomAudioRenderer, useConnectionQualityIndicator, useConnectionState, useDataChannel, useLocalParticipant, useParticipants, useRoomContext,
} from '@livekit/components-react';
import { RoomEvent, Track } from 'livekit-client';
import { Alert, useToast } from '@/ui';
import type { JoinGrant, Room } from '@/lib/schemas';
import { mergeCaption, parseCaption, type CaptionLine } from '@/lib/captions';
import { CaptionsPanel, MeetingControls, ParticipantGrid, StatusStrip, type ConnectionLabel, type QualityLabel, type Tile } from './MeetingParts';

export { parseCaption };

function connectionLabel(s: string): ConnectionLabel {
  if (s === 'connected') return 'connected';
  if (s === 'connecting') return 'connecting';
  if (s === 'reconnecting' || s === 'signalReconnecting') return 'reconnecting';
  return 'disconnected';
}

function Stage({ listen, onListen, onLeave, roomInfo, captionLang }: { listen: string; onListen(v: string): void; onLeave(): void; roomInfo: Room; captionLang: string }) {
  const room = useRoomContext();
  const participants = useParticipants();
  const { localParticipant, isMicrophoneEnabled } = useLocalParticipant();
  const toast = useToast();
  const connection = useConnectionState();
  const { quality } = useConnectionQualityIndicator({ participant: localParticipant });
  const [lines, setLines] = useState<CaptionLine[]>([]);
  const seq = useRef(0);

  useDataChannel('captions', (msg) => {
    const line = parseCaption(msg.payload, msg.from?.name || msg.from?.identity || 'Speaker', ++seq.current);
    if (!line) return;
    setLines((cur) => mergeCaption(cur, line));
  });

  // Subscribe to the original mix, or to exactly one dubbed track `dub-{lang}`.
  useEffect(() => {
    const apply = () => {
      for (const p of room.remoteParticipants.values()) {
        for (const pub of p.trackPublications.values()) {
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

  const toggleMic = useCallback(async () => {
    try { await localParticipant.setMicrophoneEnabled(!isMicrophoneEnabled); } catch (e) {
      toast({ tone: 'danger', title: 'Microphone unavailable', description: e instanceof Error ? e.message : 'Permission denied' });
    }
  }, [localParticipant, isMicrophoneEnabled, toast]);

  const tiles = useMemo<Tile[]>(() => participants.map((p) => ({
    id: p.identity, name: p.name || p.identity, speaking: p.isSpeaking, micOn: p.isMicrophoneEnabled, isLocal: p.isLocal,
  })), [participants]);

  return (
    <div className="flex flex-col gap-6">
      <StatusStrip connection={connectionLabel(connection)} quality={quality as QualityLabel} room={roomInfo} />
      <div className="grid gap-6 lg:grid-cols-[1fr_360px]">
        <ParticipantGrid tiles={tiles} />
        <CaptionsPanel lines={lines} translationLang={captionLang} />
      </div>
      <MeetingControls micOn={isMicrophoneEnabled} onToggleMic={() => void toggleMic()} listen={listen} onListen={onListen} onLeave={onLeave} />
      <RoomAudioRenderer />
    </div>
  );
}

export default function LiveKitStage({ grant, listen, onListen, onLeave, room, captionLang }: { grant: JoinGrant; listen: string; onListen(v: string): void; onLeave(): void; room: Room; captionLang: string }) {
  const [error, setError] = useState<string | null>(null);
  return (
    <LiveKitRoom
      serverUrl={grant.serverUrl}
      token={grant.token}
      connect
      audio={false}
      video={false}
      connectOptions={{ autoSubscribe: false }}
      onError={(e) => setError(e.message)}
      onDisconnected={onLeave}
      data-lk-theme="default"
    >
      {error && <Alert tone="danger" title="Connection problem" className="mb-4">{error}</Alert>}
      <Stage listen={listen} onListen={onListen} onLeave={onLeave} roomInfo={room} captionLang={captionLang} />
    </LiveKitRoom>
  );
}
