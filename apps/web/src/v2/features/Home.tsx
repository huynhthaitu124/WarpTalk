import { useMemo, type ReactNode } from 'react';
import { Link, Navigate, useNavigate } from 'react-router-dom';
import { Brain, CheckCircle, Keyboard, Plus, Sparkle, SquaresFour, UserPlus } from '@phosphor-icons/react';
import { Alert, Button, EmptyState, GroupHeader, PageBody, PageHeader, SectionLabel, Skeleton } from '@v2/ui';
import { useProfile } from '@v2/lib/auth';
import { canAdmin, useWorkspace } from '@v2/lib/workspace';
import { useRoomList } from '@v2/lib/queries';
import { useFeatures } from '@v2/lib/features';
import { useSessions } from '@v2/lib/copilot';
import type { Room } from '@v2/lib/schemas';
import { PendingInvitations, RUNNING_STATUSES, RoomRow, TERMINAL_STATUSES, UPCOMING_STATUSES, WAITING_STATUSES, roomMoment } from './common';
import { errMsg } from './errors';
import { SessionRow } from './Sessions';

function Quick({ to, icon, title, primary }: { to: string; icon: ReactNode; title: string; hint?: string; primary?: boolean }) {
  return (
    <Link
      to={to}
      className={`group flex min-h-14 items-center gap-3 rounded-lg border p-3 no-underline shadow-card transition-all hover:-translate-y-0.5 hover:no-underline focus-ring ${primary ? 'border-primary bg-primary text-on-primary shadow-btn hover:bg-primary-hover' : 'border-hairline bg-surface-1 text-ink hover:border-hairline-strong hover:bg-surface-2'}`}
    >
      <span className={`grid size-9 shrink-0 place-items-center rounded-[9px] border ${primary ? 'border-white/20 bg-white/15 text-on-primary' : 'border-hairline bg-surface-2 text-primary'}`} aria-hidden>{icon}</span>
      <span className="min-w-0 truncate text-body font-medium">{title}</span>
    </Link>
  );
}

function RoomsBand({ title, description, items, emptyTitle, emptyHint, viewAll, showOpen }: {
  title: string; description: string; items: Room[]; emptyTitle: string; emptyHint: string; viewAll: string; showOpen?: boolean;
}) {
  return (
    <section className="min-w-0">
      <GroupHeader>
        <h2 className="font-medium text-ink">{title}</h2>
        <span aria-hidden>·</span>
        <span className="tabular-nums">{items.length}</span>
        <span className="truncate max-md:hidden">· {description}</span>
        <Link to={viewAll} className="ml-auto shrink-0 text-body-sm">View all</Link>
      </GroupHeader>
      {items.length === 0 ? (
        <p className="border-b border-row px-4 py-3 text-body-sm text-ink-muted"><span className="font-medium text-ink">{emptyTitle}</span> {emptyHint}</p>
      ) : (
        <div role="list" aria-label={title}>{items.map((r) => <RoomRow key={r.id} room={r} showOpen={showOpen} compact />)}</div>
      )}
    </section>
  );
}

function RecentSessions({ wsId, viewAll, onStart }: { wsId: string; viewAll: string; onStart(): void }) {
  const q = useSessions(wsId);
  const items = (q.data ?? []).slice(0, 6);
  return (
    <section className="min-w-0" aria-label="Recent sessions">
      <GroupHeader>
        <h2 className="font-medium text-ink">Recent sessions</h2>
        <span aria-hidden>·</span>
        <span className="tabular-nums">{items.length}</span>
        <span className="truncate max-md:hidden">· Your latest copilot sessions.</span>
        <Link to={viewAll} className="ml-auto shrink-0 text-body-sm">View all</Link>
      </GroupHeader>
      {q.isError ? (
        <PageBody><Alert tone="danger" title="Could not load sessions">{errMsg(q.error)}</Alert></PageBody>
      ) : q.isLoading ? (
        <div className="flex flex-col" aria-hidden><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /></div>
      ) : items.length === 0 ? (
        <PageBody>
          <EmptyState icon={<Sparkle size={20} weight="duotone" />} title="No sessions yet" description="Start a copilot session to get a live transcript, suggestions, follow-ups and minutes for any meeting." action={<Button variant="primary" onClick={onStart}>Start copilot session</Button>} />
        </PageBody>
      ) : (
        <div role="list" aria-label="Recent sessions list">{items.map((s) => <SessionRow key={s.id} s={s} />)}</div>
      )}
    </section>
  );
}

/** Workspace dashboard: what is live, what is coming up, what just happened, and the quick actions. */
export function HomePage() {
  const { current, loading, wsPath } = useWorkspace();
  const profile = useProfile();
  const nav = useNavigate();
  const { features } = useFeatures();
  const rooms = useRoomList(features.translationMeetings ? current?.id : undefined);
  const all = useMemo(() => rooms.data?.pages.flatMap((p) => p.items) ?? [], [rooms.data]);
  const live = useMemo(() => all.filter((r) => RUNNING_STATUSES.includes(r.status)).slice(0, 6), [all]);
  const waiting = useMemo(() => all.filter((r) => WAITING_STATUSES.includes(r.status)).slice(0, 6), [all]);
  const upcoming = useMemo(
    () => all.filter((r) => UPCOMING_STATUSES.includes(r.status)).sort((a, b) => Date.parse(roomMoment(a) ?? '') - Date.parse(roomMoment(b) ?? '')).slice(0, 5),
    [all],
  );
  const recent = useMemo(() => all.filter((r) => TERMINAL_STATUSES.includes(r.status)).slice(0, 5), [all]);

  if (loading) return <PageBody><Skeleton className="h-40" /></PageBody>;
  if (!current) {
    return <Navigate to="/workspaces" replace />;
  }
  const first = profile.data?.fullName?.split(/\s+/)[0];
  return (
    <>
      <PageHeader size="page" title={first ? `Welcome back, ${first}` : 'Welcome back'} description={`Your meeting copilot for ${current.name}: transcripts, suggestions, follow-ups and minutes.`} />
      <PendingInvitations className="px-4 pt-4" />
      <section aria-label="Quick actions" className="px-4 py-4">
        <SectionLabel as="p">Quick jumps</SectionLabel>
        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
          {features.copilot && <Quick primary to={`${wsPath('/sessions')}?new=1`} icon={<Sparkle size={18} weight="bold" />} title="Start copilot session" />}
          {features.copilot && <Quick to={wsPath('/sessions')} icon={<SquaresFour size={18} weight="duotone" />} title="Recent sessions" />}
          {features.copilot && <Quick to={`${wsPath('/sessions')}?tab=actions`} icon={<CheckCircle size={18} weight="duotone" />} title="Open action items" />}
          <Quick to={wsPath('/knowledge')} icon={<Brain size={18} weight="duotone" />} title="Search knowledge" />
          {features.translationMeetings && <Quick to={`${wsPath('/meetings')}?new=1`} icon={<Plus size={18} weight="bold" />} title="New meeting" />}
          {features.translationMeetings && <Quick to={`${wsPath('/meetings')}?join=1`} icon={<Keyboard size={18} weight="duotone" />} title="Join by code" />}
          {canAdmin(current) && <Quick to={`${wsPath('/members')}?invite=1`} icon={<UserPlus size={18} weight="duotone" />} title="Invite people" />}
        </div>
      </section>
      {features.copilot && <RecentSessions wsId={current.id} viewAll={wsPath('/sessions')} onStart={() => nav(`${wsPath('/sessions')}?new=1`)} />}
      {!features.translationMeetings ? null : rooms.isError ? (
        <PageBody><Alert tone="danger" title="Could not load meetings">{errMsg(rooms.error)}</Alert></PageBody>
      ) : rooms.isLoading ? (
        <div className="flex flex-col" aria-hidden><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /></div>
      ) : all.length === 0 ? (
        <PageBody><EmptyState icon={<SquaresFour size={20} weight="duotone" />} title="No meetings yet" description="Create your first meeting and share the code." action={<Button variant="primary" onClick={() => nav(`${wsPath('/meetings')}?new=1`)}>Create meeting</Button>} /></PageBody>
      ) : (
        <div className="flex min-w-0 flex-col border-t border-row">
          <RoomsBand title="Live now" description="Meetings you can join right now." items={live} emptyTitle="Nothing live" emptyHint="Start a meeting and it shows up here." viewAll={wsPath('/meetings')} showOpen />
          {waiting.length > 0 && <RoomsBand title="Waiting to start" description="Open rooms where translation is not running yet." items={waiting} emptyTitle="" emptyHint="" viewAll={wsPath('/meetings')} showOpen />}
          <RoomsBand title="Upcoming" description="Scheduled meetings." items={upcoming} emptyTitle="Nothing scheduled" emptyHint="Scheduled meetings appear here in time order." viewAll={`${wsPath('/meetings')}?status=SCHEDULED`} />
          <RoomsBand title="Recent history" description="The latest finished meetings." items={recent} emptyTitle="No past meetings yet" emptyHint="Ended and cancelled meetings are kept in History." viewAll={`${wsPath('/meetings')}?tab=history`} />
        </div>
      )}
    </>
  );
}
