// "My tasks": action items assigned to the signed-in person across their workspaces (`GET /me/action-items`).
import { Link, useSearchParams } from 'react-router-dom';
import { ArrowSquareOut, Check, CheckCircle, ListChecks } from '@phosphor-icons/react';
import { Alert, Badge, Button, EmptyState, PageBody, PageHeader, Skeleton, Tabs, TabsContent, TabsList, TabsTrigger, useToast } from '@v2/ui';
import { useFeatures } from '@v2/lib/features';
import { useMeActionItems, useSession, useSetMeTaskDone, type MyTask, type TaskStatus } from '@v2/lib/copilot';
import { TASKS } from '@v2/lib/strings';
import { formatWhen } from './common';
import { errMsg } from './errors';

/** Title of the meeting an item came from; the link goes to the session page (copilot) or the room page. */
function SourceLink({ roomId }: { roomId: string }) {
  const { features } = useFeatures();
  const s = useSession(roomId);
  const title = s.data?.title ?? 'Meeting';
  const to = features.copilot ? `/sessions/${roomId}` : `/rooms/${roomId}`;
  return (
    <Link to={to} data-testid="task-source" aria-label={TASKS.openMeeting(title)} className="inline-flex max-w-full items-center gap-1 text-body-sm text-ink-muted hover:text-link">
      <ArrowSquareOut size={12} aria-hidden className="shrink-0" /><span className="truncate">{TASKS.fromMeeting}: {title}</span>
    </Link>
  );
}

function TaskRow({ t, busy, onToggle }: { t: MyTask; busy: boolean; onToggle(done: boolean): void }) {
  return (
    <li data-testid="task" data-done={t.done} className="flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-row px-4 py-2.5">
      <span className="grid size-4 shrink-0 place-items-center text-ink-subtle" aria-hidden>{t.done ? <CheckCircle size={16} weight="fill" /> : <span className="size-3 rounded-pill border border-control" />}</span>
      <div className="flex min-w-0 flex-1 basis-56 flex-col gap-0.5">
        <span className={`break-words text-body ${t.done ? 'text-ink-subtle line-through' : 'text-ink'}`}>{t.text}</span>
        <span className="flex flex-wrap items-center gap-x-3 gap-y-0.5 text-body-sm text-ink-subtle">
          {t.deadline && <span>Due {t.deadline}</span>}
          {t.createdAt && <span>Added {formatWhen(t.createdAt)}</span>}
          {t.roomId && <SourceLink roomId={t.roomId} />}
        </span>
      </div>
      {t.done && <Badge tone="success" size="sm">Done</Badge>}
      <Button size="sm" variant={t.done ? 'ghost' : 'secondary'} loading={busy} aria-label={`${t.done ? 'Reopen' : 'Mark done'}: ${t.text}`} onClick={() => onToggle(!t.done)}>
        {t.done ? 'Reopen' : <><Check size={14} weight="bold" aria-hidden />Mark done</>}
      </Button>
    </li>
  );
}

export function MyTasksPage() {
  const [params, setParams] = useSearchParams();
  const status: TaskStatus = params.get('status') === 'done' ? 'done' : 'open';
  const q = useMeActionItems(status);
  const set = useSetMeTaskDone();
  const toast = useToast();
  const items = q.data ?? [];
  return (
    <>
      <PageHeader title={TASKS.title} description={TASKS.description} />
      <Tabs value={status} onValueChange={(v) => setParams(v === 'done' ? { status: 'done' } : {}, { replace: true })}>
        <TabsList className="px-4 py-3" aria-label={TASKS.filterLabel}>
          <TabsTrigger value="open">{TASKS.open}</TabsTrigger>
          <TabsTrigger value="done">{TASKS.done}</TabsTrigger>
        </TabsList>
        <TabsContent value={status} className="border-t border-row" data-testid="tasks-list" data-status={status}>
        {q.isLoading ? <div aria-hidden><Skeleton className="h-14 rounded-none" /><Skeleton className="h-14 rounded-none" /></div>
          : q.isError ? <PageBody><Alert tone="danger" title={TASKS.loadError}>{errMsg(q.error)}</Alert></PageBody>
          : items.length === 0 ? (
            <PageBody>
              <EmptyState
                icon={<ListChecks size={20} weight="duotone" />}
                title={status === 'open' ? TASKS.emptyOpen : TASKS.emptyDone}
                description={status === 'open' ? TASKS.emptyOpenHint : TASKS.emptyDoneHint}
              />
            </PageBody>
          ) : (
            <ul aria-label={`${status === 'open' ? TASKS.open : TASKS.done} tasks`}>
              {items.map((t) => (
                <TaskRow
                  key={t.id}
                  t={t}
                  busy={set.isPending && set.variables?.id === t.id}
                  onToggle={(done) => set.mutate({ id: t.id, roomId: t.roomId, done }, { onError: (e) => toast({ tone: 'danger', title: 'Could not update the task', description: errMsg(e) }) })}
                />
              ))}
            </ul>
          )}
        </TabsContent>
      </Tabs>
    </>
  );
}
