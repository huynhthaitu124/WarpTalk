import { useEffect, useRef } from 'react';
import { Mic, MicOff, PhoneOff, Captions } from 'lucide-react';
import { Avatar, Badge, Button, Card, CardHeader, Select } from '@/ui';
import { cn } from '@/lib/cn';
import { LANGUAGES, languageLabel } from '@/lib/languages';
import { pickTranslation, type CaptionLine } from '@/lib/captions';
import type { Room } from '@/lib/schemas';

export type { CaptionLine };
export interface Tile { id: string; name: string; speaking?: boolean; micOn?: boolean; isLocal?: boolean; note?: string }

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

export function ParticipantGrid({ tiles, empty }: { tiles: Tile[]; empty?: string }) {
  if (tiles.length === 0) return <p className="rounded-lg border border-dashed border-hairline-strong p-8 text-center text-body text-ink-subtle">{empty ?? 'No participants yet.'}</p>;
  return (
    <ul aria-label="Participants" data-testid="participant-grid" className="grid grid-cols-[repeat(auto-fill,minmax(180px,1fr))] gap-3">
      {tiles.map((t) => (
        <li
          key={t.id}
          className={cn('relative flex aspect-video flex-col items-center justify-center gap-2 rounded-lg border bg-surface-1 p-3', t.speaking ? 'border-primary shadow-[0_0_0_1px_var(--primary)]' : 'border-hairline')}
        >
          <Avatar name={t.name} size="lg" />
          <span className="max-w-full truncate text-body font-medium">{t.name}{t.isLocal ? ' (you)' : ''}</span>
          {t.note && <Badge>{t.note}</Badge>}
          {t.micOn !== undefined && (
            <span className="absolute right-2 top-2 text-ink-subtle" role="img" aria-label={t.micOn ? 'Microphone on' : 'Microphone off'}>
              {t.micOn ? <Mic className="size-4 text-success" aria-hidden /> : <MicOff className="size-4" aria-hidden />}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

export function CaptionsPanel({
  lines, placeholder, translationLang, title = 'Captions', description = 'Original and translation, with delivery latency',
}: { lines: CaptionLine[]; placeholder?: string; translationLang?: string; title?: string; description?: string }) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => { end.current?.scrollIntoView?.({ block: 'end' }); }, [lines]);
  const wantsTranslation = !!translationLang && translationLang !== 'original';
  return (
    <Card className="flex min-h-64 flex-col" aria-label={title}>
      <CardHeader title={title} description={description} />
      <div role="log" aria-live="polite" aria-label="Live captions" data-testid="captions-log" className="max-h-[28rem] flex-1 space-y-4 overflow-y-auto p-4 text-body">
        {lines.length === 0 && (
          <p className="flex items-center gap-2 text-ink-subtle"><Captions className="size-4" aria-hidden />{placeholder ?? 'Captions will appear here when someone speaks.'}</p>
        )}
        {lines.map((l) => {
          const tr = pickTranslation(l, translationLang);
          return (
            <div key={l.id} data-testid="caption-line" className={cn('flex flex-col gap-1', !l.final && 'opacity-70')}>
              <div className="flex flex-wrap items-center gap-2 text-caption">
                <span className="font-medium text-link">{l.speaker}</span>
                {l.lang && <Badge>{l.lang}</Badge>}
                {l.latencyMs !== undefined && <span data-testid="caption-latency" title="Time from the pipeline to this screen" className="text-ink-subtle">{Math.round(l.latencyMs)} ms</span>}
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
    </Card>
  );
}

export const LISTEN_OPTIONS = [{ value: 'original', label: 'Original audio' }, ...LANGUAGES.map((l) => ({ value: l.code, label: `Dubbed: ${l.label}` }))];

export function MeetingControls({
  micOn, onToggleMic, micDisabled, micHint, listen, onListen, onLeave,
}: { micOn: boolean; onToggleMic(): void; micDisabled?: boolean; micHint?: string; listen: string; onListen(v: string): void; onLeave(): void }) {
  return (
    <div role="toolbar" aria-label="Meeting controls" className="sticky bottom-4 flex flex-wrap items-center justify-center gap-3 rounded-xl border border-hairline-strong bg-surface-2 px-4 py-3 shadow-pop">
      <Button
        variant={micOn ? 'primary' : 'secondary'}
        onClick={onToggleMic}
        disabled={micDisabled}
        aria-pressed={micOn}
        title={micHint}
      >
        {micOn ? <Mic className="size-4" aria-hidden /> : <MicOff className="size-4" aria-hidden />}
        {micOn ? 'Mute' : 'Unmute'}
      </Button>
      <div className="w-52">
        <Select aria-label="Listen language" testId="listen-language" value={listen} onValueChange={onListen} options={LISTEN_OPTIONS} />
      </div>
      <Button variant="danger" onClick={onLeave} aria-label="Leave meeting"><PhoneOff className="size-4" aria-hidden />Leave</Button>
    </div>
  );
}
