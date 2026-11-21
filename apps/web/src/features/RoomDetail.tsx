import { useEffect, useMemo, useRef, useState, type FormEvent } from 'react';
import { Link, useParams } from 'react-router-dom';
import { ArrowsClockwise, CaretDown, Copy, DotsThree, Pause, Play, Prohibit, Stop, Translate, UserCheck, UserMinus, VideoCamera } from '@phosphor-icons/react';
import {
  Alert, Avatar, Badge, Button, Card, CardBody, CardHeader, Dialog, DialogContent, Dropdown, DropdownContent, DropdownItem, DropdownSeparator, DropdownTrigger,
  EmptyState, Field, Input, PageHeader, Select, Skeleton, StatusPill, Table, TBody, TD, TH, THead, TR, useToast,
} from '@/ui';
import { useAuth, useProfile } from '@/lib/auth';
import { canAdmin, useWorkspace } from '@/lib/workspace';
import {
  useGenerateRoutes, useJoinById, useLeaveRoom, useParticipantAction, useParticipants, useRoom, useRoomAction, useRoutes, type RoomAction,
} from '@/lib/queries';
import { ApiError } from '@/lib/api';
import { LANGUAGES, languageLabel } from '@/lib/languages';
import type { Participant, Room } from '@/lib/schemas';
import { TERMINAL_STATUSES, useCopy } from './common';
import { errMsg } from './errors';

const TERMINAL = TERMINAL_STATUSES;

export function JoinRoomForm({ roomId, defaultName }: { roomId: string; defaultName?: string }) {
  const join = useJoinById(roomId);
  const [name, setName] = useState(defaultName ?? '');
  const [speak, setSpeak] = useState('en');
  const [listen, setListen] = useState('vi');
  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      await join.mutateAsync({ displayName: name.trim() || undefined, speakLanguage: speak, listenLanguage: listen });
    } catch { /* inline */ }
  }
  const opts = LANGUAGES.map((l) => ({ value: l.code, label: l.label }));
  return (
    <Card className="mx-auto max-w-md">
      <CardHeader title="Join this meeting" description="Choose how you speak and what you want to hear." />
      <CardBody>
        <form onSubmit={submit} className="flex flex-col gap-4">
          <Field label="Display name (optional)"><Input value={name} onChange={(e) => setName(e.target.value)} maxLength={100} /></Field>
          <Field label="I speak"><Select value={speak} onValueChange={setSpeak} options={opts} /></Field>
          <Field label="I want to listen in"><Select value={listen} onValueChange={setListen} options={opts} /></Field>
          {join.isError && <Alert tone="danger">{errMsg(join.error)}</Alert>}
          <Button type="submit" variant="primary" loading={join.isPending}>Join meeting</Button>
        </form>
      </CardBody>
    </Card>
  );
}

function OpenMeetingLink({ roomId, primary }: { roomId: string; primary: boolean }) {
  return (
    <Link
      to={`/meetings/${roomId}`}
      className={primary
        ? 'inline-flex h-8 items-center gap-2 rounded-md bg-primary px-3.5 font-medium text-on-primary no-underline hover:bg-primary-hover hover:no-underline focus-ring'
        : 'inline-flex h-8 items-center gap-2 rounded-md border border-control bg-surface-1 px-3.5 font-medium text-ink no-underline hover:bg-surface-2 hover:no-underline focus-ring'}
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
  const openLink = canOpen && <OpenMeetingLink roomId={room.id} primary={live} />;
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

const NOT_ACTIVE = ['KICKED', 'LEFT', 'REJECTED'];

function ParticipantsCard({ roomId, list, canModerate, selfId }: { roomId: string; list: Participant[]; canModerate: boolean; selfId: string | null }) {
  const pa = useParticipantAction(roomId);
  const toast = useToast();
  const waiting = list.filter((p) => p.status === 'WAITING');
  const others = list.filter((p) => p.status !== 'WAITING');
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

  async function run(p: Participant, action: 'admit' | 'kick', verb: 'admit' | 'decline' | 'remove') {
    try {
      await pa.mutateAsync({ pid: p.id, action });
      toast({ tone: 'success', title: verb === 'admit' ? `${p.displayName} admitted` : verb === 'decline' ? `Request from ${p.displayName} declined` : `${p.displayName} removed` });
    } catch (e) { toast({ tone: 'danger', title: 'Action failed', description: errMsg(e) }); }
  }
  async function admitAll() {
    for (const p of waiting) {
      try { await pa.mutateAsync({ pid: p.id, action: 'admit' }); } catch (e) { toast({ tone: 'danger', title: `Could not admit ${p.displayName}`, description: errMsg(e) }); return; }
    }
    toast({ tone: 'success', title: `${waiting.length} admitted` });
  }
  return (
    <div className="flex min-w-0 flex-col gap-6">
      {canModerate && waiting.length > 0 && (
        <Card data-testid="lobby" className="min-w-0">
          <CardHeader
            title={`Lobby (${waiting.length})`}
            description="These people are waiting for you to let them in."
            actions={waiting.length > 1 ? <Button size="sm" variant="primary" disabled={pa.isPending} onClick={() => void admitAll()}>Admit all</Button> : undefined}
          />
          <ul className="divide-y divide-hairline" aria-label="Lobby">
            {waiting.map((p) => (
              <li key={p.id} className="flex flex-wrap items-center gap-3 px-5 py-3">
                <Avatar name={p.displayName} />
                <span className="min-w-0 flex-1 basis-32 truncate text-body font-medium" title={p.displayName}>{p.displayName}</span>
                <span className="flex gap-2">
                  <Button size="sm" variant={waiting.length > 1 ? 'secondary' : 'primary'} onClick={() => run(p, 'admit', 'admit')} disabled={pa.isPending} aria-label={`Admit ${p.displayName}`}><UserCheck size={16} aria-hidden />Admit</Button>
                  <Button size="sm" onClick={() => run(p, 'kick', 'decline')} disabled={pa.isPending} aria-label={`Deny ${p.displayName}`}><UserMinus size={16} aria-hidden />Deny</Button>
                </span>
              </li>
            ))}
          </ul>
        </Card>
      )}
      <Card className="min-w-0">
        <CardHeader title={`Participants (${others.length})`} />
        {others.length === 0 ? (
          <CardBody><p className="text-body text-ink-subtle">Nobody has joined yet.</p></CardBody>
        ) : (
          <>
            <div className="max-sm:hidden">
              <Table bare aria-label="Participants">
                <THead><TR><TH>Name</TH><TH>Role</TH><TH>Speaks</TH><TH>Listens</TH><TH>Status</TH>{canModerate && <TH><span className="sr-only">Actions</span></TH>}</TR></THead>
                <TBody>
                  {others.map((p) => (
                    <TR key={p.id}>
                      <TD><span className="flex items-center gap-2"><Avatar name={p.displayName} size="sm" /><span className="max-w-48 truncate" title={p.displayName}>{p.displayName}</span>{p.userId === selfId && <Badge>You</Badge>}</span></TD>
                      <TD><StatusPill status={p.role} /></TD>
                      <TD>{languageLabel(p.speakLanguage)}</TD>
                      <TD>{languageLabel(p.listenLanguage)}</TD>
                      <TD><StatusPill status={p.status} /></TD>
                      {canModerate && (
                        <TD className="text-right">
                          {p.role !== 'HOST' && !NOT_ACTIVE.includes(p.status) && (
                            <Button size="sm" variant="ghost" onClick={() => run(p, 'kick', 'remove')} aria-label={`Kick ${p.displayName}`} disabled={pa.isPending}>Kick</Button>
                          )}
                        </TD>
                      )}
                    </TR>
                  ))}
                </TBody>
              </Table>
            </div>
            <ul className="divide-y divide-hairline sm:hidden" aria-label="Participants (list)">
              {others.map((p) => (
                <li key={p.id} className="flex flex-col gap-2 px-4 py-3">
                  <div className="flex items-center gap-2">
                    <Avatar name={p.displayName} size="sm" />
                    <span className="min-w-0 flex-1 truncate text-body font-medium" title={p.displayName}>{p.displayName}</span>
                    {p.userId === selfId && <Badge>You</Badge>}
                  </div>
                  <div className="flex flex-wrap items-center gap-2 text-caption text-ink-subtle">
                    <StatusPill status={p.role} /><StatusPill status={p.status} />
                    <span>{languageLabel(p.speakLanguage)} → {languageLabel(p.listenLanguage)}</span>
                    {canModerate && p.role !== 'HOST' && !NOT_ACTIVE.includes(p.status) && (
                      <Button size="sm" variant="ghost" className="ml-auto" onClick={() => run(p, 'kick', 'remove')} aria-label={`Kick ${p.displayName}`} disabled={pa.isPending}>Kick</Button>
                    )}
                  </div>
                </li>
              ))}
            </ul>
          </>
        )}
      </Card>
    </div>
  );
}

/** Route status arrives as a string or a tagged object: show a word, never JSON. */
function routeStatusLabel(status: unknown): string {
  const raw = typeof status === 'string' ? status : status && typeof status === 'object' ? Object.keys(status)[0] : undefined;
  if (!raw) return 'Unknown';
  const t = raw.replace(/([a-z])([A-Z])/g, '$1 $2').replace(/_/g, ' ').toLowerCase();
  return t[0]!.toUpperCase() + t.slice(1);
}

function RoutesCard({ roomId, canGenerate, participants }: { roomId: string; canGenerate: boolean; participants: Participant[] }) {
  const [open, setOpen] = useState(false);
  const routes = useRoutes(roomId, open);
  const gen = useGenerateRoutes(roomId);
  const names = useMemo(() => new Map(participants.map((p) => [p.id, p.displayName])), [participants]);
  const items = routes.data?.routes ?? [];
  return (
    <Card className="min-w-0">
      <CardHeader
        title="Advanced: audio routes"
        description="Who hears whom, in which language. Operator view."
        actions={<Button size="sm" aria-expanded={open} aria-controls="routes-body" onClick={() => setOpen((v) => !v)}>{open ? 'Hide' : 'Show'}</Button>}
      />
      {open && (
        <div id="routes-body">
          {canGenerate && <div className="flex justify-end px-5 pt-4"><Button size="sm" loading={gen.isPending} onClick={() => gen.mutate()}><ArrowsClockwise size={16} aria-hidden />Generate routes</Button></div>}
          {routes.isLoading ? (
            <CardBody><Skeleton className="h-10" /></CardBody>
          ) : routes.isError ? (
            <CardBody><Alert tone="warning">Audio routes are not available yet: {errMsg(routes.error)}</Alert></CardBody>
          ) : items.length === 0 ? (
            <CardBody><p className="text-body text-ink-subtle">No routes yet. Routes appear once the meeting is live and participants have joined.</p></CardBody>
          ) : (
            <Table bare aria-label="Audio routes">
              <THead><TR><TH>Speaker</TH><TH>Listener</TH><TH>Language</TH><TH>Voice clone</TH><TH>Status</TH></TR></THead>
              <TBody>
                {items.map((r) => (
                  <TR key={r.routeId}>
                    <TD>{names.get(r.sourceParticipantId) ?? 'Participant'}</TD>
                    <TD>{names.get(r.targetParticipantId) ?? 'Participant'}</TD>
                    <TD className="whitespace-nowrap">{languageLabel(r.sourceLanguage)} → {languageLabel(r.targetLanguage)}</TD>
                    <TD>{r.voiceCloneEnabled ? 'On' : 'Off'}</TD>
                    <TD><Badge>{routeStatusLabel(r.status)}</Badge></TD>
                  </TR>
                ))}
              </TBody>
            </Table>
          )}
        </div>
      )}
    </Card>
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

  if (roomQ.isLoading) return <div className="flex flex-col gap-3"><Skeleton className="h-10 w-64" /><Skeleton className="h-40" /></div>;
  if (roomQ.isError) {
    const notFound = roomQ.error instanceof ApiError && roomQ.error.status === 404;
    return notFound ? (
      <div className="flex flex-col gap-4">
        <PageHeader title="Meeting not found" description="This meeting does not exist, or you have not joined it yet." />
        <EmptyState title="Have a room code?" description="People outside the workspace can only enter a meeting with its 12-character code." action={<Link to="/join" className="text-body">Join by code</Link>} />
      </div>
    ) : <Alert tone="danger" title="Could not load meeting">{errMsg(roomQ.error)}</Alert>;
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

  return (
    <>
      <PageHeader
        title={room.title}
        description={
          <span className="flex flex-wrap items-center gap-2">
            <StatusPill status={room.status} />
            <span>{languageLabel(room.sourceLanguage)} → {room.targetLanguages.map(languageLabel).join(', ')}</span>
            {room.requiresApproval && <Badge>Lobby on</Badge>}
          </span>
        }
        actions={
          // A person still waiting to be admitted sees neither the code nor any controls.
          !inLobby && (
            <>
              <button
                type="button"
                aria-label={`Copy room code ${room.roomCode}`}
                onClick={() => void copy(room.roomCode, 'Room code')}
                className="inline-flex h-8 items-center gap-2 rounded-md border border-control bg-surface-1 px-3 text-body font-medium text-ink hover:bg-surface-2 focus-ring"
              >
                <Copy size={16} aria-hidden className="text-ink-subtle" /><code data-testid="room-code">{room.roomCode}</code>
              </button>
              {canModerate && !terminal ? <HostControls room={room} canOpen={canOpen} /> : canOpen && <OpenMeetingLink roomId={room.id} primary />}
            </>
          )
        }
      />
      {!self && !terminal && partsQ.isSuccess && <div className="mb-6"><JoinRoomForm roomId={room.id} defaultName={profile.data?.fullName} /></div>}
      {inLobby && <Alert tone="warning" title="Waiting for the host" className="mb-6">The host needs to admit you before you can enter the meeting. This page updates automatically.</Alert>}
      {self?.status === 'KICKED' && <Alert tone="danger" title="You were removed from this meeting" className="mb-6" />}
      {self?.status === 'CONNECTED' && !isHost && !terminal && (
        <div className="mb-6 flex flex-wrap items-center justify-between gap-3 rounded-lg border border-hairline bg-surface-1 px-4 py-3">
          <span className="text-body">You are in this meeting as <strong>{self.displayName}</strong>.</span>
          <Button size="sm" onClick={() => leave.mutate()} loading={leave.isPending}>Leave meeting</Button>
        </div>
      )}
      {terminal && <Alert tone="info" className="mb-6">This meeting is {room.status.toLowerCase()}.</Alert>}
      {inLobby ? null : partsQ.isError ? (
        <EmptyState title="Participants unavailable" description={errMsg(partsQ.error)} />
      ) : (
        <div className="flex min-w-0 flex-col gap-6">
          <ParticipantsCard roomId={room.id} list={list} canModerate={canModerate && !terminal} selfId={userId} />
          {canModerate && <RoutesCard roomId={room.id} canGenerate={canModerate && !terminal} participants={list} />}
        </div>
      )}
    </>
  );
}
