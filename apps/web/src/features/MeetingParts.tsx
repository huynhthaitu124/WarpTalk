import { useEffect, useRef } from 'react';
import { ClosedCaptioning } from '@phosphor-icons/react';
import { Badge, Card, CardHeader, ParticipantAvatar } from '@/ui';
import { cn } from '@/lib/cn';
import { languageLabel } from '@/lib/languages';
import { pickTranslation, type CaptionLine } from '@/lib/captions';
import type { Room } from '@/lib/schemas';

export type { CaptionLine };

type Tone = 'neutral' | 'success' | 'warning' | 'danger';
export type ConnectionLabel = 'idle' | 'connecting' | 'connected' | 'reconnecting' | 'disconnected';
export type QualityLabel = 'excellent' | 'good' | 'poor' | 'lost' | 'unknown';

const CONNECTION: Record<ConnectionLabel, { label: string; tone: Tone }> = {
  idle: { label: 'Not connected', tone: 'neutral' },
  connecting: { label: 'Connecting', tone: 'warning' },
  connected: { label: 'Connected', tone: 'success' },
  reconnecting: { label: 'Reconnecting', tone: 'warning' },
  disconnected: { label: 'Disconnected', tone: 'danger' },
};
const QUALITY: Record<QualityLabel, { label: string; tone: Tone }> = {
  excellent: { label: 'Excellent', tone: 'success' },
  good: { label: 'Good', tone: 'success' },
  poor: { label: 'Poor', tone: 'warning' },
  lost: { label: 'Lost', tone: 'danger' },
  unknown: { label: 'Unknown', tone: 'neutral' },
};

/** Translation state as the room reports it (NFR-US-02: translation state, connection state, network quality). */
export function translationState(r: Pick<Room, 'status' | 'translationActive'>): { label: string; tone: Tone } {
  if (['ENDED', 'CANCELLED', 'EXPIRED', 'FAILED'].includes(r.status)) return { label: 'Ended', tone: 'neutral' };
  if (r.status === 'PAUSED') return { label: 'Paused', tone: 'warning' };
  if (r.translationActive) return { label: 'Active', tone: 'success' };
  if (r.status === 'IN_PROGRESS') return { label: 'Stopped', tone: 'neutral' };
  return { label: 'Not started', tone: 'neutral' };
}

function Dot({ tone }: { tone: Tone }) {
  return <span aria-hidden className={cn('size-1.5 rounded-pill', { neutral: 'bg-ink-tertiary', success: 'bg-success', warning: 'bg-warning', danger: 'bg-danger' }[tone])} />;
}

/** Real-time status indicators; each carries a text label (colour is never the only signal). */
export function StatusStrip({ connection, quality, room, compact }: { connection: ConnectionLabel; quality: QualityLabel; room: Pick<Room, 'status' | 'translationActive'>; compact?: boolean }) {
  const c = CONNECTION[connection];
  const q = QUALITY[quality];
  const t = translationState(room);
  const items: { id: string; name: string; v: { label: string; tone: Tone } }[] = [
    { id: 'connection', name: 'Connection', v: c },
    { id: 'quality', name: 'Network', v: connection === 'idle' ? { label: 'n/a', tone: 'neutral' } : q },
    { id: 'translation', name: 'Translation', v: t },
  ];
  return (
    <ul aria-label="Meeting status" data-testid="status-strip" className={cn('flex flex-wrap items-center', compact ? 'gap-3' : 'gap-2')}>
      {items.map((i) => compact ? (
        <li key={i.id} data-testid={`status-${i.id}`} data-state={i.v.label} className="flex items-center gap-1.5 text-[12px] text-ink-muted">
          <Dot tone={i.v.tone} />
          {i.name}: {i.v.label}
        </li>
      ) : (
        <li key={i.id}>
          <Badge tone={i.v.tone} data-testid={`status-${i.id}`} data-state={i.v.label}>
            <Dot tone={i.v.tone} />
            {i.name}: {i.v.label}
          </Badge>
        </li>
      ))}
    </ul>
  );
}

/** A caption line plus the moment it first reached this screen (shown as the message time). */
export type TimedCaptionLine = CaptionLine & { receivedAt?: number };

const clock = (t?: number) => (t ? new Date(t).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) : '');

/** Scrolling list of caption lines (original + translation, speaker avatar, delivery-latency badge). */
export function CaptionList({ lines, placeholder, translationLang, className, selfUserId, showOriginal = true }: { lines: TimedCaptionLine[]; placeholder?: string; translationLang?: string; className?: string; selfUserId?: string; showOriginal?: boolean }) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => { end.current?.scrollIntoView?.({ block: 'end' }); }, [lines]);
  const wantsTranslation = !!translationLang && translationLang !== 'original';
  return (
    <div role="log" aria-live="polite" aria-label="Live captions" data-testid="captions-log" className={cn('flex flex-col gap-3 overflow-y-auto p-3', lines.length === 0 && 'justify-center', className)}>
      {lines.length === 0 && (
        <div className="flex flex-col items-center gap-2 px-4 py-10 text-center">
          <ClosedCaptioning weight="light" className="size-8 text-ink-tertiary" aria-hidden />
          <p className="max-w-[200px] text-[13px] text-ink-subtle">{placeholder ?? 'Captions will appear here when someone speaks.'}</p>
        </div>
      )}
      {lines.map((l, i) => {
        const tr = pickTranslation(l, translationLang);
        const self = !!selfUserId && l.speakerUserId === selfUserId;
        const run = i === 0 || lines[i - 1]?.speaker !== l.speaker;
        const showFooter = !!l.lang && wantsTranslation && !!tr;
        return (
          <div key={l.id} data-testid="caption-line" className={cn('flex gap-2', self && 'flex-row-reverse', !run && '-mt-2', !l.final && 'opacity-70')}>
            {run ? <ParticipantAvatar name={l.speaker} lang={l.lang} size="sm" className="mt-4" /> : <span aria-hidden className="size-7 shrink-0" />}
            <div className={cn('flex min-w-0 max-w-[85%] flex-col gap-1', self ? 'items-end' : 'items-start')}>
              {run && (
                <div className={cn('flex items-center gap-1.5 px-1 text-[11px] text-ink-muted', self && 'flex-row-reverse')}>
                  <span className="font-semibold">{self ? 'You' : l.speaker}</span>
                  {l.receivedAt && <time className="font-mono text-ink-tertiary">{clock(l.receivedAt)}</time>}
                  {l.latencyMs !== undefined && <span data-testid="caption-latency" title="Time from the pipeline to this screen" className="font-mono text-ink-tertiary">{Math.round(l.latencyMs)} ms</span>}
                </div>
              )}
              <div className={cn('rounded-2xl px-3 py-2 leading-relaxed shadow-sm', self ? 'rounded-tr-sm bg-primary text-white' : 'rounded-tl-sm border border-hairline bg-surface-2/60')}>
                {l.text && (showOriginal || !tr) && <p data-testid="caption-original" className={cn('break-words text-[13px]', self ? 'text-white/80' : 'text-ink-muted')}>{l.text}</p>}
                {tr ? (
                  <p data-testid="caption-translation" className={cn('break-words text-[13px] font-medium', self ? 'text-white' : 'text-ink')}>{tr}</p>
                ) : wantsTranslation && l.final && l.text ? (
                  <p className={cn('text-[12px] italic', self ? 'text-white/70' : 'text-ink-tertiary')}>Translating to {languageLabel(translationLang)}…</p>
                ) : null}
              </div>
              {showFooter && <p className="px-1 text-[10px] text-ink-tertiary">{languageLabel(l.lang as string)} → {languageLabel(translationLang as string)}</p>}
            </div>
          </div>
        );
      })}
      <div ref={end} />
    </div>
  );
}

export function CaptionsPanel({
  lines, placeholder, translationLang, title = 'Captions', description = 'Original and translation, with delivery latency',
}: { lines: CaptionLine[]; placeholder?: string; translationLang?: string; title?: string; description?: string }) {
  return (
    <Card className="flex min-h-64 flex-col" aria-label={title}>
      <CardHeader title={title} description={description} />
      <CaptionList lines={lines} placeholder={placeholder} translationLang={translationLang} className="max-h-[28rem] flex-1" />
    </Card>
  );
}
