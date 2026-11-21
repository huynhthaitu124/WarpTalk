import type { HTMLAttributes, ReactNode } from 'react';
import { cn } from '@/lib/cn';

export function Card({ className, raised, ...rest }: HTMLAttributes<HTMLDivElement> & { raised?: boolean }) {
  return (
    <div
      className={cn('rounded-lg border bg-surface-1 edge-top', raised ? 'border-hairline-strong bg-surface-2' : 'border-hairline', className)}
      {...rest}
    />
  );
}
export function CardHeader({ title, description, actions }: { title: ReactNode; description?: ReactNode; actions?: ReactNode }) {
  return (
    <div className="flex items-start justify-between gap-4 border-b border-hairline px-5 py-4">
      <div className="min-w-0">
        <h2 className="text-title font-semibold tracking-[-0.2px] text-ink">{title}</h2>
        {description && <p className="mt-0.5 text-body-sm text-ink-subtle">{description}</p>}
      </div>
      {actions && <div className="flex shrink-0 items-center gap-2">{actions}</div>}
    </div>
  );
}
export const CardBody = ({ className, ...rest }: HTMLAttributes<HTMLDivElement>) => <div className={cn('p-5', className)} {...rest} />;
