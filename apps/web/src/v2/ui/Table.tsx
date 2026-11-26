import type { HTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from 'react';
import { cn } from '@v2/lib/cn';

/**
 * Tables are full-bleed bands (hairline rows edge to edge), not boxed widgets. `framed` draws the
 * legacy knowledge-table frame (12px, white). `bare` is accepted for older call sites and is a no-op.
 * The wrapper scrolls horizontally on its own, so a wide table never widens the page.
 */
export function Table({ className, framed, bare: _bare, ...p }: HTMLAttributes<HTMLTableElement> & { bare?: boolean; framed?: boolean }) {
  return (
    <div className={cn('max-w-full overflow-x-auto', framed && 'rounded-lg border border-hairline bg-surface-1')}>
      <table className={cn('w-full border-collapse text-left text-body', className)} {...p} />
    </div>
  );
}
export const THead = (p: HTMLAttributes<HTMLTableSectionElement>) => <thead {...p} />;
export const TBody = (p: HTMLAttributes<HTMLTableSectionElement>) => <tbody {...p} />;
export const TR = ({ className, ...p }: HTMLAttributes<HTMLTableRowElement>) => (
  <tr className={cn('border-b border-row last:border-0 hover:bg-surface-2/50', className)} {...p} />
);
export const TH = ({ className, ...p }: ThHTMLAttributes<HTMLTableCellElement>) => (
  <th scope="col" className={cn('h-9 border-b border-row px-4 text-eyebrow font-semibold uppercase tracking-[0.275px] text-ink-muted', className)} {...p} />
);
export const TD = ({ className, ...p }: TdHTMLAttributes<HTMLTableCellElement>) => (
  <td className={cn('h-11 px-4 py-1 align-middle text-ink', className)} {...p} />
);

/** Full-bleed list row: 44px minimum, hairline underneath, hover tint. */
export function ListRow({ className, ...p }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('flex min-h-11 items-center gap-3 border-b border-row px-4 py-1 hover:bg-surface-2/50 max-sm:flex-wrap', className)} {...p} />;
}

/** Sticky 30px group header band above a run of rows (legacy list group header). */
export function GroupHeader({ className, ...p }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('sticky top-0 z-10 flex h-[30px] items-center gap-1.5 border-b border-row bg-surface-1/90 px-4 text-body-sm text-ink-muted backdrop-blur', className)} {...p} />;
}
