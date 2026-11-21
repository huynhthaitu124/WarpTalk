// Loaded lazily: livekit-client is large and only needed once a meeting is joined.
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { LiveKitRoom, RoomAudioRenderer, useDataChannel, useLocalParticipant, useParticipants, useRoomContext } from '@livekit/components-react';
import { RoomEvent, Track } from 'livekit-client';
import { Alert, useToast } from '@/ui';
import type { JoinGrant } from '@/lib/schemas';
import { CaptionsPanel, MeetingControls, ParticipantGrid, type CaptionLine, type Tile } from './MeetingParts';

/** Captions arrive on data topic "captions": JSON {id?, text, speaker?, language?, final?} or plain text. */
export function parseCaption(payload: Uint8Array, fallbackSpeaker: string, seq: number): CaptionLine | null {
  const raw = new TextDecoder().decode(payload).trim();
  if (!raw) return null;
  try {
    const j = JSON.parse(raw) as Record<string, unknown>;
    const text = String(j.text ?? j.caption ?? '');
    if (!text) return null;
    return {
      id: String(j.id ?? j.segmentId ?? `c${seq}`),
      speaker: String(j.speaker ?? j.speakerName ?? j.participantName ?? fallbackSpeaker),
      text,
      lang: typeof j.language === 'string' ? j.language : typeof j.lang === 'string' ? j.lang : undefined,
      final: j.final === undefined && j.isFinal === undefined ? true : Boolean(j.final ?? j.isFinal),
    };
  } catch {
    return { id: `c${seq}`, speaker: fallbackSpeaker, text: raw, final: true };
  }
}

function Stage({ listen, onListen, onLeave }: { listen: string; onListen(v: string): void; onLeave(): void }) {
  const room = useRoomContext();
  const participants = useParticipants();
  const { localParticipant, isMicrophoneEnabled } = useLocalParticipant();
  const toast = useToast();
  const [lines, setLines] = useState<CaptionLine[]>([]);
  const seq = useRef(0);

  useDataChannel('captions', (msg) => {
    const line = parseCaption(msg.payload, msg.from?.name || msg.from?.identity || 'Speaker', ++seq.current);
    if (!line) return;
    setLines((cur) => {
      const i = cur.findIndex((c) => c.id === line.id);
      const next = i >= 0 ? cur.map((c, k) => (k === i ? line : c)) : [...cur, line];
      return next.slice(-200);
    });
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
      <div className="grid gap-6 lg:grid-cols-[1fr_360px]">
        <ParticipantGrid tiles={tiles} />
        <CaptionsPanel lines={lines} />
      </div>
      <MeetingControls micOn={isMicrophoneEnabled} onToggleMic={() => void toggleMic()} listen={listen} onListen={onListen} onLeave={onLeave} />
      <RoomAudioRenderer />
    </div>
  );
}

export default function LiveKitStage({ grant, listen, onListen, onLeave }: { grant: JoinGrant; listen: string; onListen(v: string): void; onLeave(): void }) {
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
      <Stage listen={listen} onListen={onListen} onLeave={onLeave} />
    </LiveKitRoom>
  );
}
