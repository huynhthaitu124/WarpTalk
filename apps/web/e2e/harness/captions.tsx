// Test harness served by the Vite dev server (root = apps/web): mounts the meeting caption UI with
// scripted data-channel messages so caption rendering is testable without a LiveKit server.
import { createRoot } from 'react-dom/client';
import { mergeCaption, parseCaption, type CaptionLine } from '@v2/lib/captions';
import type { RoomStatus } from '@v2/lib/schemas';
import { CaptionsPanel, StatusStrip } from '@v2/features/MeetingParts';

export interface HarnessInput {
  /** Raw payloads as the pipeline publishes them (JSON strings or plain text). */
  messages: { payload: string; now: number }[];
  translationLang: string;
  room: { status: RoomStatus; translationActive: boolean };
  connection: 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'disconnected';
  quality: 'excellent' | 'good' | 'poor' | 'lost' | 'unknown';
}

export function mount(el: HTMLElement, i: HarnessInput) {
  const enc = new TextEncoder();
  let lines: CaptionLine[] = [];
  i.messages.forEach((m, n) => {
    const l = parseCaption(enc.encode(m.payload), 'Speaker', n + 1, m.now);
    if (l) lines = mergeCaption(lines, l);
  });
  createRoot(el).render(
    <div className="flex flex-col gap-4 p-6">
      <StatusStrip connection={i.connection} quality={i.quality} room={i.room} />
      <CaptionsPanel lines={lines} translationLang={i.translationLang} />
    </div>,
  );
}
