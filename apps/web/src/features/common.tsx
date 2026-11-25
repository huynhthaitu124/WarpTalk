import { useState } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Check, Copy, EnvelopeSimple, Globe } from '@phosphor-icons/react';
import { Alert, Badge, Button, Capsule, DateCapsule, HostCapsule, LanguageCapsule, StatusDot, StatusPill, useToast } from '@/ui';
import { cn } from '@/lib/cn';
import { languageLabel } from '@/lib/languages';
import { useAuth } from '@/lib/auth';
import { useAcceptInvitation, useMembers, usePendingInvitations } from '@/lib/queries';
import { useWorkspace } from '@/lib/workspace';
import type { Room } from '@/lib/schemas';
import { PROVIDER_LABEL, externalMeetingHref, type BridgeProvider } from '@/lib/bridge';
import { errMsg } from './errors';

/** Rooms people can open (everything that is not scheduled or finished). */
export const LIVE_STATUSES = ['IN_PROGRESS', 'PAUSED', 'OPEN', 'WAITING'];
/** Truly live: translation is running. Home's "Live now" shows only these. */
export const RUNNING_STATUSES = ['IN_PROGRESS'];
/** Open but not running: the lobby is open, the host has not started (or has paused) translation. */
export const WAITING_STATUSES = ['WAITING', 'OPEN', 'PAUSED'];
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

/** The room code as a white mono capsule with a copy action; the code itself stays selectable text. */
export function CodeChip({ code, className, size }: { code: string; className?: string; size?: 'sm' | 'lg' }) {
  const copy = useCopy();
  const [done, setDone] = useState(false);
  return (
    <Capsule size={size} className={cn('gap-0.5 pr-0.5 font-mono', className)}>
      <code data-testid="room-code-chip">{code}</code>
      <button
        type="button"
        aria-label={`Copy room code ${code}`}
        className="grid size-5 place-items-center rounded-pill text-ink-subtle hover:bg-surface-2 hover:text-ink focus-ring"
        onClick={() => void copy(code, 'Room code').then((ok) => { if (ok) { setDone(true); setTimeout(() => setDone(false), 1500); } })}
      >
        {done ? <Check size={12} weight="bold" aria-hidden /> : <Copy size={12} aria-hidden />}
      </button>
    </Capsule>
  );
}

const shortDate = new Intl.DateTimeFormat(undefined, { month: 'short', day: 'numeric' });
export function formatShortDate(iso: string | null | undefined): string {
  if (!iso) return '';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? '' : shortDate.format(d);
}
export const langCodes = (r: Pick<Room, 'sourceLanguage' | 'targetLanguages'>) => `${languageLabel(r.sourceLanguage)} → ${r.targetLanguages.map(languageLabel).join(', ')}`;

/** Row-leading 12px status ring (legacy: blue live, amber waiting, emerald scheduled, gray ended, red cancelled). */
export function RoomDot({ status }: { status: string }) {
  const c = status === 'IN_PROGRESS' ? 'text-blue-500'
    : ['WAITING', 'OPEN'].includes(status) ? 'text-amber-500'
    : status === 'SCHEDULED' ? 'text-emerald-500'
    : ['CANCELLED', 'FAILED'].includes(status) ? 'text-red-500'
    : 'text-gray-500';
  return <StatusDot className={cn('mr-1', c)} />;
}

/** Host name by id from the workspace member list (the room payload only carries the id). */
export function useHostName(room: Room | undefined): string | undefined {
  const { current } = useWorkspace();
  const members = useMembers(room && current?.id === room.workspaceId ? current.id : undefined);
  const m = room ? members.data?.items.find((x) => x.userId === (room.effectiveHostId ?? room.hostId)) : undefined;
  return m?.fullName || m?.email || undefined;
}

/**
 * A room as a full-bleed grid-line row (44px, hairline underneath): status ring, title, status chip,
 * host / language / date / code capsules. Stacks below 640px; `compact` drops the capsules that do
 * not fit a half-width list (Home).
 */
export function RoomRow({ room, showOpen = false, compact = false }: { room: Room; showOpen?: boolean; compact?: boolean }) {
  const live = LIVE_STATUSES.includes(room.status);
  const moment = roomMoment(room);
  const host = useHostName(room);
  return (
    <div role="listitem" className="flex min-h-11 flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-row px-4 py-1.5 hover:bg-surface-2/50 sm:flex-nowrap">
      <RoomDot status={room.status} />
      <Link to={`/rooms/${room.id}`} title={room.title} className="min-w-0 flex-1 basis-40 truncate text-body font-medium text-ink hover:text-link">{room.title}</Link>
      <div className="flex flex-wrap items-center gap-1.5 sm:shrink-0 sm:flex-nowrap">
        {room.bridge && <ProviderChip provider={room.bridge.provider} />}
        {room.requiresApproval && <Badge>Lobby</Badge>}
        <StatusPill status={room.status} />
        {!compact && host && <HostCapsule name={host} />}
        {!compact && <LanguageCapsule source={room.sourceLanguage} targets={room.targetLanguages} labels={langCodes(room)} />}
        {moment && <span title={formatWhen(moment)}><DateCapsule text={formatShortDate(moment)} /></span>}
        <CodeChip code={room.roomCode} />
        {showOpen && live && (
          <Link to={externalMeetingHref(room) ?? `/meetings/${room.id}`} aria-label={`Open meeting ${room.title}`}><Button size="sm" variant="primary" tabIndex={-1}>Open</Button></Link>
        )}
      </div>
    </div>
  );
}

/** Which external app an EXTERNAL_BRIDGE room runs in (Google Meet / Teams / Zoom). */
export function ProviderChip({ provider }: { provider: string }) {
  const label = PROVIDER_LABEL[provider as BridgeProvider] ?? 'External';
  return <Badge data-testid="provider-chip"><Globe size={12} weight="duotone" aria-hidden className="mr-1 inline" />{label}</Badge>;
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
