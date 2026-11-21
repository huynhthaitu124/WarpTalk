import { useMemo, useState, type FormEvent } from 'react';
import { Link, useParams } from 'react-router-dom';
import { Copy, Pause, Play, Square, UserCheck, UserX, Video, XCircle, RefreshCw, Languages } from 'lucide-react';
import {
  Alert, Badge, Button, Card, CardBody, CardHeader, EmptyState, Field, Input, PageHeader, Select, Skeleton, StatusPill, Table, TBody, TD, TH, THead, TR, useToast, Avatar,
} from '@/ui';
import { useAuth } from '@/lib/auth';
import { canAdmin, useWorkspace } from '@/lib/workspace';
import {
  useGenerateRoutes, useJoinById, useLeaveRoom, useParticipantAction, useParticipants, useRoom, useRoomAction, useRoutes, type RoomAction,
} from '@/lib/queries';
import { ApiError } from '@/lib/api';
import { LANGUAGES, languageLabel } from '@/lib/languages';
import type { Participant, Room } from '@/lib/schemas';
import { errMsg } from './errors';

const TERMINAL = ['ENDED', 'CANCELLED', 'EXPIRED', 'FAILED'];

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
      <CardHeader title="Join this room" description="Choose how you speak and what you want to hear." />
      <CardBody>
        <form onSubmit={submit} className="flex flex-col gap-4">
          <Field label="Display name (optional)"><Input value={name} onChange={(e) => setName(e.target.value)} maxLength={100} /></Field>
          <Field label="I speak"><Select value={speak} onValueChange={setSpeak} options={opts} /></Field>
          <Field label="I want to listen in"><Select value={listen} onValueChange={setListen} options={opts} /></Field>
          {join.isError && <Alert tone="danger">{errMsg(join.error)}</Alert>}
          <Button type="submit" variant="primary" loading={join.isPending}>Join room</Button>
        </form>
      </CardBody>
    </Card>
  );
}

function HostActions({ room }: { room: Room }) {
  const act = useRoomAction(room.id);
  const toast = useToast();
  const [error, setError] = useState('');
  const run = async (a: RoomAction, label: string) => {
    setError('');
    try { await act.mutateAsync(a); toast({ tone: 'success', title: label }); } catch (e) { setError(errMsg(e)); }
  };
  const s = room.status;
  const pending = act.isPending;
  return (
    <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Room controls">
      {['SCHEDULED', 'WAITING', 'OPEN'].includes(s) && <Button variant="primary" disabled={pending} onClick={() => run('start', 'Room started')}><Play className="size-4" aria-hidden />Start</Button>}
      {s === 'IN_PROGRESS' && <Button disabled={pending} onClick={() => run('pause', 'Room paused')}><Pause className="size-4" aria-hidden />Pause</Button>}
      {s === 'PAUSED' && <Button variant="primary" disabled={pending} onClick={() => run('resume', 'Room resumed')}><Play className="size-4" aria-hidden />Resume</Button>}
      {s === 'IN_PROGRESS' && <Button disabled={pending} onClick={() => run('translation/stop', 'Translation stopped')}><Languages className="size-4" aria-hidden />Stop translation</Button>}
      {['SCHEDULED', 'WAITING', 'OPEN'].includes(s) && <Button disabled={pending} onClick={() => run('cancel', 'Room cancelled')}><XCircle className="size-4" aria-hidden />Cancel</Button>}
      {['IN_PROGRESS', 'PAUSED', 'WAITING', 'OPEN'].includes(s) && <Button variant="danger" disabled={pending} onClick={() => run('end', 'Room ended')}><Square className="size-4" aria-hidden />End</Button>}
      {error && <span role="alert" className="text-body-sm text-danger-text">{error}</span>}
    </div>
  );
}

function ParticipantsCard({ roomId, list, canModerate, selfId }: { roomId: string; list: Participant[]; canModerate: boolean; selfId: string | null }) {
  const pa = useParticipantAction(roomId);
  const toast = useToast();
  const waiting = list.filter((p) => p.status === 'WAITING');
  const others = list.filter((p) => p.status !== 'WAITING');
  async function run(p: Participant, action: 'admit' | 'kick') {
    try {
      await pa.mutateAsync({ pid: p.id, action });
      toast({ tone: 'success', title: action === 'admit' ? `${p.displayName} admitted` : `${p.displayName} removed` });
    } catch (e) { toast({ tone: 'danger', title: 'Action failed', description: errMsg(e) }); }
  }
  return (
    <div className="flex flex-col gap-6">
      {canModerate && waiting.length > 0 && (
        <Card data-testid="lobby">
          <CardHeader title={`Lobby (${waiting.length})`} description="These people are waiting for you to let them in." />
          <ul className="divide-y divide-hairline" aria-label="Lobby">
            {waiting.map((p) => (
              <li key={p.id} className="flex items-center gap-3 px-5 py-3">
                <Avatar name={p.displayName} />
                <span className="flex-1 truncate text-body font-medium">{p.displayName}</span>
                <Button size="sm" variant="primary" onClick={() => run(p, 'admit')} disabled={pa.isPending} aria-label={`Admit ${p.displayName}`}><UserCheck className="size-4" aria-hidden />Admit</Button>
                <Button size="sm" onClick={() => run(p, 'kick')} disabled={pa.isPending} aria-label={`Deny ${p.displayName}`}><UserX className="size-4" aria-hidden />Deny</Button>
              </li>
            ))}
          </ul>
        </Card>
      )}
      <Card>
        <CardHeader title={`Participants (${others.length})`} />
        {others.length === 0 ? (
          <CardBody><p className="text-body text-ink-subtle">Nobody has joined yet.</p></CardBody>
        ) : (
          <Table aria-label="Participants">
            <THead><TR><TH>Name</TH><TH>Role</TH><TH>Speaks</TH><TH>Listens</TH><TH>Status</TH>{canModerate && <TH><span className="sr-only">Actions</span></TH>}</TR></THead>
            <TBody>
              {others.map((p) => (
                <TR key={p.id}>
                  <TD><span className="flex items-center gap-2"><Avatar name={p.displayName} size="sm" />{p.displayName}{p.userId === selfId && <Badge>You</Badge>}</span></TD>
                  <TD><StatusPill status={p.role} /></TD>
                  <TD>{languageLabel(p.speakLanguage)}</TD>
                  <TD>{languageLabel(p.listenLanguage)}</TD>
                  <TD><StatusPill status={p.status} /></TD>
                  {canModerate && (
                    <TD className="text-right">
                      {p.role !== 'HOST' && !['KICKED', 'LEFT', 'REJECTED'].includes(p.status) && (
                        <Button size="sm" variant="ghost" onClick={() => run(p, 'kick')} aria-label={`Kick ${p.displayName}`} disabled={pa.isPending}>Kick</Button>
                      )}
                    </TD>
                  )}
                </TR>
              ))}
            </TBody>
          </Table>
        )}
      </Card>
    </div>
  );
}

function RoutesCard({ roomId, canGenerate, participants }: { roomId: string; canGenerate: boolean; participants: Participant[] }) {
  const routes = useRoutes(roomId);
  const gen = useGenerateRoutes(roomId);
  const names = useMemo(() => new Map(participants.map((p) => [p.id, p.displayName])), [participants]);
  const items = routes.data?.routes ?? [];
  return (
    <Card>
      <CardHeader
        title="Audio routes"
        description="Who hears whom, in which language."
        actions={canGenerate && <Button size="sm" loading={gen.isPending} onClick={() => gen.mutate()}><RefreshCw className="size-4" aria-hidden />Generate routes</Button>}
      />
      {routes.isLoading ? (
        <CardBody><Skeleton className="h-10" /></CardBody>
      ) : routes.isError ? (
        <CardBody><Alert tone="warning">Audio routes are not available yet: {errMsg(routes.error)}</Alert></CardBody>
      ) : items.length === 0 ? (
        <CardBody><p className="text-body text-ink-subtle">No routes yet. Routes appear once the room is live and participants have joined.</p></CardBody>
      ) : (
        <Table aria-label="Audio routes">
          <THead><TR><TH>Speaker</TH><TH>Listener</TH><TH>Language</TH><TH>Voice clone</TH><TH>Status</TH></TR></THead>
          <TBody>
            {items.map((r) => (
              <TR key={r.routeId}>
                <TD>{names.get(r.sourceParticipantId) ?? 'Participant'}</TD>
                <TD>{names.get(r.targetParticipantId) ?? 'Participant'}</TD>
                <TD>{languageLabel(r.sourceLanguage)} → {languageLabel(r.targetLanguage)}</TD>
                <TD>{r.voiceCloneEnabled ? 'On' : 'Off'}</TD>
                <TD><Badge>{typeof r.status === 'string' ? r.status : JSON.stringify(r.status)}</Badge></TD>
              </TR>
            ))}
            </TBody>
          </Table>
        )}
    </Card>
  );
}

export function RoomDetailPage() {
  const { roomId = '' } = useParams();
  const { userId } = useAuth();
  const { workspaces } = useWorkspace();
  const toast = useToast();
  const roomQ = useRoom(roomId);
  const partsQ = useParticipants(roomId);
  const leave = useLeaveRoom(roomId);

  if (roomQ.isLoading) return <div className="flex flex-col gap-3"><Skeleton className="h-10 w-64" /><Skeleton className="h-40" /></div>;
  if (roomQ.isError) {
    const notFound = roomQ.error instanceof ApiError && roomQ.error.status === 404;
    return notFound ? (
      <div className="flex flex-col gap-4">
        <PageHeader title="Room" description="You are not part of this room yet." />
        <JoinRoomForm roomId={roomId} />
        <Link to="/rooms" className="mx-auto text-body-sm">Back to rooms</Link>
      </div>
    ) : <Alert tone="danger" title="Could not load room">{errMsg(roomQ.error)}</Alert>;
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
          <>
            <Button
              aria-label="Copy room code"
              onClick={() => { void navigator.clipboard?.writeText(room.roomCode).catch(() => undefined); toast({ tone: 'info', title: 'Room code copied', description: room.roomCode }); }}
            >
              <Copy className="size-4" aria-hidden /><code data-testid="room-code">{room.roomCode}</code>
            </Button>
            {!terminal && !inLobby && (self || canModerate) && (
              <Link to={`/meetings/${room.id}`} className="inline-flex h-8 items-center gap-2 rounded-md bg-primary px-3.5 font-medium text-on-primary no-underline hover:bg-primary-hover hover:no-underline">
                <Video className="size-4" aria-hidden />Open meeting
              </Link>
            )}
          </>
        }
      />
      {inLobby && <Alert tone="warning" title="Waiting for the host" className="mb-6">The host needs to admit you before you can enter the meeting. This page updates automatically.</Alert>}
      {self?.status === 'KICKED' && <Alert tone="danger" title="You were removed from this room" className="mb-6" />}
      {self?.status === 'CONNECTED' && !isHost && !terminal && (
        <div className="mb-6 flex items-center justify-between gap-3 rounded-lg border border-hairline bg-surface-1 px-4 py-3">
          <span className="text-body">You are in this room as <strong>{self.displayName}</strong>.</span>
          <Button size="sm" onClick={() => leave.mutate()} loading={leave.isPending}>Leave room</Button>
        </div>
      )}
      {canModerate && !terminal && <div className="mb-6"><HostActions room={room} /></div>}
      {terminal && <Alert tone="info" className="mb-6">This room is {room.status.toLowerCase()}.</Alert>}
      {partsQ.isError ? (
        inLobby ? null : <EmptyState title="Participants unavailable" description={errMsg(partsQ.error)} />
      ) : (
        <div className="flex flex-col gap-6">
          <ParticipantsCard roomId={room.id} list={list} canModerate={canModerate && !terminal} selfId={userId} />
          {canModerate && <RoutesCard roomId={room.id} canGenerate={canModerate && !terminal} participants={list} />}
        </div>
      )}
    </>
  );
}
