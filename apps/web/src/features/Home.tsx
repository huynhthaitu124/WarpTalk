import { useMemo, type ReactNode } from 'react';
import { Link, Navigate, useNavigate } from 'react-router-dom';
import { Brain, Keyboard, Plus, SquaresFour, UserPlus } from '@phosphor-icons/react';
import { Alert, Button, Card, CardHeader, EmptyState, PageHeader, Skeleton } from '@/ui';
import { useProfile } from '@/lib/auth';
import { canAdmin, useWorkspace } from '@/lib/workspace';
import { useRoomList } from '@/lib/queries';
import type { Room } from '@/lib/schemas';
import { PendingInvitations, RUNNING_STATUSES, RoomRow, TERMINAL_STATUSES, UPCOMING_STATUSES, WAITING_STATUSES, roomMoment } from './common';
import { errMsg } from './errors';

function Quick({ to, icon, title, hint, primary }: { to: string; icon: ReactNode; title: string; hint: string; primary?: boolean }) {
  return (
    <Link
      to={to}
      className="group flex items-center gap-3 rounded-lg border border-hairline bg-surface-1 p-4 text-ink no-underline edge-top transition-colors hover:border-control hover:bg-surface-2 hover:no-underline focus-ring"
    >
      <span className={`grid size-9 shrink-0 place-items-center rounded-md ${primary ? 'bg-primary text-on-primary' : 'bg-surface-3 text-ink-muted'}`} aria-hidden>{icon}</span>
      <span className="min-w-0">
        <span className="block truncate text-body font-medium">{title}</span>
        <span className="block truncate text-caption text-ink-subtle">{hint}</span>
      </span>
    </Link>
  );
}

function RoomsCard({ title, description, items, emptyTitle, emptyHint, viewAll, showOpen }: {
  title: string; description: string; items: Room[]; emptyTitle: string; emptyHint: string; viewAll: string; showOpen?: boolean;
}) {
  return (
    <Card className="min-w-0">
      <CardHeader title={title} description={description} actions={<Link to={viewAll} className="text-body-sm">View all</Link>} />
      {items.length === 0 ? (
        <div className="px-5 py-8 text-center">
          <p className="text-body font-medium text-ink">{emptyTitle}</p>
          <p className="mt-1 text-body-sm text-ink-subtle">{emptyHint}</p>
        </div>
      ) : (
        <div role="list" aria-label={title}>{items.map((r) => <RoomRow key={r.id} room={r} showOpen={showOpen} />)}</div>
      )}
    </Card>
  );
}

/** Workspace dashboard: what is live, what is coming up, what just happened, and the quick actions. */
export function HomePage() {
  const { current, loading, wsPath } = useWorkspace();
  const profile = useProfile();
  const nav = useNavigate();
  const rooms = useRoomList(current?.id);
  const all = useMemo(() => rooms.data?.pages.flatMap((p) => p.items) ?? [], [rooms.data]);
  const live = useMemo(() => all.filter((r) => RUNNING_STATUSES.includes(r.status)).slice(0, 6), [all]);
  const waiting = useMemo(() => all.filter((r) => WAITING_STATUSES.includes(r.status)).slice(0, 6), [all]);
  const upcoming = useMemo(
    () => all.filter((r) => UPCOMING_STATUSES.includes(r.status)).sort((a, b) => Date.parse(roomMoment(a) ?? '') - Date.parse(roomMoment(b) ?? '')).slice(0, 5),
    [all],
  );
  const recent = useMemo(() => all.filter((r) => TERMINAL_STATUSES.includes(r.status)).slice(0, 5), [all]);

  if (loading) return <Skeleton className="h-40" />;
  if (!current) {
    return <Navigate to="/workspaces" replace />;
  }
  const first = profile.data?.fullName?.split(/\s+/)[0];
  return (
    <>
      <PageHeader title={first ? `Welcome back, ${first}` : 'Welcome back'} description={`Here is what is happening in ${current.name}.`} />
      <PendingInvitations className="mb-6" />
      <section aria-label="Quick actions" className="mb-6 grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
        <Quick primary to={`${wsPath('/meetings')}?new=1`} icon={<Plus size={18} weight="bold" />} title="New meeting" hint="Start a translated room" />
        <Quick to={`${wsPath('/meetings')}?join=1`} icon={<Keyboard size={18} weight="duotone" />} title="Join by code" hint="Enter a 12-character code" />
        <Quick to={wsPath('/knowledge')} icon={<Brain size={18} weight="duotone" />} title="Search knowledge" hint="Documents and past meetings" />
        {canAdmin(current)
          ? <Quick to={`${wsPath('/members')}?invite=1`} icon={<UserPlus size={18} weight="duotone" />} title="Invite people" hint="Add teammates by email" />
          : <Quick to={wsPath('/meetings')} icon={<SquaresFour size={18} weight="duotone" />} title="All meetings" hint="Active, history and search" />}
      </section>
      {rooms.isError ? (
        <Alert tone="danger" title="Could not load meetings">{errMsg(rooms.error)}</Alert>
      ) : rooms.isLoading ? (
        <div className="grid gap-6 lg:grid-cols-2"><Skeleton className="h-48" /><Skeleton className="h-48" /></div>
      ) : all.length === 0 ? (
        <EmptyState icon={<SquaresFour size={20} weight="duotone" />} title="No meetings yet" description="Create your first meeting, share the code, and start translating." action={<Button variant="primary" onClick={() => nav(`${wsPath('/meetings')}?new=1`)}>Create meeting</Button>} />
      ) : (
        <div className="grid min-w-0 gap-6 lg:grid-cols-2">
          <div className="flex min-w-0 flex-col gap-6">
            <RoomsCard title="Live now" description="Meetings you can join right now." items={live} emptyTitle="Nothing live" emptyHint="Start a meeting and it shows up here." viewAll={wsPath('/meetings')} showOpen />
            {waiting.length > 0 && <RoomsCard title="Waiting to start" description="Open rooms where translation is not running yet." items={waiting} emptyTitle="" emptyHint="" viewAll={wsPath('/meetings')} showOpen />}
            <RoomsCard title="Upcoming" description="Scheduled meetings." items={upcoming} emptyTitle="Nothing scheduled" emptyHint="Scheduled meetings appear here in time order." viewAll={`${wsPath('/meetings')}?status=SCHEDULED`} />
          </div>
          <RoomsCard title="Recent history" description="The latest finished meetings." items={recent} emptyTitle="No past meetings yet" emptyHint="Ended and cancelled meetings are kept in History." viewAll={`${wsPath('/meetings')}?tab=history`} />
        </div>
      )}
    </>
  );
}
