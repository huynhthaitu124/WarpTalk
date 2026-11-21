import { useMemo } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Keyboard, MagnifyingGlass, Plus, SquaresFour } from '@phosphor-icons/react';
import {
  Alert, Button, EmptyState, Input, PageHeader, Select, Skeleton, Tabs, TabsContent, TabsList, TabsTrigger,
} from '@/ui';
import { useWorkspace } from '@/lib/workspace';
import { useRoomList } from '@/lib/queries';
import { languageLabel } from '@/lib/languages';
import type { Room } from '@/lib/schemas';
import { CreateWorkspaceDialog } from './Workspaces';
import { CreateRoomDialog } from './CreateRoom';
import { JoinDialog } from './Join';
import { LIVE_STATUSES, RoomRow, TERMINAL_STATUSES, UPCOMING_STATUSES } from './common';
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

/** One list for every room of the workspace: Active / History / All, search and status filter all live in the URL. */
export function MeetingsPage() {
  const { current, loading } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const tab = (TABS.find((t) => t.value === params.get('tab'))?.value ?? 'active') as Tab;
  const q = params.get('q') ?? '';
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
  if (items.length === 0 && !rooms.isLoading) {
    const clear = <Button onClick={() => patch({ q: null, status: null })}>Clear filter{q.trim() && status !== 'all' ? 's' : ''}</Button>;
    if (all.length === 0 && !filtered) {
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
      <Tabs value={tab} onValueChange={(t) => patch({ tab: t === 'active' ? null : t, status: null })}>
        <TabsList aria-label="Meeting views">
          {TABS.map((t) => <TabsTrigger key={t.value} value={t.value}>{t.label}</TabsTrigger>)}
        </TabsList>
        <div className="mt-4 flex flex-wrap items-center gap-2">
          <div className="relative min-w-0 flex-1 basis-56">
            <MagnifyingGlass size={16} aria-hidden className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-subtle" />
            <Input type="search" aria-label="Search meetings" placeholder="Search by title, code or language" className="pl-8" value={q} onChange={(e) => patch({ q: e.target.value })} />
          </div>
          <Select aria-label="Filter by status" className="w-40 max-sm:w-full" value={status} onValueChange={(v) => patch({ status: v === 'all' ? null : v })} options={[{ value: 'all', label: 'All statuses' }, ...statusOptions]} />
        </div>
        <TabsContent value={tab} className="mt-4">
          {rooms.isLoading ? (
            <div className="flex flex-col gap-2"><Skeleton className="h-14" /><Skeleton className="h-14" /><Skeleton className="h-14" /></div>
          ) : rooms.isError ? (
            <Alert tone="danger" title="Could not load meetings">{errMsg(rooms.error)}</Alert>
          ) : empty ? (
            <EmptyState icon={<SquaresFour size={20} weight="duotone" />} title={empty.title} description={empty.description} action={empty.action} />
          ) : (
            <div className="overflow-hidden rounded-lg border border-hairline bg-surface-1 edge-top" role="list" aria-label="Meetings">
              {items.map((r) => <RoomRow key={r.id} room={r} showOpen={tab !== 'history'} />)}
            </div>
          )}
          {rooms.hasNextPage && (
            <div className="mt-4 flex flex-col items-center gap-1">
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
