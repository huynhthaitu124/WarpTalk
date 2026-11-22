import type { HTMLAttributes } from 'react';
import { CalendarBlank, UsersThree } from '@phosphor-icons/react';
import { cn } from '@/lib/cn';
import { Avatar } from './Misc';

/** Legacy info capsule: white, hairline, 26px (list) or 28px (detail), 11px text. */
export function Capsule({ className, size = 'sm', ...rest }: HTMLAttributes<HTMLSpanElement> & { size?: 'sm' | 'lg' }) {
  return (
    <span
      className={cn(
        'inline-flex shrink-0 items-center gap-1.5 whitespace-nowrap rounded-pill border border-hairline/60 bg-surface-1 px-2.5 text-[11px] text-ink-muted shadow-capsule',
        size === 'lg' ? 'h-7' : 'h-[26px]',
        className,
      )}
      {...rest}
    />
  );
}

export function HostCapsule({ name, size, className }: { name: string; size?: 'sm' | 'lg'; className?: string }) {
  return (
    <Capsule size={size} className={cn('pl-1 text-ink', className)} data-testid="room-host">
      <Avatar name={name} size="sm" className="size-5" />
      <span className="max-w-32 truncate" title={name}>{name}</span>
    </Capsule>
  );
}

/** `VI · EN · JA`: 2-letter caps chips inside one capsule; the full names are the accessible label. */
export function LanguageCapsule({ source, targets, labels, size, className }: { source: string; targets: string[]; labels: string; size?: 'sm' | 'lg'; className?: string }) {
  const codes = [source, ...targets];
  return (
    <Capsule size={size} className={cn('gap-1', className)} title={labels} data-testid="room-languages">
      <span className="sr-only">{labels}</span>
      {codes.map((c, i) => (
        <span key={`${c}-${i}`} className="inline-flex items-center gap-1" aria-hidden>
          {i > 0 && <span className="text-ink-tertiary">·</span>}
          <span className="rounded-pill bg-surface-2 px-1.5 text-[11px] font-normal uppercase leading-[16px] text-ink-muted">{c.split('-')[0]}</span>
        </span>
      ))}
    </Capsule>
  );
}

export function OccupancyCapsule({ count, max, label, size }: { count: number; max?: number | null; label?: string; size?: 'sm' | 'lg' }) {
  return (
    <Capsule size={size} className="tabular-nums" data-testid="room-occupancy">
      <UsersThree size={14} aria-hidden />
      <span>{max ? `${count}/${max}` : label ?? count}</span>
    </Capsule>
  );
}

export function DateCapsule({ text, size }: { text: string; size?: 'sm' | 'lg' }) {
  return (
    <Capsule size={size} data-testid="room-when">
      <CalendarBlank size={14} aria-hidden />
      <span>{text}</span>
    </Capsule>
  );
}
