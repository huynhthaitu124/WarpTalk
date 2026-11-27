// Loaded lazily (livekit-client is large). Joins the bridge room as a silent listener: no audio is
// subscribed or played here; the macOS app plays the dub natively and only captions are read.
import { useEffect, useRef } from 'react';
import { LiveKitRoom, useDataChannel, useLocalParticipant } from '@livekit/components-react';
import { parseCaption, senderLabel, type CaptionLine } from '@v2/lib/captions';
import type { JoinGrant } from '@v2/lib/schemas';

function Listener({ onCaption }: { onCaption(l: CaptionLine): void }) {
  const seq = useRef(0);
  useDataChannel('captions', (msg) => {
    const line = parseCaption(msg.payload, senderLabel(msg.from) || 'Speaker', ++seq.current);
    if (line) onCaption(line);
  });
  return null;
}

// `LiveKitRoom` only applies its `audio` prop when it connects, so muting later would keep the
// microphone published. Follow the prop for the whole session.
function MicSync({ on }: { on: boolean }) {
  const { localParticipant } = useLocalParticipant();
  useEffect(() => {
    if (localParticipant.isMicrophoneEnabled === on) return;
    localParticipant.setMicrophoneEnabled(on).catch(() => undefined);
  }, [localParticipant, on]);
  return null;
}

export default function ExternalCaptions({
  grant, onCaption, onConnected, onDisconnected, onError, audio = false,
}: {
  grant: JoinGrant;
  onCaption(l: CaptionLine): void;
  onConnected(): void;
  onDisconnected(): void;
  onError(message: string): void;
  /** Publish the microphone (copilot mode; needs a non-listener grant). */
  audio?: boolean;
}) {
  return (
    <LiveKitRoom
      serverUrl={grant.serverUrl}
      token={grant.token}
      connect
      audio={audio}
      video={false}
      connectOptions={{ autoSubscribe: false }}
      options={{ publishDefaults: { stopMicTrackOnMute: true } }}
      onConnected={onConnected}
      onDisconnected={onDisconnected}
      onError={(e) => onError(e.message)}
    >
      <Listener onCaption={onCaption} />
      <MicSync on={audio} />
    </LiveKitRoom>
  );
}
