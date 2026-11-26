import type { HTMLAttributes, ReactNode } from 'react';
import { cn } from '@v2/lib/cn';

/**
 * Legacy card: white, 12px radius, 1px hairline, barely-there shadow. `dashed` is the 8px
 * "nothing yet / still processing" placeholder, `featured` the lavender call-to-action card.
 */
export function Card({ className, raised, variant = 'default', ...rest }: HTMLAttributes<HTMLDivElement> & { raised?: boolean; variant?: 'default' | 'dashed' | 'featured' }) {
  return (
    <div
      className={cn(
        variant === 'dashed' ? 'rounded-md border border-dashed border-hairline bg-surface-1'
          : variant === 'featured' ? 'rounded-lg border border-primary bg-primary text-on-primary shadow-btn'
          : cn('rounded-lg border bg-surface-1 shadow-card', raised ? 'border-hairline-strong bg-elevated' : 'border-hairline'),
        className,
      )}
      {...rest}
    />
  );
}
/** No header band: a compact title row inside the card (long-form headings live outside, see `SectionLabel`). */
export function CardHeader({ title, description, actions }: { title: ReactNode; description?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4 px-4 pb-1 pt-3">
      <div className="min-w-0">
        <h2 className="text-title font-semibold text-ink">{title}</h2>
        {description && <p className="mt-0.5 text-body-sm text-ink-muted">{description}</p>}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </div>
  );
}
export const CardBody = ({ className, ...rest }: HTMLAttributes<HTMLDivElement>) => <div className={cn('p-4', className)} {...rest} />;

/** 11px uppercase muted group label that sits above (outside) a card or band. */
export function SectionLabel({ children, className, as: Tag = 'h2' }: { children: ReactNode; className?: string; as?: 'h2' | 'h3' | 'p' }) {
  return <Tag className={cn('mb-2 text-eyebrow font-semibold uppercase text-ink-subtle', className)}>{children}</Tag>;
}

/** Settings group: one white card whose rows are divided by hairlines. */
export function SettingsGroup({ className, ...rest }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('divide-y divide-hairline overflow-hidden rounded-lg border border-hairline bg-surface-1', className)} {...rest} />;
}
/** One settings row: label + description left, control right (stacks on narrow screens). */
export function SettingsRow({ label, description, children, className }: { label: ReactNode; description?: ReactNode; children?: ReactNode; className?: string }) {
  return (
    <div className={cn('flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6', className)}>
      <div className="min-w-0">
        <div className="text-body font-semibold text-ink">{label}</div>
        {description && <p className="mt-0.5 text-body-sm text-ink-muted">{description}</p>}
      </div>
      {children && <div className="min-w-0 sm:w-64 sm:shrink-0">{children}</div>}
    </div>
  );
}
