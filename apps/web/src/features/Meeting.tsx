import { lazy, Suspense, useEffect, useRef, useState } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { Alert, Button, Card, CardBody, PageHeader, Skeleton, StatusPill } from '@/ui';
import { ApiError } from '@/lib/api';
import { useAuth } from '@/lib/auth';
import { useJoinById, useMeetingJoin, useParticipants, useRoom } from '@/lib/queries';
import { externalMeetingHref } from '@/lib/bridge';
import type { JoinGrant } from '@/lib/schemas';
import { languageLabel } from '@/lib/languages';
import { errMsg } from './errors';
import { EndMeetingDialog } from './meeting/Dialogs';
import { EndedView, OfflineRoom } from './meeting/Offline';
import { PreJoin, roomLanguages, type PreJoinValues } from './meeting/PreJoin';
import { useMediaPreview } from './meeting/useMediaPreview';
import type { JoinMedia } from './LiveKitStage';

const LiveKitStage = lazy(() => import('./LiveKitStage'));

type Phase =
  | { kind: 'prejoin' }
  | { kind: 'lobby' }
  | { kind: 'joined'; grant: JoinGrant; media: JoinMedia; name: string; joinedListen: string }
  | { kind: 'not-configured' }
  | { kind: 'ended'; title: string; body: string; canRejoin: boolean }
  | { kind: 'error'; message: string };

const TERMINAL = ['ENDED', 'CANCELLED', 'EXPIRED', 'FAILED'];

export function MeetingPage() {
  const { roomId = '' } = useParams();
  const nav = useNavigate();
  const { userId } = useAuth();
  const room = useRoom(roomId);
  const parts = useParticipants(roomId);
  const roomJoin = useJoinById(roomId);
  const meetingJoin = useMeetingJoin(roomId);
  const [confirmEnd, setConfirmEnd] = useState(false);
  const [phase, setPhase] = useState<Phase>({ kind: 'prejoin' });
  const [edits, setEdits] = useState<Partial<PreJoinValues>>({});
  const [listen, setListen] = useState('original');
  const leaving = useRef(false);
  const lastMedia = useRef<JoinMedia>({ mic: false, cam: false });
  const self = parts.data?.find((p) => p.userId === userId);
  const r = room.data;

  const terminal = !!r && TERMINAL.includes(r.status);
  // External (Teams / Google Meet) rooms have no LiveKit stage: send them to the copilot flow instead.
  const externalHref = r ? externalMeetingHref(r) : null;
  useEffect(() => {
    if (externalHref && !terminal) nav(externalHref, { replace: true });
  }, [externalHref, terminal, nav]);
  // The room can be ended by somebody else (or the host from another tab) while we are in it.
  useEffect(() => {
    if (terminal && (phase.kind === 'joined' || phase.kind === 'lobby')) {
      leaving.current = true;
      setPhase({ kind: 'ended', title: 'This meeting has ended', body: 'The host ended the meeting for everyone.', canRejoin: false });
    }
  }, [terminal, phase.kind]);

  async function join(values: PreJoinValues, media: JoinMedia) {
    lastMedia.current = media;
    leaving.current = false;
    const name = values.name.trim();
    try {
      // Record speak/listen languages on the participant first (the meeting join takes only a name).
      // People outside the workspace cannot join by id (404/403): they still get a media token.
      try {
        await roomJoin.mutateAsync({ displayName: name || undefined, speakLanguage: values.language, listenLanguage: values.language });
      } catch (e) {
        if (!(e instanceof ApiError) || ![403, 404].includes(e.status)) throw e;
      }
      const grant = await meetingJoin.mutateAsync({ displayName: name || undefined });
      setListen(values.language);
      setPhase({ kind: 'joined', grant, media, name: name || self?.displayName || 'You', joinedListen: values.language });
    } catch (e) {
      if (e instanceof ApiError && (e.code === 'LIVEKIT_NOT_CONFIGURED' || e.status === 503)) setPhase({ kind: 'not-configured' });
      else if (e instanceof ApiError && e.code === 'IN_LOBBY') setPhase({ kind: 'lobby' });
      else setPhase({ kind: 'error', message: errMsg(e) });
    }
  }
  const pending = useRef<PreJoinValues | null>(null);

  // Lobby: once the host admits us, join automatically.
  useEffect(() => {
    if (phase.kind === 'lobby' && self?.status === 'CONNECTED' && pending.current) void join(pending.current, lastMedia.current);
    if (phase.kind === 'lobby' && self && ['KICKED', 'REJECTED'].includes(self.status)) setPhase({ kind: 'error', message: 'The host did not admit you to this meeting.' });
    // join is stable enough for this one-shot reaction to a status change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase.kind, self?.status]);

  const backToRoom = () => nav(`/rooms/${roomId}`);
  function leave() {
    leaving.current = true;
    setPhase({ kind: 'prejoin' });
    backToRoom();
  }
  function ended() {
    leaving.current = true;
    setPhase({ kind: 'prejoin' });
    backToRoom();
  }

  if (room.isLoading) return <Skeleton className="h-40" />;
  if (room.isError || !r) return <Alert tone="danger" title="Meeting not found">{errMsg(room.error)} <Link to="/rooms">Back to rooms</Link></Alert>;

  const isHost = r.effectiveHostId === userId || r.hostId === userId;
  const langs = roomLanguages(r);
  const defaultLang = self?.speakLanguage && langs.includes(self.speakLanguage) ? self.speakLanguage : r.sourceLanguage;
  const values: PreJoinValues = { name: '', language: defaultLang, micOn: false, ...edits };
  const roster = (parts.data ?? []).filter((p) => ['CONNECTED', 'WAITING'].includes(p.status));

  if (phase.kind === 'joined') {
    const captionLang = listen === 'original' ? phase.joinedListen : listen;
    return (
      <Suspense fallback={<Skeleton className="h-64" />}>
        <LiveKitStage
          grant={phase.grant}
          media={phase.media}
          listen={listen}
          onListen={(v) => {
            setListen(v);
            // Captions follow the last dubbed language while the original audio is selected.
            if (v !== 'original') setPhase((p) => (p.kind === 'joined' ? { ...p, joinedListen: v } : p));
          }}
          onLeave={leave}
          onEnded={ended}
          onDisconnected={(kind) => {
            if (leaving.current) return;
            setPhase(kind === 'ended'
              ? { kind: 'ended', title: 'This meeting has ended', body: 'The host ended the meeting for everyone.', canRejoin: false }
              : kind === 'removed'
                ? { kind: 'ended', title: 'You were removed from the meeting', body: 'The host removed you from this meeting.', canRejoin: false }
                : { kind: 'ended', title: 'You were disconnected', body: 'The connection to the meeting was lost. You can rejoin while it is live.', canRejoin: !terminal });
          }}
          room={r}
          captionLang={captionLang}
          selfUserId={userId}
          selfName={phase.name}
        />
      </Suspense>
    );
  }

  return (
    <>
      <PageHeader
        title={r.title}
        description={<span className="flex flex-wrap items-center gap-2"><StatusPill status={r.status} />{languageLabel(r.sourceLanguage)} → {r.targetLanguages.map(languageLabel).join(', ')}</span>}
        actions={(
          <>
            <Link to={`/rooms/${r.id}`} className="text-body">Room details</Link>
            {isHost && ['IN_PROGRESS', 'PAUSED'].includes(r.status) && <Button variant="ghost" size="sm" className="text-danger-text" onClick={() => setConfirmEnd(true)}>End meeting for everyone</Button>}
          </>
        )}
      />
      <EndMeetingDialog roomId={r.id} open={confirmEnd} onOpenChange={setConfirmEnd} onEnded={ended} />
      <div className="flex flex-col gap-6 px-4 py-6 sm:py-8">
        {terminal && phase.kind !== 'ended' && <Alert tone="info" title="This meeting has ended">It is closed to new participants. <Link to={`/rooms/${r.id}`}>Back to room details</Link></Alert>}
        {phase.kind === 'ended' && <EndedView title={phase.title} body={phase.body} onBack={backToRoom} onRejoin={phase.canRejoin ? () => setPhase({ kind: 'prejoin' }) : undefined} />}
        {(phase.kind === 'prejoin' || phase.kind === 'error') && !terminal && (
          <PreJoinStage
            room={r}
            values={values}
            onChange={(v) => setEdits((e) => ({ ...e, ...v }))}
            joining={roomJoin.isPending || meetingJoin.isPending}
            notice={phase.kind === 'error' ? { tone: 'danger', title: 'Could not join the meeting', body: phase.message } : null}
            onJoin={(media) => { pending.current = values; void join(values, media); }}
          />
        )}
        {phase.kind === 'lobby' && (
          <Card className="mx-auto w-full max-w-md" data-testid="meeting-lobby">
            <CardBody className="flex flex-col gap-3">
              <h2 className="text-title font-semibold">Waiting for the host</h2>
              <p className="text-body text-ink-subtle">The host has to admit you before you can enter. This page joins automatically once you are let in.</p>
              <Button onClick={() => { setPhase({ kind: 'prejoin' }); }}>Cancel</Button>
            </CardBody>
          </Card>
        )}
        {phase.kind === 'not-configured' && <OfflineRoom room={r} roster={roster} selfUserId={userId} onRetry={() => setPhase({ kind: 'prejoin' })} />}
      </div>
    </>
  );
}

/** Owns the camera/microphone preview so it is released as soon as the card unmounts (join, leave). */
function PreJoinStage({ room, values, onChange, joining, notice, onJoin }: {
  room: Parameters<typeof PreJoin>[0]['room'];
  values: PreJoinValues;
  onChange(v: Partial<PreJoinValues>): void;
  joining: boolean;
  notice: Parameters<typeof PreJoin>[0]['notice'];
  onJoin(media: JoinMedia): void;
}) {
  const preview = useMediaPreview();
  const { stop, retry } = preview;
  useEffect(() => { if (notice) retry(); }, [notice, retry]);
  return (
    <PreJoin
      room={room}
      values={values}
      onChange={onChange}
      preview={preview}
      joining={joining}
      notice={notice}
      onJoin={() => {
        const mic = values.micOn && preview.hasAudio;
        const cam = preview.camOn && preview.hasVideo;
        const micId = preview.micId || preview.stream?.getAudioTracks()[0]?.getSettings().deviceId;
        const camId = preview.camId || preview.stream?.getVideoTracks()[0]?.getSettings().deviceId;
        stop();
        onJoin({ mic, cam, micId, camId });
      }}
    />
  );
}
