import { Link } from 'react-router-dom';
import { Video } from 'lucide-react';
import { Alert, Button, EmptyState, ListRow, PageHeader, Skeleton, StatusPill } from '@/ui';
import { useWorkspace } from '@/lib/workspace';
import { useRooms } from '@/lib/queries';
import { languageLabel } from '@/lib/languages';
import { errMsg } from './errors';

export function MeetingsPage() {
  const { current } = useWorkspace();
  const rooms = useRooms(current?.id);
  const live = (rooms.data?.items ?? []).filter((r) => ['IN_PROGRESS', 'PAUSED', 'OPEN', 'WAITING'].includes(r.status));
  return (
    <>
      <PageHeader title="Meetings" description="Rooms you can join right now." />
      {!current ? (
        <EmptyState icon={<Video className="size-5" />} title="No workspace selected" description="Create or select a workspace to see its meetings." />
      ) : rooms.isLoading ? (
        <Skeleton className="h-32" />
      ) : rooms.isError ? (
        <Alert tone="danger">{errMsg(rooms.error)}</Alert>
      ) : live.length === 0 ? (
        <EmptyState icon={<Video className="size-5" />} title="No live meetings" description="Start a room from the Rooms page and it will show up here." action={<Link to="/rooms" className="text-body">Go to rooms</Link>} />
      ) : (
        <div className="overflow-hidden rounded-lg border border-hairline bg-surface-1" role="list" aria-label="Live meetings">
          {live.map((r) => (
            <ListRow key={r.id} role="listitem">
              <div className="min-w-0 flex-1">
                <p className="truncate text-body font-medium">{r.title}</p>
                <p className="text-caption text-ink-subtle">{languageLabel(r.sourceLanguage)} → {r.targetLanguages.map(languageLabel).join(', ')}</p>
              </div>
              <StatusPill status={r.status} />
              <Link to={`/meetings/${r.id}`} aria-label={`Open meeting ${r.title}`}><Button size="sm" variant="primary" tabIndex={-1}>Open</Button></Link>
            </ListRow>
          ))}
        </div>
      )}
    </>
  );
}
