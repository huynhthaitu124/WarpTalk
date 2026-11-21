import { lazy, Suspense, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { PlugZap, Video, PhoneOff } from 'lucide-react';
import { Alert, Button, Card, CardBody, Dialog, DialogClose, DialogContent, Field, Input, PageHeader, Skeleton, StatusPill, useToast } from '@/ui';
import { ApiError } from '@/lib/api';
import { useAuth } from '@/lib/auth';
import { useEndMeeting, useMeetingJoin, useParticipants, useRoom } from '@/lib/queries';
import type { JoinGrant } from '@/lib/schemas';
import { languageLabel } from '@/lib/languages';
import { CaptionsPanel, MeetingControls, ParticipantGrid, StatusStrip } from './MeetingParts';
import { errMsg } from './errors';

const LiveKitStage = lazy(() => import('./LiveKitStage'));

type Phase =
  | { kind: 'prejoin' }
  | { kind: 'joined'; grant: JoinGrant }
  | { kind: 'not-configured' }
  | { kind: 'error'; message: string };

export function MeetingPage() {
  const { roomId = '' } = useParams();
  const nav = useNavigate();
  const { userId } = useAuth();
  const room = useRoom(roomId);
  const parts = useParticipants(roomId);
  const joinM = useMeetingJoin(roomId);
  const endM = useEndMeeting(roomId);
  const toast = useToast();
  const [confirmEnd, setConfirmEnd] = useState(false);
  const [phase, setPhase] = useState<Phase>({ kind: 'prejoin' });
  const [name, setName] = useState('');
  const self = parts.data?.find((p) => p.userId === userId);
  const [listenOverride, setListen] = useState<string | null>(null);
  const listen = listenOverride ?? self?.listenLanguage ?? 'original';

  async function join() {
    try {
      const grant = await joinM.mutateAsync(name.trim() || undefined);
      setPhase({ kind: 'joined', grant });
    } catch (e) {
      if (e instanceof ApiError && (e.code === 'LIVEKIT_NOT_CONFIGURED' || e.status === 503)) setPhase({ kind: 'not-configured' });
      else setPhase({ kind: 'error', message: errMsg(e) });
    }
  }
  const leave = () => { setPhase({ kind: 'prejoin' }); nav(`/rooms/${roomId}`); };
  async function endMeeting() {
    try {
      await endM.mutateAsync();
      setConfirmEnd(false);
      setPhase({ kind: 'prejoin' });
      toast({ tone: 'success', title: 'Meeting ended' });
      nav(`/rooms/${roomId}`);
    } catch { /* shown inside the dialog */ }
  }

  if (room.isLoading) return <Skeleton className="h-40" />;
  if (room.isError || !room.data) return <Alert tone="danger" title="Meeting not found">{errMsg(room.error)} <Link to="/rooms">Back to rooms</Link></Alert>;
  const r = room.data;
  const roster = (parts.data ?? []).filter((p) => ['CONNECTED', 'WAITING'].includes(p.status));
  const isHost = r.effectiveHostId === userId || r.hostId === userId;
  const terminal = ['ENDED', 'CANCELLED', 'EXPIRED', 'FAILED'].includes(r.status);
  const captionLang = listen === 'original' ? (self?.listenLanguage ?? 'original') : listen;

  return (
    <>
      <PageHeader
        title={r.title}
        description={<span className="flex items-center gap-2"><StatusPill status={r.status} />{languageLabel(r.sourceLanguage)} → {r.targetLanguages.map(languageLabel).join(', ')}</span>}
        actions={(
          <>
            <Link to={`/rooms/${r.id}`} className="text-body">Room details</Link>
            {isHost && !terminal && <Button variant="danger" size="sm" onClick={() => setConfirmEnd(true)}><PhoneOff className="size-4" aria-hidden />End meeting</Button>}
          </>
        )}
      />
      <Dialog open={confirmEnd} onOpenChange={(v) => { if (!endM.isPending) { setConfirmEnd(v); if (!v) endM.reset(); } }}>
        <DialogContent
          title="End meeting for everyone?"
          description="Everyone is disconnected and the room is closed. This cannot be undone."
          footer={(
            <>
              <DialogClose asChild><Button>Cancel</Button></DialogClose>
              <Button variant="danger" loading={endM.isPending} onClick={() => void endMeeting()}>End meeting</Button>
            </>
          )}
        >
          {endM.isError && <Alert tone="danger" title="Could not end the meeting">{errMsg(endM.error)}</Alert>}
        </DialogContent>
      </Dialog>
      {phase.kind !== 'joined' && <div className="mb-6"><StatusStrip connection="idle" quality="unknown" room={r} /></div>}
      {phase.kind === 'joined' ? (
        <Suspense fallback={<Skeleton className="h-64" />}>
          <LiveKitStage grant={phase.grant} listen={listen} onListen={setListen} onLeave={leave} room={r} captionLang={captionLang} />
        </Suspense>
      ) : (
        <div className="flex flex-col gap-6">
          {terminal && <Alert tone="info" title="This meeting has ended">It is closed to new participants. <Link to={`/rooms/${r.id}`}>Back to room details</Link></Alert>}
          {phase.kind === 'prejoin' && !terminal && (
            <Card className="max-w-md" data-testid="meeting-prejoin">
              <CardBody className="flex flex-col gap-4">
                <div className="flex items-center gap-3"><span className="flex size-10 items-center justify-center rounded-lg bg-surface-2 text-ink-subtle"><Video className="size-5" aria-hidden /></span><div><h2 className="text-title font-semibold">Ready to join?</h2><p className="text-body-sm text-ink-subtle">You will connect with your microphone off.</p></div></div>
                <Field label="Display name (optional)"><Input value={name} onChange={(e) => setName(e.target.value)} maxLength={100} /></Field>
                <Button variant="primary" size="lg" loading={joinM.isPending} onClick={() => void join()}>Join meeting</Button>
              </CardBody>
            </Card>
          )}
          {phase.kind === 'not-configured' && (
            <Alert tone="warning" title="LiveKit is not configured">
              <span className="flex items-start gap-2"><PlugZap className="mt-0.5 size-4 shrink-0" aria-hidden /><span data-testid="livekit-not-configured">The server has no LiveKit credentials (LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET), so live audio is unavailable. You can still see who is in the room; captions and dubbed audio need a media server.</span></span>
              <Button size="sm" className="mt-3" onClick={() => setPhase({ kind: 'prejoin' })}>Try again</Button>
            </Alert>
          )}
          {phase.kind === 'error' && (
            <Alert tone="danger" title="Could not join the meeting">
              {phase.message}
              <Button size="sm" className="mt-3" onClick={() => setPhase({ kind: 'prejoin' })}>Back</Button>
            </Alert>
          )}
          <div className="grid gap-6 lg:grid-cols-[1fr_360px]">
            <ParticipantGrid tiles={roster.map((p) => ({ id: p.id, name: p.displayName, note: p.status === 'WAITING' ? 'In lobby' : undefined, isLocal: p.userId === userId }))} empty="Nobody is in the room yet." />
            <CaptionsPanel lines={[]} translationLang={captionLang} placeholder="Captions are unavailable until you are connected to the meeting." />
          </div>
          <MeetingControls micOn={false} onToggleMic={() => undefined} micDisabled micHint="Join the meeting to use your microphone" listen={listen} onListen={setListen} onLeave={leave} />
        </div>
      )}
    </>
  );
}
