import * as RD from '@radix-ui/react-dialog';
import { X } from 'lucide-react';
import type { ReactNode } from 'react';
import { cn } from '@/lib/cn';

export const Dialog = RD.Root;
export const DialogTrigger = RD.Trigger;
export const DialogClose = RD.Close;

export function DialogContent({
  title, description, children, footer, className,
}: { title: string; description?: string; children?: ReactNode; footer?: ReactNode; className?: string }) {
  return (
    <RD.Portal>
      <RD.Overlay className="fixed inset-0 z-50 animate-fade-in bg-overlay backdrop-blur-[2px]" />
      <RD.Content
        className={cn(
          'fixed left-1/2 top-1/2 z-50 flex max-h-[85vh] w-[calc(100vw-32px)] max-w-lg -translate-x-1/2 -translate-y-1/2 animate-pop-in flex-col rounded-xl border border-hairline-strong bg-surface-2 shadow-pop focus:outline-none',
          className,
        )}
      >
        <div className="flex items-start justify-between gap-4 px-6 pb-2 pt-5">
          <div>
            <RD.Title className="text-title font-semibold tracking-[-0.2px] text-ink">{title}</RD.Title>
            {description ? (
              <RD.Description className="mt-1 text-body-sm text-ink-subtle">{description}</RD.Description>
            ) : (
              <RD.Description className="sr-only">{title}</RD.Description>
            )}
          </div>
          <RD.Close aria-label="Close dialog" className="-mr-2 -mt-1 rounded-md p-1.5 text-ink-subtle hover:bg-surface-3 hover:text-ink focus-ring">
            <X className="size-4" aria-hidden />
          </RD.Close>
        </div>
        <div className="overflow-y-auto px-6 py-3">{children}</div>
        {footer && <div className="flex justify-end gap-2 border-t border-hairline px-6 py-4">{footer}</div>}
      </RD.Content>
    </RD.Portal>
  );
}
