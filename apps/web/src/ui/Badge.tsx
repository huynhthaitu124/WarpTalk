import { cva, type VariantProps } from 'class-variance-authority';
import type { HTMLAttributes } from 'react';
import { CheckCircle } from '@phosphor-icons/react';
import { cn } from '@/lib/cn';

/**
 * Legacy chip recipe: tinted fill (c/10), 1px border (c/20), darker text of the same hue.
 * `shape` follows the kind of chip: `pill` for status, `tag` (8px) for member status / tags,
 * `box` (4px) for role and membership.
 */
const badge = cva('inline-flex items-center whitespace-nowrap border font-medium', {
  variants: {
    tone: {
      neutral: 'border-hairline bg-surface-2 text-ink-muted',
      primary: 'border-primary/20 bg-primary/10 text-chip-primary-text',
      success: 'border-chip-emerald/25 bg-chip-emerald/10 text-chip-emerald-text',
      warning: 'border-chip-amber/30 bg-chip-amber/10 text-chip-amber-text',
      danger: 'border-chip-rose/25 bg-chip-rose/10 text-chip-rose-text',
      info: 'border-chip-sky/25 bg-chip-sky/10 text-chip-sky-text',
      /** Finished states: gray text, no fill, no frame. */
      quiet: 'border-transparent bg-transparent px-0 text-chip-gray-text',
    },
    size: {
      sm: 'h-5 gap-1 px-2 text-[10px]',
      md: 'h-[22.5px] gap-1.5 px-2 text-[11px]',
    },
    shape: { pill: 'rounded-pill', tag: 'rounded-md', box: 'rounded-xs' },
  },
  defaultVariants: { tone: 'neutral', size: 'md', shape: 'pill' },
});

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badge> {}
export function Badge({ className, tone, size, shape, ...rest }: BadgeProps) {
  return <span className={cn(badge({ tone, size, shape }), className)} {...rest} />;
}

type Tone = NonNullable<VariantProps<typeof badge>['tone']>;
const STATUS_TONE: Record<string, Tone> = {
  SCHEDULED: 'success', WAITING: 'warning', OPEN: 'info', IN_PROGRESS: 'success', PAUSED: 'neutral',
  ENDED: 'quiet', CANCELLED: 'danger', EXPIRED: 'quiet', FAILED: 'danger',
  CONNECTED: 'success', DISCONNECTED: 'neutral', LEFT: 'neutral', KICKED: 'danger', REJECTED: 'danger', INVITED: 'warning',
  DRAFT: 'warning', PUBLISHED: 'success', ARCHIVED: 'neutral',
};
const ROLES = ['OWNER', 'ADMIN', 'MEMBER', 'HOST', 'PARTICIPANT'];
const MEMBER_STATUSES = ['CONNECTED', 'DISCONNECTED', 'LEFT', 'KICKED', 'REJECTED', 'INVITED'];
const LABEL: Record<string, string> = { IN_PROGRESS: 'Live' };

/** The 12px ring dot (1.5px border, c/20 fill) that leads every legacy status chip. */
export function StatusDot({ className }: { className?: string }) {
  return <span aria-hidden className={cn('size-3 shrink-0 rounded-pill border-[1.5px] border-current bg-current/20', className)} />;
}

/**
 * Status chip: ring dot + label (colour never carries meaning alone). Rooms and documents are pills,
 * workspace roles are 4px boxes, participant/member states are 8px tags.
 */
export function StatusPill({ status, className }: { status: string; className?: string }) {
  const tone = STATUS_TONE[status] ?? 'neutral';
  const label = LABEL[status] ?? status.toLowerCase().replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase());
  if (ROLES.includes(status)) {
    return <Badge size="sm" shape="box" className={cn('font-semibold', className)} data-status={status}>{label}</Badge>;
  }
  if (MEMBER_STATUSES.includes(status)) {
    return <Badge tone={tone} size="sm" shape="tag" className={className} data-status={status}>{label}</Badge>;
  }
  return (
    <Badge tone={tone} className={className} data-status={status}>
      {status === 'ENDED' ? <CheckCircle aria-hidden weight="fill" className="size-3 shrink-0" /> : <StatusDot />}
      {label}
    </Badge>
  );
}

export const ROOM_STATUS_SAMPLE = ['SCHEDULED', 'WAITING', 'OPEN', 'IN_PROGRESS', 'PAUSED', 'ENDED', 'CANCELLED', 'FAILED'] as const;
