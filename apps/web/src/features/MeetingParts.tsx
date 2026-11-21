import { useEffect, useRef } from 'react';
import { ClosedCaptioning } from '@phosphor-icons/react';
import { Badge, Card, CardHeader } from '@/ui';
import { cn } from '@/lib/cn';
import { languageLabel } from '@/lib/languages';
import { speakerColor } from './meeting/speakers';
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
export function StatusStrip({ connection, quality, room }: { connection: ConnectionLabel; quality: QualityLabel; room: Pick<Room, 'status' | 'translationActive'> }) {
  const c = CONNECTION[connection];
  const q = QUALITY[quality];
  const t = translationState(room);
  const items: { id: string; name: string; v: { label: string; tone: Tone } }[] = [
    { id: 'connection', name: 'Connection', v: c },
    { id: 'quality', name: 'Network', v: connection === 'idle' ? { label: 'n/a', tone: 'neutral' } : q },
    { id: 'translation', name: 'Translation', v: t },
  ];
  return (
    <ul aria-label="Meeting status" data-testid="status-strip" className="flex flex-wrap items-center gap-2">
      {items.map((i) => (
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

/** Scrolling list of caption lines (original + translation, speaker colour, delivery-latency badge). */
export function CaptionList({ lines, placeholder, translationLang, className }: { lines: CaptionLine[]; placeholder?: string; translationLang?: string; className?: string }) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => { end.current?.scrollIntoView?.({ block: 'end' }); }, [lines]);
  const wantsTranslation = !!translationLang && translationLang !== 'original';
  return (
    <div role="log" aria-live="polite" aria-label="Live captions" data-testid="captions-log" className={cn('space-y-4 overflow-y-auto p-4 text-body', className)}>
      {lines.length === 0 && (
        <p className="flex items-start gap-2 text-ink-subtle"><ClosedCaptioning className="mt-0.5 size-4 shrink-0" aria-hidden />{placeholder ?? 'Captions will appear here when someone speaks.'}</p>
      )}
      {lines.map((l) => {
        const tr = pickTranslation(l, translationLang);
        return (
          <div key={l.id} data-testid="caption-line" className={cn('flex flex-col gap-1 border-l-2 pl-3', !l.final && 'opacity-70')} style={{ borderColor: speakerColor(l.speaker) }}>
            <div className="flex flex-wrap items-center gap-2 text-caption">
              <span className="font-medium text-ink">{l.speaker}</span>
              {l.lang && <Badge>{l.lang}</Badge>}
              {l.latencyMs !== undefined && <span data-testid="caption-latency" title="Time from the pipeline to this screen" className="rounded-pill bg-surface-2 px-1.5 text-ink-subtle">{Math.round(l.latencyMs)} ms</span>}
            </div>
            {l.text && <p data-testid="caption-original" className="text-body-sm text-ink-subtle">{l.text}</p>}
            {tr ? (
              <p data-testid="caption-translation" className="text-body-lg text-ink">{tr}</p>
            ) : wantsTranslation && l.final && l.text ? (
              <p className="text-body-sm italic text-ink-tertiary">Translating to {languageLabel(translationLang)}…</p>
            ) : null}
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
