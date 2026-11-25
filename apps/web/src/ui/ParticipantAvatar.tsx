import { Avatar } from './Misc';
import { cn } from '@/lib/cn';

export type ParticipantAvatarSize = 'xs' | 'sm' | 'md' | 'lg' | 'xl';

const DIM: Record<ParticipantAvatarSize, { box: string; badge: string }> = {
  xs: { box: 'size-6 text-[9px]', badge: 'h-3.5 min-w-3.5 text-[8px] -bottom-0.5 -right-0.5' },
  sm: { box: 'size-7 text-[10px]', badge: 'h-4 min-w-4 text-[9px] -bottom-0.5 -right-0.5' },
  md: { box: 'size-9 text-caption', badge: 'h-4 min-w-4 text-[9px] -bottom-1 -right-1' },
  lg: { box: 'size-14 text-title', badge: 'h-5 min-w-5 text-[11px] -bottom-1 -right-1' },
  xl: { box: 'size-20 text-[26px]', badge: 'h-6 min-w-6 text-[13px] -bottom-1 -right-1' },
};

/** Initials circle with the participant's speaking language as a small corner badge. */
export function ParticipantAvatar({ name, lang, size = 'md', speaking, src, className }: {
  name: string;
  /** Language code, shown upper-cased in the corner badge. */
  lang?: string;
  size?: ParticipantAvatarSize;
  speaking?: boolean;
  src?: string | null;
  className?: string;
}) {
  const d = DIM[size];
  return (
    <span className={cn('relative inline-flex shrink-0 rounded-full', speaking && 'ring-2 ring-primary ring-offset-1 ring-offset-surface-1', className)}>
      <Avatar name={name} src={src} className={d.box} />
      {lang && lang !== 'none' && (
        <span aria-hidden className={cn('absolute grid place-items-center rounded-full border border-border bg-surface-1 px-0.5 font-semibold uppercase leading-none text-ink', d.badge)}>
          {lang.slice(0, 2)}
        </span>
      )}
    </span>
  );
}
