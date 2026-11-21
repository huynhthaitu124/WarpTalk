// Test harness served by the Vite dev server (root = apps/web): mounts the real in-meeting UI
// (InMeeting, Offline, Ended) on top of a signed-in page with a scripted MeetingSession instead of a
// LiveKit room. Everything that talks to the backend (roster, admit/deny, start/stop translation,
// end meeting) is the real code calling the real API; only the media layer is fake.
import { createRoot } from 'react-dom/client';
import { useSyncExternalStore } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { ToastProvider, TooltipProvider } from '@/ui';
import { useParticipants, useRoom } from '@/lib/queries';
import { InMeeting } from '@/features/meeting/InMeeting';
import { EndedView, OfflineRoom } from '@/features/meeting/Offline';
import type { DataMessage, DataTopic, MeetingSession, SessionParticipant } from '@/features/meeting/types';
import type { ConnectionLabel, QualityLabel } from '@/features/MeetingParts';

export interface MockParticipantInit { identity: string; name: string; isLocal?: boolean; micOn?: boolean; camOn?: boolean }
export interface HarnessOptions {
  roomId: string;
  selfUserId: string;
  selfName: string;
  participants: MockParticipantInit[];
  listen?: string;
  captionLang?: string;
}

const enc = new TextEncoder();

/** A canvas stream standing in for a camera. */
function fakeCamera(colour: string): SessionParticipant['video'] {
  return {
    attach(el) {
      const c = document.createElement('canvas');
      c.width = 320; c.height = 180;
      const g = c.getContext('2d');
      if (g) { g.fillStyle = colour; g.fillRect(0, 0, 320, 180); }
      el.srcObject = c.captureStream(1);
      void el.play().catch(() => undefined);
    },
    detach(el) { el.srcObject = null; },
  };
}

export class MockMeeting {
  connection: ConnectionLabel = 'connected';
  quality: QualityLabel = 'good';
  participants: SessionParticipant[];
  /** Everything the UI published (it is not echoed back to itself, like LiveKit). */
  sent: { topic: string; payload: unknown }[] = [];
  private handlers = new Map<DataTopic, Set<(m: DataMessage) => void>>();
  private subs = new Set<() => void>();
  private snap: MeetingSession;

  constructor(init: MockParticipantInit[]) {
    this.participants = init.map((p) => ({ identity: p.identity, name: p.name, isLocal: !!p.isLocal, speaking: false, micOn: !!p.micOn, camOn: !!p.camOn, sharing: false, video: p.camOn ? fakeCamera('#2f6fd0') : undefined }));
    this.snap = this.build();
  }

  private build(): MeetingSession {
    return {
      connection: this.connection,
      quality: this.quality,
      participants: this.participants,
      canShare: true,
      toggleMic: this.toggleMic,
      toggleCamera: this.toggleCamera,
      toggleShare: this.toggleShare,
      send: this.send,
      subscribe: this.subscribe,
    };
  }
  private emit() { this.snap = this.build(); this.subs.forEach((f) => f()); }
  private patch(id: string, p: Partial<SessionParticipant>) {
    this.participants = this.participants.map((x) => (x.identity === id ? { ...x, ...p } : x));
    this.emit();
  }
  private local() { return this.participants.find((p) => p.isLocal); }

  toggleMic = async () => { const l = this.local(); if (l) this.patch(l.identity, { micOn: !l.micOn }); };
  toggleCamera = async () => { const l = this.local(); if (l) this.patch(l.identity, { camOn: !l.camOn, video: !l.camOn ? fakeCamera('#1f7a6e') : undefined }); };
  toggleShare = async () => { const l = this.local(); if (l) this.patch(l.identity, { sharing: !l.sharing, screen: !l.sharing ? fakeCamera('#6a3fc0') : undefined }); };
  send = async (topic: 'chat' | 'reactions', payload: unknown) => { this.sent.push({ topic, payload }); };
  subscribe = (topic: DataTopic, h: (m: DataMessage) => void) => {
    const set = this.handlers.get(topic) ?? new Set();
    set.add(h);
    this.handlers.set(topic, set);
    return () => { set.delete(h); };
  };
  watch = (f: () => void) => { this.subs.add(f); return () => { this.subs.delete(f); }; };
  snapshot = () => this.snap;

  // ---- scripting API used from Playwright (window.__meeting)
  speak(identity: string, on = true) { this.patch(identity, { speaking: on }); }
  setMic(identity: string, on: boolean) { this.patch(identity, { micOn: on }); }
  setConnection(c: ConnectionLabel) { this.connection = c; this.emit(); }
  setQuality(q: QualityLabel) { this.quality = q; this.emit(); }
  join(p: MockParticipantInit) { this.participants = [...this.participants, { identity: p.identity, name: p.name, isLocal: false, speaking: false, micOn: !!p.micOn, camOn: !!p.camOn, sharing: false }]; this.emit(); }
  leave(identity: string) { this.participants = this.participants.filter((p) => p.identity !== identity); this.emit(); }
  remoteShare(identity: string, on: boolean) { this.patch(identity, { sharing: on, screen: on ? fakeCamera('#b0386f') : undefined }); }
  /** Deliver a data message as if it came from `from` over the given topic. `body` objects are JSON-encoded. */
  data(topic: DataTopic, body: unknown, from: { identity: string; name: string }) {
    const payload = enc.encode(typeof body === 'string' ? body : JSON.stringify(body));
    this.handlers.get(topic)?.forEach((h) => h({ payload, from }));
  }
}

function Live({ opts, mock }: { opts: HarnessOptions; mock: MockMeeting }) {
  const session = useSyncExternalStore(mock.watch, mock.snapshot);
  const room = useRoom(opts.roomId);
  if (!room.data) return null;
  return (
    <InMeeting
      room={room.data}
      session={session}
      selfUserId={opts.selfUserId}
      selfName={opts.selfName}
      listen={opts.listen ?? 'en'}
      onListen={() => undefined}
      captionLang={opts.captionLang ?? 'en'}
      onLeave={() => { document.body.dataset.meetingLeft = 'true'; }}
      onEnded={() => { document.body.dataset.meetingEnded = 'true'; }}
    />
  );
}

function Offline({ opts }: { opts: HarnessOptions }) {
  const room = useRoom(opts.roomId);
  const parts = useParticipants(opts.roomId);
  if (!room.data) return null;
  return <div className="p-6"><OfflineRoom room={room.data} roster={parts.data ?? []} selfUserId={opts.selfUserId} onRetry={() => undefined} /></div>;
}

const qc = new QueryClient({ defaultOptions: { queries: { staleTime: 1000, retry: false, refetchOnWindowFocus: false } } });
function Providers({ children }: { children: React.ReactNode }) {
  return <QueryClientProvider client={qc}><TooltipProvider><ToastProvider>{children}</ToastProvider></TooltipProvider></QueryClientProvider>;
}

type Win = Window & { __meeting?: MockMeeting };

export function mount(el: HTMLElement, kind: 'live' | 'offline' | 'ended', opts: HarnessOptions): MockMeeting {
  const mock = new MockMeeting(opts.participants);
  (window as Win).__meeting = mock;
  createRoot(el).render(
    <Providers>
      {kind === 'live' && <Live opts={opts} mock={mock} />}
      {kind === 'offline' && <Offline opts={opts} />}
      {kind === 'ended' && <div className="p-6"><EndedView title="This meeting has ended" body="The host ended the meeting for everyone." onBack={() => undefined} /></div>}
    </Providers>,
  );
  return mock;
}
