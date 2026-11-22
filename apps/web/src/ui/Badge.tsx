import { cva, type VariantProps } from 'class-variance-authority';
import type { HTMLAttributes } from 'react';
import { cn } from '@/lib/cn';

const badge = cva('inline-flex items-center gap-1.5 rounded-pill px-2 py-0.5 text-caption font-medium whitespace-nowrap', {
  variants: {
    tone: {
      neutral: 'bg-surface-2 text-ink-muted',
      primary: 'bg-primary/15 text-link',
      success: 'bg-success-soft text-success-text',
      warning: 'bg-warning-soft text-warning-text',
      danger: 'bg-danger-soft text-danger-text',
      info: 'bg-info-soft text-info-text',
    },
  },
  defaultVariants: { tone: 'neutral' },
});

export interface BadgeProps extends HTMLAttributes<HTMLSpanElement>, VariantProps<typeof badge> {}
export function Badge({ className, tone, ...rest }: BadgeProps) {
  return <span className={cn(badge({ tone }), className)} {...rest} />;
}

const STATUS_TONE: Record<string, VariantProps<typeof badge>['tone']> = {
  SCHEDULED: 'info', WAITING: 'warning', OPEN: 'info', IN_PROGRESS: 'success', PAUSED: 'warning',
  ENDED: 'neutral', CANCELLED: 'neutral', EXPIRED: 'neutral', FAILED: 'danger',
  CONNECTED: 'success', DISCONNECTED: 'neutral', LEFT: 'neutral', KICKED: 'danger', REJECTED: 'danger', INVITED: 'info',
  DRAFT: 'warning', PUBLISHED: 'success', ARCHIVED: 'neutral',
  OWNER: 'primary', ADMIN: 'info', MEMBER: 'neutral', HOST: 'primary', PARTICIPANT: 'neutral',
};
const LABEL: Record<string, string> = { IN_PROGRESS: 'Live' };

/** Status pill: colour dot + label. Colour never carries meaning alone (label always shown). */
export function StatusPill({ status, className }: { status: string; className?: string }) {
  const tone = STATUS_TONE[status] ?? 'neutral';
  const label = LABEL[status] ?? status.toLowerCase().replace(/_/g, ' ').replace(/^./, (c) => c.toUpperCase());
  return (
    <Badge tone={tone} className={className} data-status={status}>
      <span aria-hidden className="size-1.5 rounded-pill bg-current" />
      {label}
    </Badge>
  );
}

export const ROOM_STATUS_SAMPLE = ['SCHEDULED', 'WAITING', 'OPEN', 'IN_PROGRESS', 'PAUSED', 'ENDED', 'CANCELLED', 'FAILED'] as const;
