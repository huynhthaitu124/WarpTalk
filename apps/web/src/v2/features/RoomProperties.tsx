import { useState, type FormEvent, type ReactNode } from 'react';
import { CalendarBlank, Check, DoorOpen, LockSimple, Pause, Play, Prohibit, Stop, Translate } from '@phosphor-icons/react';
import {
  Avatar, Button, Checkbox, Dialog, DialogContent, Dropdown, DropdownContent, DropdownItem, DropdownSeparator, DropdownTrigger,
  Field, FlagLabel, Input, Select, StatusDot, Textarea, useToast,
} from '@v2/ui';
import { cn } from '@v2/lib/cn';
import { LANGUAGES } from '@v2/lib/languages';
import { languageOptions } from '@v2/lib/languageOptions';
import { useRoomAction, useUpdateRoom, type RoomAction, type RoomPatch } from '@v2/lib/queries';
import type { Participant, Room } from '@v2/lib/schemas';
import { formatShortDate, formatWhen, langCodes, roomMoment } from './common';
import { errMsg } from './errors';

const STATUS_TEXT: Record<string, string> = {
  SCHEDULED: 'text-chip-emerald-text', WAITING: 'text-chip-amber-text', OPEN: 'text-chip-sky-text', IN_PROGRESS: 'text-chip-emerald-text',
  CANCELLED: 'text-chip-rose-text', FAILED: 'text-chip-rose-text',
};
const statusLabel = (s: string) => (s === 'IN_PROGRESS' ? 'Live' : s.toLowerCase().replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase()));

const ROW = 'flex min-h-9 w-full items-center gap-2.5 text-left text-body text-ink';
const BTN = '-mx-1.5 rounded-md px-1.5 hover:bg-surface-2 focus-ring data-[state=open]:bg-surface-2 cursor-pointer';

function Icon({ children }: { children: ReactNode }) {
  return <span aria-hidden className="flex size-4 shrink-0 items-center justify-center text-ink-muted">{children}</span>;
}

/** Read-only property line (Linear style: muted icon + value; the label is for screen readers and the tooltip). */
export function PropItem({ label, icon, children }: { label: string; icon: ReactNode; children: ReactNode }) {
  return (
    <li className={ROW} title={label}>
      <Icon>{icon}</Icon>
      <span className="sr-only">{label}: </span>
      <span className="flex min-w-0 items-center truncate">{children}</span>
    </li>
  );
}

/** Clickable property line; pass `asTrigger` props through (Radix `asChild`). */
const PropButton = ({ label, icon, children, className, ...p }: { label: string; icon: ReactNode; children: ReactNode } & React.ComponentProps<'button'>) => (
  <button type="button" title={label} className={cn(ROW, BTN, className)} {...p}>
    <Icon>{icon}</Icon>
    <span className="sr-only">{label}: </span>
    <span className="flex min-w-0 items-center truncate">{children}</span>
  </button>
);

export function PropGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="flex flex-col gap-1">
      <h2 className="mb-1 text-body-sm font-medium text-ink-muted">{title}</h2>
      <ul className="flex flex-col">{children}</ul>
    </section>
  );
}

function useSave(roomId: string) {
  const upd = useUpdateRoom(roomId);
  const toast = useToast();
  const save = async (patch: RoomPatch, title = 'Meeting updated') => {
    try { await upd.mutateAsync(patch); toast({ tone: 'success', title }); return true; } catch (e) { toast({ tone: 'danger', title: 'Could not save', description: errMsg(e) }); return false; }
  };
  return { save, pending: upd.isPending };
}

// ------------------------------------------------------------------ status

function StatusProp({ room }: { room: Room }) {
  const act = useRoomAction(room.id);
  const toast = useToast();
  const [confirm, setConfirm] = useState<null | 'end' | 'cancel'>(null);
  const s = room.status;
  const startable = ['SCHEDULED', 'WAITING', 'OPEN'].includes(s);
  const live = s === 'IN_PROGRESS';
  const canEnd = ['IN_PROGRESS', 'PAUSED', 'WAITING', 'OPEN'].includes(s);
  const run = async (a: RoomAction, title: string) => {
    try { await act.mutateAsync(a); toast({ tone: 'success', title }); } catch (e) { toast({ tone: 'danger', title: 'Action failed', description: errMsg(e) }); }
    setConfirm(null);
  };
  return (
    <li>
      <Dropdown>
        <DropdownTrigger asChild>
          <PropButton label="Status" icon={<StatusDot className={STATUS_TEXT[s] ?? 'text-ink-muted'} />} disabled={act.isPending}>{statusLabel(s)}</PropButton>
        </DropdownTrigger>
        <DropdownContent className="w-52">
          {startable && <DropdownItem icon={<Play size={16} weight="fill" />} onSelect={() => void run('start', 'Meeting started')}>Start meeting</DropdownItem>}
          {live && <DropdownItem icon={<Pause size={16} weight="fill" />} onSelect={() => void run('pause', 'Meeting paused')}>Pause</DropdownItem>}
          {s === 'PAUSED' && <DropdownItem icon={<Play size={16} weight="fill" />} onSelect={() => void run('resume', 'Meeting resumed')}>Resume</DropdownItem>}
          {live && <DropdownItem icon={<Translate size={16} />} onSelect={() => void run('translation/stop', 'Translation stopped')}>Stop translation</DropdownItem>}
          {startable && <DropdownItem icon={<Prohibit size={16} />} onSelect={() => setConfirm('cancel')}>Cancel meeting</DropdownItem>}
          {canEnd && <><DropdownSeparator /><DropdownItem danger icon={<Stop size={16} weight="fill" />} onSelect={() => setConfirm('end')}>End meeting</DropdownItem></>}
        </DropdownContent>
      </Dropdown>
      <Dialog open={confirm !== null} onOpenChange={(v) => { if (!v) setConfirm(null); }}>
        <DialogContent
          title={confirm === 'end' ? 'End this meeting?' : 'Cancel this meeting?'}
          description={confirm === 'end' ? 'It ends for everyone and cannot be restarted.' : 'Nobody will be able to join it.'}
          footer={
            <>
              <Button onClick={() => setConfirm(null)}>Back</Button>
              <Button variant="danger" loading={act.isPending} onClick={() => void run(confirm === 'end' ? 'end' : 'cancel', confirm === 'end' ? 'Meeting ended' : 'Meeting cancelled')}>
                {confirm === 'end' ? 'End meeting' : 'Cancel meeting'}
              </Button>
            </>
          }
        />
      </Dialog>
    </li>
  );
}

// ------------------------------------------------------------------ languages

function LanguagesProp({ room }: { room: Room }) {
  const { save, pending } = useSave(room.id);
  const [open, setOpen] = useState(false);
  const [source, setSource] = useState(room.sourceLanguage);
  const [targets, setTargets] = useState<string[]>(room.targetLanguages);
  const reset = (v: boolean) => { if (v) { setSource(room.sourceLanguage); setTargets(room.targetLanguages); } setOpen(v); };
  const valid = targets.filter((t) => t !== source);
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (valid.length === 0) return;
    if (await save({ sourceLanguage: source, targetLanguages: valid }, 'Languages updated')) setOpen(false);
  }
  return (
    <li>
      <PropButton label="Languages" icon={<Translate size={16} />} onClick={() => reset(true)}>{langCodes(room)}</PropButton>
      <Dialog open={open} onOpenChange={reset}>
        <DialogContent
          title="Meeting languages"
          description="People who picked a language you remove switch to the source language."
          footer={<><Button onClick={() => setOpen(false)}>Cancel</Button><Button variant="primary" type="submit" form="room-langs" loading={pending} disabled={valid.length === 0}>Save</Button></>}
        >
          <form id="room-langs" onSubmit={submit} className="flex flex-col gap-4">
            <Field label="Source language">
              <Select value={source} onValueChange={(v) => { setSource(v); setTargets((t) => t.filter((c) => c !== v)); }} options={languageOptions()} />
            </Field>
            <fieldset className="flex flex-col gap-2">
              <legend className="mb-1.5 text-body-sm font-medium text-ink-muted">Target languages</legend>
              {LANGUAGES.filter((l) => l.code !== source).map((l) => (
                <Checkbox
                  key={l.code}
                  checked={targets.includes(l.code)}
                  onCheckedChange={(v) => setTargets((t) => (v ? [...t, l.code] : t.filter((c) => c !== l.code)))}
                  label={<FlagLabel code={l.code}>{l.label}</FlagLabel>}
                />
              ))}
              {valid.length === 0 && <p role="alert" className="text-caption text-danger-text">Add at least one target language.</p>}
            </fieldset>
          </form>
        </DialogContent>
      </Dialog>
    </li>
  );
}

// ------------------------------------------------------------------ date

/** `YYYY-MM-DDTHH:mm` in local time for `<input type="datetime-local">`. */
function toLocalInput(iso: string) {
  const d = new Date(iso);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

function DateProp({ room }: { room: Room }) {
  const { save, pending } = useSave(room.id);
  const [open, setOpen] = useState(false);
  const [value, setValue] = useState('');
  const moment = roomMoment(room);
  const editable = !room.startedAt;
  const reset = (v: boolean) => { if (v) setValue(room.scheduledAt ? toLocalInput(room.scheduledAt) : ''); setOpen(v); };
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!value) return;
    if (await save({ scheduledAt: new Date(value).toISOString() }, 'Date updated')) setOpen(false);
  }
  const shown = moment ? <span title={formatWhen(moment)}>{formatShortDate(moment)}</span> : 'Set date';
  if (!editable) return moment ? <PropItem label="Date" icon={<CalendarBlank size={16} />}>{shown}</PropItem> : null;
  return (
    <li>
      <PropButton label="Date" icon={<CalendarBlank size={16} />} onClick={() => reset(true)}>{shown}</PropButton>
      <Dialog open={open} onOpenChange={reset}>
        <DialogContent
          title="Schedule"
          description="A future date marks the meeting as scheduled. People can still join before it starts."
          footer={
            <>
              {room.scheduledAt && <Button className="mr-auto" disabled={pending} onClick={() => void save({ scheduledAt: null }, 'Schedule cleared').then((ok) => ok && setOpen(false))}>Clear date</Button>}
              <Button onClick={() => setOpen(false)}>Cancel</Button>
              <Button variant="primary" type="submit" form="room-date" loading={pending} disabled={!value}>Save</Button>
            </>
          }
        >
          <form id="room-date" onSubmit={submit}>
            <Field label="Date and time"><Input type="datetime-local" value={value} onChange={(e) => setValue(e.target.value)} autoFocus /></Field>
          </form>
        </DialogContent>
      </Dialog>
    </li>
  );
}

// ------------------------------------------------------------------ lobby

function LobbyProp({ room }: { room: Room }) {
  const { save, pending } = useSave(room.id);
  const on = room.requiresApproval;
  const pick = (v: boolean) => { if (v !== on) void save({ requiresApproval: v }, v ? 'Lobby turned on' : 'Lobby turned off'); };
  return (
    <li>
      <Dropdown>
        <DropdownTrigger asChild>
          <PropButton label="Lobby" icon={on ? <LockSimple size={16} /> : <DoorOpen size={16} />} disabled={pending}>{on ? 'Lobby on' : 'Lobby off'}</PropButton>
        </DropdownTrigger>
        <DropdownContent className="w-64">
          <DropdownItem icon={<LockSimple size={16} />} shortcut={on ? <Check size={14} weight="bold" /> : undefined} onSelect={() => pick(true)}>Lobby on · host admits people</DropdownItem>
          <DropdownItem icon={<DoorOpen size={16} />} shortcut={!on ? <Check size={14} weight="bold" /> : undefined} onSelect={() => pick(false)}>Lobby off · anyone joins</DropdownItem>
        </DropdownContent>
      </Dropdown>
    </li>
  );
}

/** Editable property list for hosts; read-only lines for everybody else or once the meeting is over. */
export function RoomProperties({ room, hostName, editable }: { room: Room; hostName?: string; editable: boolean }) {
  const moment = roomMoment(room);
  return (
    <PropGroup title="Properties">
      {editable ? <StatusProp room={room} /> : <PropItem label="Status" icon={<StatusDot className={STATUS_TEXT[room.status] ?? 'text-ink-muted'} />}>{statusLabel(room.status)}</PropItem>}
      {hostName && <PropItem label="Host" icon={<Avatar name={hostName} size="sm" className="size-4 text-[7px]" />}>{hostName}</PropItem>}
      {editable ? <LanguagesProp room={room} /> : <PropItem label="Languages" icon={<Translate size={16} />}>{langCodes(room)}</PropItem>}
      {editable ? <DateProp room={room} /> : moment && <PropItem label="Date" icon={<CalendarBlank size={16} />}><span title={formatWhen(moment)}>{formatShortDate(moment)}</span></PropItem>}
      {editable ? <LobbyProp room={room} /> : <PropItem label="Lobby" icon={room.requiresApproval ? <LockSimple size={16} /> : <DoorOpen size={16} />}>{room.requiresApproval ? 'Lobby on' : 'Lobby off'}</PropItem>}
    </PropGroup>
  );
}

// ------------------------------------------------------------------ people

const PEOPLE_STATUS: Record<string, string> = {
  CONNECTED: 'In room', WAITING: 'Waiting', DISCONNECTED: 'Attended', LEFT: 'Attended', KICKED: 'Removed', REJECTED: 'Declined', INVITED: 'Invited',
};

/** People count line that opens the matching list. */
export function PeopleProp({ label, icon, text, people }: { label: string; icon: ReactNode; text: string; people: Participant[] }) {
  const [open, setOpen] = useState(false);
  return (
    <li>
      <PropButton label={label} icon={icon} onClick={() => setOpen(true)}>{text}</PropButton>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent title={label} footer={<Button onClick={() => setOpen(false)}>Close</Button>}>
          {people.length === 0 ? (
            <p className="py-6 text-center text-body-sm text-ink-subtle">Nobody here yet.</p>
          ) : (
            <ul className="flex flex-col">
              {people.map((p) => (
                <li key={p.id} className="flex items-center gap-3 border-b border-row py-2 last:border-0">
                  <Avatar name={p.displayName} />
                  <span className="min-w-0 flex-1 truncate text-body font-medium">{p.displayName}{p.role === 'HOST' && <span className="ml-1.5 text-caption font-normal text-ink-subtle">Host</span>}</span>
                  <FlagLabel code={p.speakLanguage}>{p.speakLanguage.toUpperCase()}</FlagLabel>
                  <span className="w-20 text-right text-caption text-ink-subtle">{PEOPLE_STATUS[p.status] ?? p.status}</span>
                </li>
              ))}
            </ul>
          )}
        </DialogContent>
      </Dialog>
    </li>
  );
}

// ------------------------------------------------------------------ title / description

/** Click-to-edit heading; Enter or blur saves, Escape cancels. */
export function EditableTitle({ room, editable }: { room: Room; editable: boolean }) {
  const { save } = useSave(room.id);
  const [draft, setDraft] = useState<string | null>(null);
  const cls = 'break-words text-headline font-semibold text-ink max-sm:text-title';
  if (!editable) return <h1 className={cls} title={room.title}>{room.title}</h1>;
  if (draft === null) {
    return (
      <h1 className={cls}>
        <button type="button" aria-label={`Edit title: ${room.title}`} onClick={() => setDraft(room.title)} className="-mx-1.5 w-[calc(100%+12px)] cursor-text rounded-md px-1.5 text-left hover:bg-surface-2 focus-ring">{room.title}</button>
      </h1>
    );
  }
  const commit = async () => {
    const t = draft.trim();
    if (t && t !== room.title && !(await save({ title: t }, 'Title updated'))) return;
    setDraft(null);
  };
  return (
    <input
      aria-label="Title"
      autoFocus
      maxLength={255}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => void commit()}
      onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void commit(); } else if (e.key === 'Escape') setDraft(null); }}
      className={cn(cls, '-mx-1.5 w-[calc(100%+12px)] rounded-md bg-surface-2 px-1.5 outline-none focus-ring')}
    />
  );
}

/** Click-to-edit description; ⌘/Ctrl+Enter or blur saves, Escape cancels. */
export function EditableDescription({ room, editable }: { room: Room; editable: boolean }) {
  const { save } = useSave(room.id);
  const [draft, setDraft] = useState<string | null>(null);
  const text = room.description?.trim();
  const cls = 'whitespace-pre-wrap break-words text-[15px] leading-7';
  if (!editable) {
    return text ? <p className={cn(cls, 'text-ink')} data-testid="room-description">{text}</p> : null;
  }
  if (draft === null) {
    return (
      <button
        type="button"
        aria-label="Edit description"
        onClick={() => setDraft(text ?? '')}
        className={cn(cls, '-mx-1.5 cursor-text rounded-md px-1.5 text-left hover:bg-surface-2 focus-ring', text ? 'text-ink' : 'text-ink-subtle')}
        data-testid="room-description"
      >
        {text || 'Add a description…'}
      </button>
    );
  }
  const commit = async () => {
    const d = draft.trim();
    if (d !== (text ?? '') && !(await save({ description: d || null }, 'Description updated'))) return;
    setDraft(null);
  };
  return (
    <Textarea
      aria-label="Description"
      autoFocus
      rows={Math.max(3, draft.split('\n').length)}
      maxLength={5000}
      value={draft}
      placeholder="Agenda, context or notes for participants"
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => void commit()}
      onKeyDown={(e) => { if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) { e.preventDefault(); void commit(); } else if (e.key === 'Escape') setDraft(null); }}
      className="text-[15px] leading-7"
    />
  );
}
