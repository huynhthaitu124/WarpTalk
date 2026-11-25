import { useEffect, useRef } from 'react';
import { CellSignalFull, Hand, Microphone, MicrophoneSlash, MonitorArrowUp } from '@phosphor-icons/react';
import { ParticipantAvatar } from '@/ui';
import { cn } from '@/lib/cn';
import type { SessionParticipant, VideoSource } from './types';

export interface TileInfo {
  p: SessionParticipant;
  host: boolean;
  hand: boolean;
  /** "vi → en" chip from the room roster. */
  langs?: string;
  /** Speak language code from the room roster (language badge). */
  lang?: string;
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

function Tile({ t, screen, thumb, pip, solo }: { t: TileInfo; screen?: boolean; thumb?: boolean; pip?: boolean; solo?: boolean }) {
  const { p } = t;
  const source = screen ? p.screen : p.camOn ? p.video : undefined;
  const speaking = p.speaking && !screen;
  const chip = 'flex size-6 items-center justify-center rounded-md bg-surface-1/90 text-ink shadow-sm';
  const small = thumb || pip;
  return (
    <li
      data-testid="participant-tile"
      data-identity={p.identity}
      data-speaking={speaking}
      data-screen={screen ? true : undefined}
      aria-label={screen ? `${p.name} screen` : p.name}
      className={cn(
        'relative flex items-center justify-center overflow-hidden bg-surface-2',
        pip ? 'absolute bottom-3 right-3 z-20 aspect-video h-[clamp(84px,12vw,132px)] rounded-xl border border-border shadow-sm'
          : cn('rounded-xl', screen || solo ? 'size-full min-h-[180px]' : thumb ? 'aspect-video min-h-20' : 'min-h-[180px] max-sm:aspect-video max-sm:min-h-24'),
      )}
    >
      {source ? <Video source={source} mirror={p.isLocal && !screen} label={screen ? `${p.name} screen` : `${p.name} camera`} /> : (
        <div className="flex flex-col items-center gap-3">
          <ParticipantAvatar name={p.name} lang={t.lang} size={small ? 'md' : solo ? 'xl' : 'lg'} className={!small ? 'max-sm:[&>span:first-child]:size-12' : undefined} />
          {!screen && !small && <span aria-hidden className="max-w-full truncate px-3 text-[14px] font-medium text-ink">{p.name}</span>}
        </div>
      )}
      {speaking && <span aria-hidden className="pointer-events-none absolute inset-0 rounded-[inherit] ring-2 ring-inset ring-primary" />}
      {!screen && (
        <div className={cn('absolute flex gap-1', pip ? 'right-1.5 top-1.5' : 'right-2 top-2')}>
          {t.hand && (
            <span data-testid="hand-badge" role="img" aria-label="Hand raised" className={cn(chip, 'bg-amber-400 text-black')}>
              <Hand weight="fill" className="size-3.5" aria-hidden />
            </span>
          )}
          <span className={cn(chip, !p.micOn && 'text-red-600')} title={p.micOn ? 'Microphone on' : 'Microphone off'}>
            {p.micOn ? <Microphone className="size-3.5" aria-hidden /> : <MicrophoneSlash className="size-3.5" aria-hidden />}
          </span>
          {!pip && <span aria-hidden className={cn(chip, 'text-ink-muted')}><CellSignalFull className="size-3.5" /></span>}
        </div>
      )}
      <div className={cn('absolute flex max-w-[calc(100%-24px)] items-center gap-1.5 rounded-full bg-black/55 font-medium text-white backdrop-blur', small ? 'bottom-1.5 left-1.5 px-2 py-0.5 text-[11px]' : 'bottom-3 left-3 px-3 py-1 text-[13px] sm:bottom-5 sm:left-5')}>
        {screen && <MonitorArrowUp className="size-4 shrink-0" aria-hidden />}
        {!screen && t.lang && t.lang !== 'none' && <span aria-hidden className="shrink-0 rounded bg-white/20 px-1 text-[10px] font-semibold uppercase leading-4">{t.lang.slice(0, 2)}</span>}
        <span className="min-w-0 truncate">{screen ? `${p.name}'s screen` : p.name}</span>
        <span className="sr-only">{screen ? '' : `${p.isLocal ? ', you' : ''}${t.host ? ', host' : ''}. ${p.micOn ? 'Microphone on' : 'Microphone off'}${speaking ? ', speaking' : ''}`}</span>
      </div>
    </li>
  );
}

/** Participant grid. A screen share takes the main area; everybody else moves to a strip. Your own tile floats as a PiP. */
export function Stage({ tiles }: { tiles: TileInfo[] }) {
  const sharer = tiles.find((t) => t.p.sharing && t.p.screen);
  if (sharer) {
    return (
      <div className="flex h-full min-h-0 flex-col gap-3 p-3 lg:flex-row" data-testid="participant-grid" data-layout="spotlight">
        <ul aria-label="Shared screen" className="min-h-48 flex-1"><Tile t={sharer} screen /></ul>
        <ul aria-label="Participants" className="grid shrink-0 grid-cols-3 gap-2 max-lg:grid-flow-col max-lg:auto-cols-[8rem] max-lg:grid-cols-none max-lg:overflow-x-auto lg:w-56 lg:grid-cols-1 lg:overflow-y-auto">
          {tiles.map((t) => <Tile key={t.p.identity} t={t} thumb />)}
        </ul>
      </div>
    );
  }
  const self = tiles.find((t) => t.p.isLocal);
  const remotes = tiles.filter((t) => t !== self);
  const n = remotes.length;
  if (!self || n === 0) {
    return (
      <ul aria-label="Participants" data-testid="participant-grid" data-layout="grid" className="grid h-full min-h-0 w-full grid-cols-1 gap-3 p-3">
        {tiles.map((t) => <Tile key={t.p.identity} t={t} solo />)}
      </ul>
    );
  }
  return (
    <div className="relative h-full min-h-0" data-testid="participant-grid" data-layout="grid">
      <ul
        aria-label="Participants"
        className={cn(
          'grid h-full min-h-0 w-full gap-3 overflow-y-auto p-3 sm:auto-rows-fr',
          n === 1 ? 'grid-cols-1' : n === 2 ? 'grid-cols-1 lg:grid-cols-2' : 'grid-cols-2 xl:grid-cols-3',
        )}
      >
        {remotes.map((t) => <Tile key={t.p.identity} t={t} />)}
      </ul>
      <ul aria-label="You"><Tile t={self} pip /></ul>
    </div>
  );
}
