import { createContext, useContext, type ReactNode } from 'react';
import { cn } from '@v2/lib/cn';

const PadContext = createContext(false);

/** Padded page body for pages that are not full-bleed: gives them the 16px gutter the shell no longer applies. */
export function Padded({ children, className }: { children: ReactNode; className?: string }) {
  return <PadContext.Provider value><div className={cn('min-w-0 px-4 py-4', className)}>{children}</div></PadContext.Provider>;
}
/** Gutter for the non-band part of a full-bleed page (alerts, forms, empty states). */
export function PageBody({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cn('min-w-0 px-4 py-4', className)}>{children}</div>;
}

/**
 * Page header. `bar` (default, list pages): the title is the breadcrumb, so the h1 is screen-reader
 * only and the band holds the description and actions. `page`: visible 20px title (Home).
 * `detail`: 30px title (room detail). `settings`: 20px/700 title.
 */
export function PageHeader({
  title, description, actions, size = 'bar', flush,
}: { title: string; description?: ReactNode; actions?: ReactNode; size?: 'bar' | 'page' | 'detail' | 'settings'; flush?: boolean }) {
  const padded = useContext(PadContext);
  const bleed = padded ? '-mx-4 -mt-4 mb-4' : '';
  if (size === 'bar') {
    return (
      <div className={cn('flex min-h-11 flex-wrap items-center justify-between gap-x-4 gap-y-2 border-b border-row px-4 py-1.5', bleed)}>
        <h1 className="sr-only">{title}</h1>
        <div className="min-w-0 max-w-full flex-1 truncate text-body-sm text-ink-muted">{description}</div>
        {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
      </div>
    );
  }
  const h1 = {
    page: 'text-section font-semibold text-ink',
    detail: 'text-headline font-semibold max-sm:text-title text-ink',
    settings: 'text-settings font-bold text-ink',
  }[size];
  return (
    <div className={cn('flex flex-wrap items-start justify-between gap-4', flush ? 'pb-6' : 'px-4 pb-4 pt-5', size === 'page' && 'border-b border-row', !flush && bleed)}>
      <div className="min-w-0 max-w-full">
        <h1 className={cn('line-clamp-2 break-words', h1)} title={title}>{title}</h1>
        {description && <div className="mt-1 text-body-sm text-ink-muted">{description}</div>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2">{actions}</div>}
    </div>
  );
}
