import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Check, Copy, EnvelopeSimple } from '@phosphor-icons/react';
import { Alert, Badge, Button, StatusPill, useToast } from '@/ui';
import { languageLabel } from '@/lib/languages';
import { useAuth } from '@/lib/auth';
import { useAcceptInvitation, usePendingInvitations } from '@/lib/queries';
import { useWorkspace } from '@/lib/workspace';
import type { Room } from '@/lib/schemas';
import { errMsg } from './errors';

export const LIVE_STATUSES = ['IN_PROGRESS', 'PAUSED', 'OPEN', 'WAITING'];
export const UPCOMING_STATUSES = ['SCHEDULED'];
export const TERMINAL_STATUSES = ['ENDED', 'CANCELLED', 'EXPIRED', 'FAILED'];

const when = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' });
export function formatWhen(iso: string | null | undefined): string {
  if (!iso) return '';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? '' : when.format(d);
}
/** The moment a room is "about": scheduled start, else actual start/end, else creation. */
export function roomMoment(r: Room): string | null | undefined {
  return r.scheduledAt ?? r.startedAt ?? r.endedAt ?? r.createdAt;
}

/** Copy to the clipboard and say so only when it actually worked (FN-09). */
export function useCopy() {
  const toast = useToast();
  return async (text: string, what: string) => {
    try {
      if (!navigator.clipboard) throw new Error('clipboard unavailable');
      await navigator.clipboard.writeText(text);
      toast({ tone: 'success', title: `${what} copied`, description: text });
      return true;
    } catch {
      toast({ tone: 'warning', title: `Could not copy the ${what.toLowerCase()}`, description: `Select it and copy manually: ${text}` });
      return false;
    }
  };
}

/** The room code as a monospace chip with a copy action; the code itself stays selectable text. */
export function CodeChip({ code, className }: { code: string; className?: string }) {
  const copy = useCopy();
  const [done, setDone] = useState(false);
  return (
    <span className={`inline-flex h-6 items-center gap-1 rounded-md bg-surface-2 pl-2 text-caption text-ink-muted ${className ?? ''}`}>
      <code data-testid="room-code-chip">{code}</code>
      <button
        type="button"
        aria-label={`Copy room code ${code}`}
        className="grid size-6 place-items-center rounded-md text-ink-subtle hover:bg-surface-3 hover:text-ink focus-ring"
        onClick={() => void copy(code, 'Room code').then((ok) => { if (ok) { setDone(true); setTimeout(() => setDone(false), 1500); } })}
      >
        {done ? <Check size={12} weight="bold" aria-hidden /> : <Copy size={12} aria-hidden />}
      </button>
    </span>
  );
}

/** A room as a list row; stacks below 640px; the title truncates and carries a tooltip. */
export function RoomRow({ room, showOpen = false }: { room: Room; showOpen?: boolean }) {
  const live = LIVE_STATUSES.includes(room.status);
  const moment = formatWhen(roomMoment(room));
  return (
    <div role="listitem" className="flex flex-col gap-2 border-b border-hairline px-4 py-3 last:border-0 hover:bg-surface-2/60 sm:flex-row sm:items-center sm:gap-3">
      <div className="min-w-0 flex-1">
        <Link to={`/rooms/${room.id}`} title={room.title} className="block truncate text-body font-medium text-ink hover:text-link">{room.title}</Link>
        <p className="mt-0.5 flex flex-wrap gap-x-2 text-caption text-ink-subtle">
          <span>{languageLabel(room.sourceLanguage)} → {room.targetLanguages.map(languageLabel).join(', ')}</span>
          {moment && <span>{moment}</span>}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2 sm:shrink-0">
        <CodeChip code={room.roomCode} />
        {room.requiresApproval && <Badge>Lobby</Badge>}
        <StatusPill status={room.status} />
        {showOpen && live && (
          <Link to={`/meetings/${room.id}`} aria-label={`Open meeting ${room.title}`}><Button size="sm" variant="primary" tabIndex={-1}>Open</Button></Link>
        )}
      </div>
    </div>
  );
}

/** Invitations addressed to me: accept with one click (the workspace becomes current). */
export function PendingInvitations({ className }: { className?: string }) {
  const { userId } = useAuth();
  const pending = usePendingInvitations(userId);
  const accept = useAcceptInvitation();
  const { select } = useWorkspace();
  const toast = useToast();
  const nav = useNavigate();
  const items = pending.data ?? [];
  if (items.length === 0) return null;
  return (
    <section aria-label="Pending invitations" className={`flex flex-col gap-2 ${className ?? ''}`}>
      {items.map((inv) => (
        <Alert key={inv.id} tone="info" className="flex flex-wrap items-center justify-between gap-3">
          <span className="flex min-w-0 items-center gap-2">
            <EnvelopeSimple size={16} weight="duotone" aria-hidden className="shrink-0" />
            <span className="min-w-0 break-words">
              You were invited to <strong>{inv.workspaceName ?? 'a workspace'}</strong> as {inv.role.toLowerCase()}{inv.inviterEmail ? ` by ${inv.inviterEmail}` : ''}.
            </span>
          </span>
          <Button
            size="sm"
            variant="primary"
            loading={accept.isPending && accept.variables === inv.id}
            aria-label={`Accept invitation to ${inv.workspaceName ?? 'workspace'}`}
            onClick={async () => {
              try {
                const r = await accept.mutateAsync(inv.id);
                select(r.workspaceId);
                toast({ tone: 'success', title: `Joined ${r.workspaceName}` });
                nav('/');
              } catch (e) { toast({ tone: 'danger', title: 'Could not accept the invitation', description: errMsg(e) }); }
            }}
          >
            Accept
          </Button>
        </Alert>
      ))}
    </section>
  );
}
