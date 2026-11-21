import type { HTMLAttributes, TdHTMLAttributes, ThHTMLAttributes } from 'react';
import { cn } from '@/lib/cn';

export function Table({ className, ...p }: HTMLAttributes<HTMLTableElement>) {
  return (
    <div className="overflow-x-auto rounded-lg border border-hairline bg-surface-1">
      <table className={cn('w-full border-collapse text-left text-body', className)} {...p} />
    </div>
  );
}
export const THead = (p: HTMLAttributes<HTMLTableSectionElement>) => <thead {...p} />;
export const TBody = (p: HTMLAttributes<HTMLTableSectionElement>) => <tbody {...p} />;
export const TR = ({ className, ...p }: HTMLAttributes<HTMLTableRowElement>) => (
  <tr className={cn('border-b border-hairline last:border-0 hover:bg-surface-2/60', className)} {...p} />
);
export const TH = ({ className, ...p }: ThHTMLAttributes<HTMLTableCellElement>) => (
  <th scope="col" className={cn('h-9 border-b border-hairline px-4 text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle', className)} {...p} />
);
export const TD = ({ className, ...p }: TdHTMLAttributes<HTMLTableCellElement>) => (
  <td className={cn('h-11 px-4 align-middle text-ink', className)} {...p} />
);

/** Linear-style list row: used for dense issue-like lists (rooms, meetings). */
export function ListRow({ className, ...p }: HTMLAttributes<HTMLDivElement>) {
  return <div className={cn('flex min-h-11 items-center gap-3 border-b border-hairline px-4 py-2 last:border-0 hover:bg-surface-2/60', className)} {...p} />;
}
