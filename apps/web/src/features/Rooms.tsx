import { useState, type FormEvent } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router-dom';
import { DoorOpen, Plus } from 'lucide-react';
import {
  Alert, Badge, Button, Checkbox, Dialog, DialogContent, EmptyState, Field, Input, ListRow, PageHeader, Select, Skeleton, StatusPill, useToast,
} from '@/ui';
import { useWorkspace } from '@/lib/workspace';
import { useCreateRoom, useRooms } from '@/lib/queries';
import { LANGUAGES, languageLabel } from '@/lib/languages';
import { ROOM_STATUSES } from '@/lib/schemas';
import { CreateWorkspaceDialog } from './Workspaces';
import { errMsg } from './errors';

export function CreateRoomDialog({ open, onOpenChange }: { open: boolean; onOpenChange(v: boolean): void }) {
  const { current } = useWorkspace();
  const create = useCreateRoom();
  const toast = useToast();
  const nav = useNavigate();
  const [title, setTitle] = useState('');
  const [source, setSource] = useState('vi');
  const [targets, setTargets] = useState<string[]>(['en']);
  const [approval, setApproval] = useState(false);

  const toggle = (code: string, on: boolean) => setTargets((t) => (on ? [...new Set([...t, code])] : t.filter((c) => c !== code)));

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!current) return;
    try {
      const r = await create.mutateAsync({
        workspaceId: current.id, title: title.trim(), sourceLanguage: source, targetLanguages: targets, requiresApproval: approval,
      });
      toast({ tone: 'success', title: 'Room created', description: r.title });
      setTitle('');
      onOpenChange(false);
      nav(`/rooms/${r.id}`);
    } catch { /* inline */ }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        title="Create room"
        description="A translation room with live captions and dubbed audio."
        footer={
          <>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button variant="primary" type="submit" form="create-room-form" loading={create.isPending} disabled={!title.trim() || targets.length === 0}>Create room</Button>
          </>
        }
      >
        <form id="create-room-form" onSubmit={submit} className="flex flex-col gap-4">
          <Field label="Title"><Input required maxLength={200} value={title} onChange={(e) => setTitle(e.target.value)} autoFocus /></Field>
          <Field label="Source language">
            <Select value={source} onValueChange={(v) => { setSource(v); setTargets((t) => t.filter((c) => c !== v)); }} options={LANGUAGES.map((l) => ({ value: l.code, label: l.label }))} />
          </Field>
          <fieldset className="flex flex-col gap-2">
            <legend className="mb-1.5 text-body-sm font-medium text-ink-muted">Target languages</legend>
            {LANGUAGES.filter((l) => l.code !== source).map((l) => (
              <Checkbox key={l.code} checked={targets.includes(l.code)} onCheckedChange={(v) => toggle(l.code, v)} label={l.label} />
            ))}
          </fieldset>
          <Checkbox checked={approval} onCheckedChange={setApproval} label="Requires approval" description="Participants wait in the lobby until the host admits them." />
          {create.isError && <Alert tone="danger">{errMsg(create.error)}</Alert>}
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function RoomsPage() {
  const { current, loading: wsLoading } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const [status, setStatus] = useState('all');
  const rooms = useRooms(current?.id, status === 'all' ? undefined : status);
  const open = params.get('new') === '1';
  const setOpen = (v: boolean) => setParams(v ? { new: '1' } : {}, { replace: true });

  if (wsLoading) return <Skeleton className="h-40" />;
  if (!current) {
    return (
      <>
        <PageHeader title="Rooms" />
        <EmptyState icon={<DoorOpen className="size-5" />} title="Create a workspace first" description="Rooms live inside a workspace." action={<Button variant="primary" onClick={() => setOpen(true)}>Create workspace</Button>} />
        <CreateWorkspaceDialog open={open} onOpenChange={setOpen} />
      </>
    );
  }
  const items = rooms.data?.items ?? [];
  return (
    <>
      <PageHeader
        title="Rooms"
        description={`Translation rooms in ${current.name}.`}
        actions={
          <>
            <Select aria-label="Filter by status" className="w-40" value={status} onValueChange={setStatus} options={[{ value: 'all', label: 'All statuses' }, ...ROOM_STATUSES.map((s) => ({ value: s, label: s.replace('_', ' ').toLowerCase().replace(/^./, (c) => c.toUpperCase()) }))]} />
            <Button variant="primary" onClick={() => setOpen(true)}><Plus className="size-4" aria-hidden />Create room</Button>
          </>
        }
      />
      {rooms.isLoading ? (
        <div className="flex flex-col gap-2"><Skeleton className="h-12" /><Skeleton className="h-12" /><Skeleton className="h-12" /></div>
      ) : rooms.isError ? (
        <Alert tone="danger" title="Could not load rooms">{errMsg(rooms.error)}</Alert>
      ) : items.length === 0 ? (
        <EmptyState icon={<DoorOpen className="size-5" />} title="No rooms yet" description="Create a room, share the code, and start translating." action={<Button variant="primary" onClick={() => setOpen(true)}>Create room</Button>} />
      ) : (
        <div className="overflow-hidden rounded-lg border border-hairline bg-surface-1" role="list" aria-label="Rooms">
          {items.map((r) => (
            <ListRow key={r.id} role="listitem">
              <div className="min-w-0 flex-1">
                <Link to={`/rooms/${r.id}`} className="block truncate text-body font-medium text-ink hover:text-link">{r.title}</Link>
                <p className="truncate text-caption text-ink-subtle">
                  {languageLabel(r.sourceLanguage)} → {r.targetLanguages.map(languageLabel).join(', ')} · code <code className="text-ink-muted">{r.roomCode}</code>
                </p>
              </div>
              {r.requiresApproval && <Badge>Lobby</Badge>}
              <StatusPill status={r.status} />
            </ListRow>
          ))}
        </div>
      )}
      <CreateRoomDialog open={open} onOpenChange={setOpen} />
    </>
  );
}
