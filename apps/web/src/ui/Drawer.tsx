import * as RD from '@radix-ui/react-dialog';
import { X } from '@phosphor-icons/react';
import type { ReactNode } from 'react';
import { cn } from '@/lib/cn';

/** Right-hand sheet (audit history, details). A Radix dialog: focus trap, Escape, scroll lock, focus return. */
export function Drawer({ open, onOpenChange, title, description, children, className }: {
  open: boolean; onOpenChange(v: boolean): void; title: string; description?: string; children?: ReactNode; className?: string;
}) {
  return (
    <RD.Root open={open} onOpenChange={onOpenChange}>
      <RD.Portal>
        <RD.Overlay className="fixed inset-0 z-50 animate-fade-in bg-overlay" />
        <RD.Content
          className={cn('fixed inset-y-0 right-0 z-50 flex w-[28rem] max-w-full animate-fade-in flex-col border-l border-hairline-strong bg-surface-2 shadow-pop focus:outline-none', className)}
        >
          <div className="flex items-start justify-between gap-4 border-b border-hairline px-5 py-4">
            <div className="min-w-0">
              <RD.Title className="text-title font-semibold tracking-[-0.2px] text-ink">{title}</RD.Title>
              {description ? <RD.Description className="mt-1 text-body-sm text-ink-subtle">{description}</RD.Description> : <RD.Description className="sr-only">{title}</RD.Description>}
            </div>
            <RD.Close aria-label="Close drawer" className="-mr-2 -mt-1 rounded-md p-1.5 text-ink-subtle hover:bg-surface-3 hover:text-ink focus-ring"><X className="size-4" aria-hidden /></RD.Close>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">{children}</div>
        </RD.Content>
      </RD.Portal>
    </RD.Root>
  );
}
