import * as RA from '@radix-ui/react-avatar';
import type { HTMLAttributes, ReactNode } from 'react';
import { cn } from '@/lib/cn';

export function Skeleton({ className, ...p }: HTMLAttributes<HTMLDivElement>) {
  return <div aria-hidden className={cn('animate-shimmer rounded-md bg-surface-2', className)} {...p} />;
}

export function EmptyState({
  icon, title, description, action, className,
}: { icon?: ReactNode; title: string; description?: ReactNode; action?: ReactNode; className?: string }) {
  return (
    <div className={cn('flex flex-col items-center gap-3 rounded-lg border border-dashed border-hairline-strong px-6 py-12 text-center', className)}>
      {icon && <div className="flex size-10 items-center justify-center rounded-lg bg-surface-2 text-ink-subtle" aria-hidden>{icon}</div>}
      <h3 className="text-title font-semibold tracking-[-0.2px] text-ink">{title}</h3>
      {description && <p className="max-w-sm text-body text-ink-subtle">{description}</p>}
      {action}
    </div>
  );
}

// All >= 4.5:1 against white initials.
const PALETTE = ['#5e6ad2', '#1f7a6e', '#a8650f', '#b0386f', '#2f6fd0', '#6a3fc0'];
function hue(s: string) {
  let h = 0;
  for (const c of s) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return PALETTE[h % PALETTE.length]!;
}
function initials(name: string) {
  const parts = name.trim().split(/\s+/).filter(Boolean);
  return ((parts[0]?.[0] ?? '?') + (parts.length > 1 ? parts[parts.length - 1]![0]! : '')).toUpperCase();
}

export function Avatar({ name, src, size = 'md', className }: { name: string; src?: string | null; size?: 'sm' | 'md' | 'lg'; className?: string }) {
  const dim = size === 'sm' ? 'size-6 text-[10px]' : size === 'lg' ? 'size-10 text-body' : 'size-8 text-caption';
  return (
    <RA.Root className={cn('inline-flex shrink-0 select-none items-center justify-center overflow-hidden rounded-pill font-semibold text-white', dim, className)} style={{ background: hue(name) }} title={name}>
      {src && <RA.Image src={src} alt="" className="size-full object-cover" />}
      <RA.Fallback aria-hidden>{initials(name)}</RA.Fallback>
    </RA.Root>
  );
}

export function Kbd({ children, className }: { children: ReactNode; className?: string }) {
  return (
    <kbd className={cn('inline-flex h-5 min-w-5 items-center justify-center rounded-xs border border-hairline-strong bg-surface-2 px-1 font-mono text-[11px] text-ink-subtle', className)}>
      {children}
    </kbd>
  );
}

export function PageHeader({ title, description, actions }: { title: string; description?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="mb-6 flex flex-wrap items-start justify-between gap-4">
      <div className="min-w-0 max-w-full">
        <h1 className="line-clamp-2 break-words text-headline font-semibold tracking-[-0.6px] text-ink max-sm:text-title" title={title}>{title}</h1>
        {description && <div className="mt-1 text-body text-ink-subtle">{description}</div>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}

export function Alert({ tone = 'info', title, children, className }: { tone?: 'info' | 'warning' | 'danger' | 'success'; title?: string; children?: ReactNode; className?: string }) {
  const t = { info: 'bg-info-soft text-info-text', warning: 'bg-warning-soft text-warning-text', danger: 'bg-danger-soft text-danger-text', success: 'bg-success-soft text-success-text' }[tone];
  return (
    <div role={tone === 'danger' ? 'alert' : 'status'} className={cn('rounded-lg px-4 py-3 text-body-sm', t, className)}>
      {title && <p className="font-semibold">{title}</p>}
      {children && <div className={title ? 'mt-0.5' : ''}>{children}</div>}
    </div>
  );
}
