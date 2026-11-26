import { useEffect, useRef, useState, type FormEvent } from 'react';
import { Link, useParams } from 'react-router-dom';
import { CaretDown, CheckCircle, Copy, Hourglass, Users, UsersThree, DotsThree, Pause, Play, Prohibit, Stop, Translate, UserCheck, UserMinus, VideoCamera } from '@phosphor-icons/react';
import {
  Alert, Avatar, Button, Card, CardBody, CardHeader, Dialog, DialogContent, Dropdown, DropdownContent, DropdownItem, DropdownSeparator, DropdownTrigger,
  EmptyState, Field, GroupHeader, Input, PageBody, PageHeader, Select, Skeleton, useToast,
} from '@v2/ui';
import { useAuth, useProfile } from '@v2/lib/auth';
import { canAdmin, useWorkspace } from '@v2/lib/workspace';
import {
  useJoinById, useLeaveRoom, useParticipantAction, useParticipants, useRoom, useRoomAction, type RoomAction,
} from '@v2/lib/queries';
import { externalMeetingHref } from '@v2/lib/bridge';
import { ApiError } from '@v2/lib/api';
import { languageOptions } from '@v2/lib/languageOptions';
import type { Participant, Room } from '@v2/lib/schemas';
import { TERMINAL_STATUSES, useCopy, useHostName } from './common';
import { errMsg } from './errors';
import { RoomRecords } from './RoomRecords';
import { EditableDescription, EditableTitle, PeopleProp, PropGroup, RoomProperties } from './RoomProperties';

const TERMINAL = TERMINAL_STATUSES;

export function JoinRoomForm({ roomId, defaultName }: { roomId: string; defaultName?: string }) {
  const join = useJoinById(roomId);
  const [name, setName] = useState(defaultName ?? '');
  const [lang, setLang] = useState('en');
  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await join.mutateAsync({ displayName: name.trim() || undefined, speakLanguage: lang, listenLanguage: lang });
    } catch { /* inline */ }
  }
  const opts = languageOptions();
  return (
    <Card className="mx-auto max-w-md">
      <CardHeader title="Join this meeting" description="Pick your language: you speak and hear it, everyone else hears you in theirs." />
      <CardBody>
        <form onSubmit={submit} className="flex flex-col gap-4">
          <Field label="Display name (optional)"><Input value={name} onChange={(e) => setName(e.target.value)} maxLength={100} /></Field>
          <Field label="My language"><Select value={lang} onValueChange={setLang} options={opts} /></Field>
          {join.isError && <Alert tone="danger">{errMsg(join.error)}</Alert>}
          <Button type="submit" variant="primary" loading={join.isPending}>Join meeting</Button>
        </form>
      </CardBody>
    </Card>
  );
}

function OpenMeetingLink({ room, primary }: { room: Room; primary: boolean }) {
  return (
    <Link
      to={externalMeetingHref(room) ?? `/meetings/${room.id}`}
      className={primary
        ? 'inline-flex h-7 items-center gap-2 rounded-pill bg-ink-btn pl-2.5 pr-3 font-medium text-on-ink-btn shadow-btn no-underline hover:no-underline hover:opacity-90 focus-ring'
        : 'inline-flex h-7 items-center gap-2 rounded-pill border border-hairline/60 bg-surface-2 pl-2.5 pr-3 font-medium text-ink shadow-btn no-underline hover:bg-surface-3 hover:no-underline focus-ring'}
    >
      <VideoCamera size={16} weight="duotone" aria-hidden />Open meeting
    </Link>
  );
}

/**
 * Lifecycle controls. One primary action per state (Start / Resume / Open meeting); the rest of
 * the room's actions, including the destructive ones, live in the "More" menu (LY-07, DS-06).
 */
function HostControls({ room, canOpen }: { room: Room; canOpen: boolean }) {
  const act = useRoomAction(room.id);
  const toast = useToast();
  const [error, setError] = useState('');
  const [confirm, setConfirm] = useState<null | 'end' | 'cancel'>(null);
  const run = async (a: RoomAction, label: string) => {
    setError('');
    try { await act.mutateAsync(a); toast({ tone: 'success', title: label }); setConfirm(null); } catch (e) { setError(errMsg(e)); setConfirm(null); }
  };
  const s = room.status;
  const pending = act.isPending;
  const startable = ['SCHEDULED', 'WAITING', 'OPEN'].includes(s);
  const live = s === 'IN_PROGRESS';
  const canEnd = ['IN_PROGRESS', 'PAUSED', 'WAITING', 'OPEN'].includes(s);
  const openLink = canOpen && <OpenMeetingLink room={room} primary={live} />;
  return (
    <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Room controls">
      {openLink}
      {startable && <Button variant="primary" disabled={pending} onClick={() => run('start', 'Meeting started')}><Play size={16} weight="fill" aria-hidden />Start</Button>}
      {s === 'PAUSED' && <Button variant="primary" disabled={pending} onClick={() => run('resume', 'Meeting resumed')}><Play size={16} weight="fill" aria-hidden />Resume</Button>}
      {live && <Button disabled={pending} onClick={() => run('pause', 'Meeting paused')}><Pause size={16} weight="fill" aria-hidden />Pause</Button>}
      {(live || startable || canEnd) && (
        <Dropdown>
          <DropdownTrigger asChild>
            <Button aria-label="More actions" disabled={pending}><DotsThree size={16} weight="bold" aria-hidden />More<CaretDown size={12} aria-hidden /></Button>
          </DropdownTrigger>
          <DropdownContent align="end" className="w-52">
            {live && <DropdownItem icon={<Translate size={16} />} onSelect={() => void run('translation/stop', 'Translation stopped')}>Stop translation</DropdownItem>}
            {startable && <DropdownItem icon={<Prohibit size={16} />} onSelect={() => setConfirm('cancel')}>Cancel meeting</DropdownItem>}
            {canEnd && <><DropdownSeparator /><DropdownItem danger icon={<Stop size={16} weight="fill" />} onSelect={() => setConfirm('end')}>End meeting</DropdownItem></>}
          </DropdownContent>
        </Dropdown>
      )}
      {error && <span role="alert" className="basis-full text-body-sm text-danger-text">{error}</span>}
      <Dialog open={confirm !== null} onOpenChange={(v) => { if (!v) setConfirm(null); }}>
        <DialogContent
          title={confirm === 'end' ? 'End this meeting?' : 'Cancel this meeting?'}
          description={confirm === 'end' ? 'It ends for everyone and cannot be restarted.' : 'Nobody will be able to join it.'}
          footer={
            <>
              <Button onClick={() => setConfirm(null)}>{confirm === 'end' ? 'Keep meeting' : 'Back'}</Button>
              <Button variant="danger" loading={pending} onClick={() => void run(confirm === 'end' ? 'end' : 'cancel', confirm === 'end' ? 'Meeting ended' : 'Meeting cancelled')}>
                {confirm === 'end' ? 'End meeting' : 'Cancel meeting'}
              </Button>
            </>
          }
        />
      </Dialog>
    </div>
  );
}

/** Hosts admit or deny people waiting in the lobby. Renders nothing when the lobby is empty. */
function LobbySection({ roomId, list, canModerate }: { roomId: string; list: Participant[]; canModerate: boolean }) {
  const pa = useParticipantAction(roomId);
  const toast = useToast();
  const waiting = list.filter((p) => p.status === 'WAITING');
  const seen = useRef<number | null>(null);

  // In-app heads-up when somebody lands in the lobby (the list polls every 3s).
  useEffect(() => {
    if (!canModerate) return;
    if (seen.current !== null && waiting.length > seen.current) {
      const newest = waiting[waiting.length - 1];
      toast({ tone: 'info', title: `${newest?.displayName ?? 'Someone'} is waiting in the lobby` });
    }
    seen.current = waiting.length;
  }, [waiting, canModerate, toast]);

  async function run(p: Participant, action: 'admit' | 'kick', verb: 'admit' | 'decline') {
    try {
      await pa.mutateAsync({ pid: p.id, action });
      toast({ tone: 'success', title: verb === 'admit' ? `${p.displayName} admitted` : `Request from ${p.displayName} declined` });
    } catch (e) { toast({ tone: 'danger', title: 'Action failed', description: errMsg(e) }); }
  }
  async function admitAll() {
    for (const p of waiting) {
      try { await pa.mutateAsync({ pid: p.id, action: 'admit' }); } catch (e) { toast({ tone: 'danger', title: `Could not admit ${p.displayName}`, description: errMsg(e) }); return; }
    }
    toast({ tone: 'success', title: `${waiting.length} admitted` });
  }
  if (!canModerate || waiting.length === 0) return null;
  return (
        <section data-testid="lobby" className="min-w-0 overflow-hidden rounded-[10px] border border-hairline bg-surface-1 shadow-capsule">
          <GroupHeader className="pr-2">
            <h2 className="font-medium text-ink">{`Lobby (${waiting.length})`}</h2><span className="truncate max-sm:hidden">· These people are waiting for you to let them in.</span>
            {waiting.length > 1 && <Button size="sm" variant="primary" className="ml-auto" disabled={pa.isPending} onClick={() => void admitAll()}>Admit all</Button>}
          </GroupHeader>
          <ul aria-label="Lobby">
            {waiting.map((p) => (
              <li key={p.id} className="flex flex-wrap items-center gap-3 border-b border-row px-4 py-2 last:border-0">
                <Avatar name={p.displayName} />
                <span className="min-w-0 flex-1 basis-32 truncate text-body font-medium" title={p.displayName}>{p.displayName}</span>
                <span className="flex gap-2">
                  <Button size="sm" variant={waiting.length > 1 ? 'secondary' : 'primary'} onClick={() => run(p, 'admit', 'admit')} disabled={pa.isPending} aria-label={`Admit ${p.displayName}`}><UserCheck size={16} aria-hidden />Admit</Button>
                  <Button size="sm" onClick={() => run(p, 'kick', 'decline')} disabled={pa.isPending} aria-label={`Deny ${p.displayName}`}><UserMinus size={16} aria-hidden />Deny</Button>
                </span>
              </li>
            ))}
          </ul>
        </section>
  );
}

export function RoomDetailPage() {
  const { roomId = '' } = useParams();
  const { userId } = useAuth();
  const { workspaces } = useWorkspace();
  const roomQ = useRoom(roomId);
  const partsQ = useParticipants(roomId, roomQ.isSuccess);
  const leave = useLeaveRoom(roomId);
  const profile = useProfile();
  const copy = useCopy();
  const hostName = useHostName(roomQ.data);

  if (roomQ.isLoading) return <PageBody className="flex flex-col gap-3"><Skeleton className="h-10 w-64" /><Skeleton className="h-40" /></PageBody>;
  if (roomQ.isError) {
    const notFound = roomQ.error instanceof ApiError && roomQ.error.status === 404;
    return notFound ? (
      <>
        <PageHeader size="detail" title="Meeting not found" description="This meeting does not exist, or you have not joined it yet." />
        <PageBody><EmptyState title="Have a room code?" description="People outside the workspace can only enter a meeting with its 12-character code." action={<Link to="/join" className="text-body">Join by code</Link>} /></PageBody>
      </>
    ) : <PageBody><Alert tone="danger" title="Could not load meeting">{errMsg(roomQ.error)}</Alert></PageBody>;
  }
  const room = roomQ.data;
  if (!room) return null;
  const ws = workspaces.find((w) => w.id === room.workspaceId) ?? null;
  const isHost = room.effectiveHostId === userId || room.hostId === userId;
  const canModerate = isHost || canAdmin(ws);
  const list = partsQ.data ?? [];
  const self = list.find((p) => p.userId === userId);
  const terminal = TERMINAL.includes(room.status);
  const inLobby = self?.status === 'WAITING';
  const canOpen = !terminal && !inLobby && !!(self || canModerate);

  const connected = list.filter((p) => p.status === 'CONNECTED');
  const waiting = list.filter((p) => p.status === 'WAITING');
  const attended = list.filter((p) => ['DISCONNECTED', 'LEFT'].includes(p.status));
  const editable = canModerate && !terminal;

  const controls = !inLobby && (canModerate && !terminal ? <HostControls room={room} canOpen={canOpen} /> : canOpen && <OpenMeetingLink room={room} primary />);

  return (
    <div className="flex min-h-full flex-col">
      {/* Toolbar row, top right (as in Linear's issue view). A person still in the lobby sees no controls. */}
      <div className="flex min-h-14 items-center justify-end px-6 pt-3 max-sm:px-4">{controls}</div>
      <div className="mx-auto flex w-full max-w-[1280px] flex-col gap-10 px-6 pb-10 pt-4 max-sm:gap-6 max-sm:px-4 lg:flex-row lg:items-start lg:gap-16 lg:pl-20">
        <div className="flex min-w-0 max-w-[760px] flex-1 flex-col gap-6">
          <header className="flex flex-col gap-5">
            <EditableTitle room={room} editable={editable} />
            <EditableDescription room={room} editable={editable} />
          </header>
          <div className="flex flex-col gap-4 empty:hidden">
            {!self && !terminal && partsQ.isSuccess && <JoinRoomForm roomId={room.id} defaultName={profile.data?.fullName} />}
            {inLobby && <Alert tone="warning" title="Waiting for the host">The host needs to admit you before you can enter the meeting. This page updates automatically.</Alert>}
            {self?.status === 'KICKED' && <Alert tone="danger" title="You were removed from this meeting" />}
            {self?.status === 'CONNECTED' && !isHost && !terminal && (
              <div className="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-hairline bg-surface-1 px-4 py-3 shadow-card">
                <span className="text-body">You are in this meeting as <strong>{self.displayName}</strong>.</span>
                <Button size="sm" onClick={() => leave.mutate()} loading={leave.isPending}>Leave meeting</Button>
              </div>
            )}
            {terminal && <Alert tone="info">This meeting is {room.status.toLowerCase()}.</Alert>}
            {!inLobby && <LobbySection roomId={room.id} list={list} canModerate={canModerate && !terminal} />}
          </div>
          {!inLobby && (self || canModerate) && (
            <div className="mt-6"><RoomRecords roomId={room.id} lang={self?.listenLanguage} live={room.status === 'IN_PROGRESS'} ended={terminal} /></div>
          )}
        </div>
        <aside aria-label="Properties" className="flex shrink-0 flex-col gap-9 lg:sticky lg:top-6 lg:w-[280px]">
          <RoomProperties room={room} hostName={hostName} editable={editable} />
          {!inLobby && (
            <PropGroup title="Room code">
              <li>
                <button
                  type="button"
                  aria-label={`Copy room code ${room.roomCode}`}
                  onClick={() => void copy(room.roomCode, 'Room code')}
                  className="-ml-1.5 inline-flex h-8 items-center gap-2.5 rounded-md px-1.5 text-body text-ink hover:bg-surface-2 focus-ring"
                >
                  <Copy size={16} aria-hidden className="text-ink-muted" /><code data-testid="room-code" className="font-mono">{room.roomCode}</code>
                </button>
              </li>
            </PropGroup>
          )}
          {!inLobby && !partsQ.isError && (
            <PropGroup title="People">
              <PeopleProp label="In room" icon={<UsersThree size={16} />} text={`${connected.length} in room`} people={connected} />
              <PeopleProp label="Waiting to be admitted" icon={<Hourglass size={16} />} text={`${waiting.length} waiting`} people={waiting} />
              <PeopleProp label="Attended" icon={<CheckCircle size={16} />} text={`${attended.length} attended`} people={attended} />
              <PeopleProp label="Participants" icon={<Users size={16} />} text={`${list.length} total`} people={list} />
            </PropGroup>
          )}
        </aside>
      </div>
    </div>
  );
}
