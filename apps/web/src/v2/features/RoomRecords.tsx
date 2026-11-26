import type { ReactNode } from 'react';
import * as RT from '@radix-ui/react-tabs';
import { ClosedCaptioning, FileText, Paperclip } from '@phosphor-icons/react';
import { Skeleton } from '@v2/ui';
import { cn } from '@v2/lib/cn';
import { useTranscript, type Transcript } from '@v2/lib/queries';
import { errMsg } from './errors';

const TRIGGER =
  'relative inline-flex h-9 shrink-0 items-center gap-1.5 whitespace-nowrap px-0.5 text-body font-medium text-ink-subtle transition-colors hover:text-ink focus-ring ' +
  'data-[state=active]:text-ink after:absolute after:inset-x-0 after:-bottom-px after:h-0.5 after:rounded-full after:bg-transparent data-[state=active]:after:bg-ink';

function Count({ n }: { n: number }) {
  return <span className="grid h-4 min-w-4 place-items-center rounded-full bg-surface-2 px-1 text-[10px] tabular-nums text-ink-muted">{n}</span>;
}

function Empty({ icon, title, children }: { icon: ReactNode; title: string; children: ReactNode }) {
  return (
    <div className="flex flex-col items-center gap-2 py-12 text-center">
      <span aria-hidden className="text-ink-tertiary">{icon}</span>
      <p className="text-body font-medium text-ink">{title}</p>
      <p className="max-w-[320px] text-body-sm text-ink-subtle">{children}</p>
    </div>
  );
}

function clock(ms?: number | null) {
  if (ms == null) return null;
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const mm = String(Math.floor((s % 3600) / 60)).padStart(2, '0');
  const ss = String(s % 60).padStart(2, '0');
  return h ? `${h}:${mm}:${ss}` : `${mm}:${ss}`;
}

function TranscriptView({ t }: { t: Transcript }) {
  const segs = [...t.segments].sort((a, b) => a.sequenceOrder - b.sequenceOrder);
  return (
    <ol className="flex flex-col" data-testid="room-transcript">
      {segs.map((s, i) => {
        const run = i === 0 || segs[i - 1]?.speakerName !== s.speakerName;
        return (
          <li key={s.id} className={cn('grid grid-cols-[56px_1fr] gap-3', run ? 'pt-4' : 'pt-1')}>
            <span className="pt-0.5 font-mono text-[12px] tabular-nums text-ink-tertiary">{run ? clock(s.startTimeMs) : ''}</span>
            <div className="min-w-0">
              {run && <p className="mb-0.5 text-body-sm font-semibold text-ink">{s.speakerName}</p>}
              <p className="break-words text-body text-ink">{s.originalText}</p>
              {s.translation?.text && <p className="break-words text-body text-ink-muted">{s.translation.text}</p>}
            </div>
          </li>
        );
      })}
    </ol>
  );
}

/** Bottom section of the meeting page: transcript, minutes and attachments. */
export function RoomRecords({ roomId, lang, live, ended }: { roomId: string; lang?: string; live: boolean; ended: boolean }) {
  const tq = useTranscript(roomId, lang, live);
  const segCount = tq.data?.segments.length ?? 0;
  return (
    <RT.Root defaultValue="transcript" className="flex flex-col" data-testid="room-records">
      <RT.List aria-label="Meeting records" className="flex gap-5 overflow-x-auto border-b border-hairline [scrollbar-width:none]">
        <RT.Trigger value="transcript" className={TRIGGER}>Transcript{segCount > 0 && <Count n={segCount} />}</RT.Trigger>
        <RT.Trigger value="minutes" className={TRIGGER}>Minutes</RT.Trigger>
        <RT.Trigger value="attachments" className={TRIGGER}>Attachments</RT.Trigger>
      </RT.List>
      <RT.Content value="transcript" className="focus-ring">
        {tq.isLoading ? (
          <div className="flex flex-col gap-2 pt-4"><Skeleton className="h-5 w-48" /><Skeleton className="h-5 w-full" /><Skeleton className="h-5 w-3/4" /></div>
        ) : tq.isError ? (
          <p role="alert" className="pt-4 text-body-sm text-danger-text">{errMsg(tq.error)}</p>
        ) : tq.data && tq.data.segments.length > 0 ? (
          <TranscriptView t={tq.data} />
        ) : (
          <Empty icon={<ClosedCaptioning size={32} weight="light" />} title="No transcript yet">
            {live ? 'What people say appears here while the meeting runs.' : ended ? 'Nothing was recorded in this meeting.' : 'Start the meeting to record a transcript.'}
          </Empty>
        )}
      </RT.Content>
      <RT.Content value="minutes" className="focus-ring">
        <Empty icon={<FileText size={32} weight="light" />} title="No minutes yet">Meeting minutes are written from the transcript after the meeting ends.</Empty>
      </RT.Content>
      <RT.Content value="attachments" className="focus-ring">
        <Empty icon={<Paperclip size={32} weight="light" />} title="No attachments">Files shared for this meeting will show up here.</Empty>
      </RT.Content>
    </RT.Root>
  );
}
