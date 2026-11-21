import { useEffect, useRef } from 'react';
import { Crown, Hand, Microphone, MicrophoneSlash, MonitorArrowUp } from '@phosphor-icons/react';
import { Avatar } from '@/ui';
import { cn } from '@/lib/cn';
import type { SessionParticipant, VideoSource } from './types';

export interface TileInfo {
  p: SessionParticipant;
  host: boolean;
  hand: boolean;
  /** "vi → en" chip from the room roster. */
  langs?: string;
}

function Video({ source, mirror, label }: { source: VideoSource; mirror?: boolean; label: string }) {
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return undefined;
    source.attach(el);
    return () => source.detach(el);
  }, [source]);
  return <video ref={ref} autoPlay playsInline muted aria-label={label} className={cn('absolute inset-0 size-full object-cover', mirror && '-scale-x-100')} />;
}

function Tile({ t, screen }: { t: TileInfo; screen?: boolean }) {
  const { p } = t;
  const source = screen ? p.screen : p.camOn ? p.video : undefined;
  const speaking = p.speaking && !screen;
  return (
    <li
      data-testid="participant-tile"
      data-identity={p.identity}
      data-speaking={speaking}
      data-screen={screen ? true : undefined}
      aria-label={screen ? `${p.name} screen` : p.name}
      className={cn(
        'relative flex min-h-24 items-center justify-center overflow-hidden rounded-lg border bg-surface-1 transition-shadow',
        screen ? 'size-full' : 'aspect-video',
        speaking ? 'border-primary shadow-[0_0_0_2px_var(--primary)]' : 'border-hairline',
      )}
    >
      {source ? <Video source={source} mirror={p.isLocal && !screen} label={screen ? `${p.name} screen` : `${p.name} camera`} /> : <Avatar name={p.name} size="lg" className="size-14 text-body-lg max-sm:size-10" />}
      {!screen && t.hand && (
        <span data-testid="hand-badge" role="img" aria-label="Hand raised" className="absolute left-2 top-2 flex size-7 items-center justify-center rounded-pill bg-warning text-black max-sm:size-6">
          <Hand weight="fill" className="size-4" aria-hidden />
        </span>
      )}
      <div className="absolute inset-x-0 bottom-0 flex items-center gap-1.5 bg-gradient-to-t from-black/70 to-transparent px-2 pb-1.5 pt-6 text-white">
        {screen ? <MonitorArrowUp className="size-4 shrink-0" aria-hidden /> : p.micOn ? <Microphone weight="fill" className="size-4 shrink-0" aria-hidden /> : <MicrophoneSlash className="size-4 shrink-0" aria-hidden />}
        <span className="min-w-0 truncate text-body-sm font-medium max-sm:text-caption">{screen ? `${p.name}'s screen` : p.name}{p.isLocal && !screen ? ' (you)' : ''}</span>
        {!screen && t.host && <Crown weight="fill" className="size-3.5 shrink-0" aria-label="Host" role="img" />}
        <span className="sr-only">{screen ? '' : `${p.micOn ? 'Microphone on' : 'Microphone off'}${speaking ? ', speaking' : ''}`}</span>
        {t.langs && !screen && <span className="ml-auto hidden shrink-0 rounded-pill bg-black/50 px-1.5 text-caption sm:inline">{t.langs}</span>}
      </div>
    </li>
  );
}

/** Participant grid. A screen share takes the main area; everybody else moves to a strip. */
export function Stage({ tiles }: { tiles: TileInfo[] }) {
  const sharer = tiles.find((t) => t.p.sharing && t.p.screen);
  const n = tiles.length;
  if (sharer) {
    return (
      <div className="flex h-full min-h-0 flex-col gap-3 lg:flex-row" data-testid="participant-grid" data-layout="spotlight">
        <ul aria-label="Shared screen" className="min-h-48 flex-1"><Tile t={sharer} screen /></ul>
        <ul aria-label="Participants" className="grid shrink-0 grid-cols-3 gap-2 max-lg:grid-flow-col max-lg:auto-cols-[8rem] max-lg:grid-cols-none max-lg:overflow-x-auto lg:w-56 lg:grid-cols-1 lg:overflow-y-auto">
          {tiles.map((t) => <Tile key={t.p.identity} t={t} />)}
        </ul>
      </div>
    );
  }
  return (
    <ul
      aria-label="Participants"
      data-testid="participant-grid"
      data-layout="grid"
      className={cn(
        'mx-auto grid w-full content-center gap-2 sm:gap-3',
        n <= 1 ? 'max-w-3xl grid-cols-1' : n <= 4 ? 'grid-cols-2' : 'grid-cols-2 sm:grid-cols-3 xl:grid-cols-4',
        n > 1 && n <= 4 && 'max-w-5xl',
      )}
    >
      {tiles.map((t) => <Tile key={t.p.identity} t={t} />)}
    </ul>
  );
}
