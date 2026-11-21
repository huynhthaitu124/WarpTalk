import { useEffect, useRef } from 'react';
import { Mic, MicOff, PhoneOff, Captions } from 'lucide-react';
import { Avatar, Badge, Button, Card, CardHeader, Select } from '@/ui';
import { cn } from '@/lib/cn';
import { LANGUAGES } from '@/lib/languages';

export interface Tile { id: string; name: string; speaking?: boolean; micOn?: boolean; isLocal?: boolean; note?: string }
export interface CaptionLine { id: string; speaker: string; text: string; lang?: string; final: boolean }

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

export function CaptionsPanel({ lines, placeholder }: { lines: CaptionLine[]; placeholder?: string }) {
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => { end.current?.scrollIntoView?.({ block: 'end' }); }, [lines]);
  return (
    <Card className="flex min-h-64 flex-col" aria-label="Captions">
      <CardHeader title="Captions" description="Live transcript and translation" />
      <div role="log" aria-live="polite" aria-label="Live captions" data-testid="captions-log" className="flex-1 space-y-3 overflow-y-auto p-4 text-body">
        {lines.length === 0 && (
          <p className="flex items-center gap-2 text-ink-subtle"><Captions className="size-4" aria-hidden />{placeholder ?? 'Captions will appear here when someone speaks.'}</p>
        )}
        {lines.map((l) => (
          <p key={l.id} className={cn(!l.final && 'opacity-70')}>
            <span className="mr-2 font-medium text-link">{l.speaker}</span>
            {l.lang && <Badge className="mr-2">{l.lang}</Badge>}
            {l.text}
          </p>
        ))}
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
