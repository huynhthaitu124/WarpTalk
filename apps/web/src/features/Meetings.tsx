import { useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import { CaretDown, CaretLeft, CaretRight, Keyboard, MagnifyingGlass, Plus, SquaresFour } from '@phosphor-icons/react';
import {
  Alert, Button, EmptyState, GroupHeader, Input, PageBody, PageHeader, Select, Skeleton, Tabs, TabsContent, TabsList, TabsTrigger,
} from '@/ui';
import { cn } from '@/lib/cn';
import { useWorkspace } from '@/lib/workspace';
import { useRoomList } from '@/lib/queries';
import { languageLabel } from '@/lib/languages';
import type { Room } from '@/lib/schemas';
import { CreateWorkspaceDialog } from './Workspaces';
import { CreateRoomDialog } from './CreateRoom';
import { JoinDialog } from './Join';
import { LIVE_STATUSES, RoomRow, TERMINAL_STATUSES, UPCOMING_STATUSES, roomMoment } from './common';
import { errMsg } from './errors';

type Tab = 'active' | 'history' | 'all';
const TABS: { value: Tab; label: string; statuses: string[] | null }[] = [
  { value: 'active', label: 'Active', statuses: [...LIVE_STATUSES, ...UPCOMING_STATUSES] },
  { value: 'history', label: 'History', statuses: TERMINAL_STATUSES },
  { value: 'all', label: 'All', statuses: null },
];
const label = (s: string) => s.replace('_', ' ').toLowerCase().replace(/^./, (c) => c.toUpperCase());

function matches(r: Room, q: string) {
  const n = q.trim().toLowerCase();
  if (!n) return true;
  return [r.title, r.roomCode, languageLabel(r.sourceLanguage), ...r.targetLanguages.map(languageLabel)].some((v) => v.toLowerCase().includes(n));
}

const dayKey = (iso: string | null | undefined) => {
  if (!iso) return '';
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? '' : `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
};
const keyOf = (d: Date) => dayKey(d.toISOString());
const fromKey = (k: string) => { const [y, m, d] = k.split('-').map(Number); return new Date(y!, m! - 1, d); };
const dayLabel = (k: string) => new Intl.DateTimeFormat(undefined, { weekday: 'short', month: 'short', day: 'numeric' }).format(fromKey(k));

/** Week strip (Mon to Sun): a dot marks days that have meetings; picking a day filters the list. */
function DayStrip({ rooms, day, onDay }: { rooms: Room[]; day: string | null; onDay(d: string | null): void }) {
  const [offset, setOffset] = useState(0);
  const today = new Date();
  const monday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - ((today.getDay() + 6) % 7) + offset * 7);
  const days = Array.from({ length: 7 }, (_, i) => new Date(monday.getFullYear(), monday.getMonth(), monday.getDate() + i));
  const withRooms = new Set(rooms.map((r) => dayKey(roomMoment(r))));
  const todayKey = dayKey(new Date().toISOString());
  const fmt = new Intl.DateTimeFormat(undefined, { weekday: 'short' });
  const arrow = 'grid size-6 place-items-center rounded-pill text-ink-muted hover:bg-surface-2 hover:text-ink focus-ring';
  return (
    <div className="flex items-center gap-2 border-b border-row px-4 py-2" role="group" aria-label="Day filter">
      <button type="button" className={arrow} aria-label="Previous week" onClick={() => setOffset(offset - 1)}><CaretLeft size={12} aria-hidden /></button>
      <div className="flex gap-1">
        {days.map((d) => {
          const k = keyOf(d);
          const sel = day === k;
          return (
            <button
              key={k}
              type="button"
              aria-pressed={sel}
              onClick={() => onDay(sel ? null : k)}
              className={cn('flex w-11 flex-col items-center gap-0.5 rounded-xl py-1 focus-ring', sel ? 'bg-surface-2' : 'hover:bg-surface-2/60')}
            >
              <span className="sr-only">Show meetings on </span>
              <span className="text-[10px] font-medium uppercase text-ink-subtle">{fmt.format(d)}</span>
              <span className={cn('grid size-6 place-items-center rounded-pill text-body-sm tabular-nums', sel ? 'bg-primary font-semibold text-on-primary' : k === todayKey ? 'font-semibold text-chip-primary-text' : 'text-ink')}>{d.getDate()}</span>
              <span aria-hidden className={cn('size-1 rounded-pill', withRooms.has(k) ? 'bg-primary' : 'bg-transparent')} />
            </button>
          );
        })}
      </div>
      <button type="button" className={arrow} aria-label="Next week" onClick={() => setOffset(offset + 1)}><CaretRight size={12} aria-hidden /></button>
      {day && <button type="button" className="ml-1 text-body-sm text-link hover:underline focus-ring" onClick={() => onDay(null)}>Clear day</button>}
    </div>
  );
}

/** One list for every room of the workspace: Active / History / All, search and status filter all live in the URL. */
export function MeetingsPage() {
  const { current, loading } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const tab = (TABS.find((t) => t.value === params.get('tab'))?.value ?? 'active') as Tab;
  const q = params.get('q') ?? '';
  // Typing is local (fast keystrokes must not race the URL); the URL follows.
  const [typed, setTyped] = useState(q);
  const status = params.get('status') ?? 'all';
  const creating = params.get('new') === '1';
  const joining = params.get('join') === '1';
  const tabDef = TABS.find((t) => t.value === tab)!;

  const patch = (next: Record<string, string | null>) => {
    const p = new URLSearchParams(params);
    for (const [k, v] of Object.entries(next)) { if (v === null || v === '') p.delete(k); else p.set(k, v); }
    setParams(p, { replace: true });
  };

  const rooms = useRoomList(current?.id, status === 'all' ? undefined : status);
  const all = useMemo(() => rooms.data?.pages.flatMap((p) => p.items) ?? [], [rooms.data]);
  const total = rooms.data?.pages[0]?.total ?? 0;
  const inTab = useMemo(() => all.filter((r) => !tabDef.statuses || tabDef.statuses.includes(r.status)), [all, tabDef]);
  const items = useMemo(() => inTab.filter((r) => matches(r, q)), [inTab, q]);
  const [day, setDay] = useState<string | null>(null);
  const shown = useMemo(() => (day ? items.filter((r) => dayKey(roomMoment(r)) === day) : items), [items, day]);

  const statusOptions = (tabDef.statuses ?? [...LIVE_STATUSES, ...UPCOMING_STATUSES, ...TERMINAL_STATUSES]).map((s) => ({ value: s, label: label(s) }));
  const filtered = !!q.trim() || status !== 'all';

  if (loading) return <Skeleton className="h-40" />;
  if (!current) {
    return (
      <>
        <PageHeader title="Meetings" />
        <EmptyState icon={<SquaresFour size={20} weight="duotone" />} title="Create a workspace first" description="Meetings live inside a workspace." action={<Button variant="primary" onClick={() => patch({ new: '1' })}>Create workspace</Button>} />
        <CreateWorkspaceDialog open={creating} onOpenChange={(v) => patch({ new: v ? '1' : null })} />
      </>
    );
  }

  let empty: { title: string; description: string; action?: React.ReactNode } | null = null;
  if (shown.length === 0 && !rooms.isLoading) {
    const clear = <Button onClick={() => { setTyped(''); patch({ q: null, status: null }); }}>Clear filter{q.trim() && status !== 'all' ? 's' : ''}</Button>;
    if (day && items.length > 0) {
      empty = { title: 'No meetings that day', description: 'Pick another day or clear the day filter.', action: <Button onClick={() => setDay(null)}>Clear day</Button> };
    } else if (all.length === 0 && !filtered) {
      empty = { title: 'No meetings yet', description: 'Create a meeting, share the code, and start translating.', action: <Button variant="primary" onClick={() => patch({ new: '1' })}>Create meeting</Button> };
    } else if (q.trim()) {
      empty = { title: `No meetings match “${q.trim()}”`, description: 'Try a different title, language or code.', action: clear };
    } else if (status !== 'all') {
      empty = { title: `No ${label(status).toLowerCase()} meetings`, description: 'Nothing in this view has that status.', action: clear };
    } else if (tab === 'active') {
      empty = { title: 'No active meetings', description: 'Live and upcoming meetings show up here. Past ones are in History.', action: <Button onClick={() => patch({ tab: 'history' })}>View history</Button> };
    } else {
      empty = { title: 'No past meetings yet', description: 'Meetings that ended or were cancelled are kept here.' };
    }
  }

  return (
    <>
      <PageHeader
        title="Meetings"
        description={`Translation meetings in ${current.name}.`}
        actions={
          <>
            <Button onClick={() => patch({ join: '1' })}><Keyboard size={16} aria-hidden />Join by code</Button>
            <Button variant="primary" onClick={() => patch({ new: '1' })}><Plus size={16} weight="bold" aria-hidden />New meeting</Button>
          </>
        }
      />
      <Tabs value={tab} onValueChange={(t) => { setDay(null); patch({ tab: t === 'active' ? null : t, status: null }); }}>
        <div className="flex flex-wrap items-center gap-3 border-b border-row px-4 py-2">
          <TabsList aria-label="Meeting views">
            {TABS.map((t) => <TabsTrigger key={t.value} value={t.value}>{t.label}</TabsTrigger>)}
          </TabsList>
          <div className="ml-auto flex min-w-0 flex-1 basis-72 items-center justify-end gap-2">
            <div className="relative min-w-0 max-w-72 flex-1">
              <MagnifyingGlass size={14} aria-hidden className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-subtle" />
              <Input type="search" aria-label="Search meetings" placeholder="Search by title, code or language" className="h-8 rounded-pill pl-8 text-body-sm" value={typed} onChange={(e) => { setTyped(e.target.value); patch({ q: e.target.value }); }} />
            </div>
            <Select aria-label="Filter by status" className="h-8 w-36 rounded-pill max-sm:w-full" value={status} onValueChange={(v) => patch({ status: v === 'all' ? null : v })} options={[{ value: 'all', label: 'All statuses' }, ...statusOptions]} />
          </div>
        </div>
        <DayStrip rooms={inTab} day={day} onDay={setDay} />
        <TabsContent value={tab}>
          {rooms.isLoading ? (
            <div className="flex flex-col" aria-hidden>{[0, 1, 2].map((i) => <Skeleton key={i} className="h-11 rounded-none border-b border-row" />)}</div>
          ) : rooms.isError ? (
            <PageBody><Alert tone="danger" title="Could not load meetings">{errMsg(rooms.error)}</Alert></PageBody>
          ) : empty ? (
            <PageBody><EmptyState icon={<SquaresFour size={20} weight="duotone" />} title={empty.title} description={empty.description} action={empty.action} /></PageBody>
          ) : (
            <>
              <GroupHeader>
                <CaretDown size={12} aria-hidden />
                <span className="font-medium text-ink">{TABS.find((t) => t.value === tab)?.label} Meetings</span>
                <span aria-hidden>·</span>
                <span>{day ? dayLabel(day) : 'All dates'}</span>
                <span aria-hidden>·</span>
                <span className="tabular-nums" data-testid="meetings-count">{shown.length}</span>
              </GroupHeader>
              <div role="list" aria-label="Meetings">
                {shown.map((r) => <RoomRow key={r.id} room={r} showOpen={tab !== 'history'} />)}
              </div>
            </>
          )}
          {rooms.hasNextPage && (
            <div className="flex flex-col items-center gap-1 py-4">
              <Button loading={rooms.isFetchingNextPage} onClick={() => void rooms.fetchNextPage()}>Load more</Button>
              <span className="text-caption text-ink-subtle">Showing {all.length} of {total}</span>
            </div>
          )}
        </TabsContent>
      </Tabs>
      <CreateRoomDialog open={creating} onOpenChange={(v) => patch({ new: v ? '1' : null })} />
      <JoinDialog open={joining} onOpenChange={(v) => patch({ join: v ? '1' : null })} />
    </>
  );
}
