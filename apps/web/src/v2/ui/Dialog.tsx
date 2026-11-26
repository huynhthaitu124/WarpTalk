import * as RD from '@radix-ui/react-dialog';
import { X } from '@phosphor-icons/react';
import { createContext, useContext, useRef, type MutableRefObject, type ReactNode } from 'react';
import { cn } from '@v2/lib/cn';

const ReturnFocus = createContext<MutableRefObject<HTMLElement | null> | null>(null);

/**
 * Radix restores focus to a <DialogTrigger>. Dialogs opened from elsewhere (URL state, command
 * menu) have no trigger, so remember the element that had focus when `open` turned true and
 * give focus back to it on close.
 */
export function Dialog({ open, ...props }: RD.DialogProps) {
  const ret = useRef<HTMLElement | null>(null);
  const wasOpen = useRef(false);
  if (open && !wasOpen.current && typeof document !== 'undefined') {
    const el = document.activeElement;
    ret.current = el instanceof HTMLElement && el !== document.body ? el : null;
  }
  wasOpen.current = !!open;
  return (
    <ReturnFocus.Provider value={ret}>
      <RD.Root open={open} {...props} />
    </ReturnFocus.Provider>
  );
}
export const DialogTrigger = RD.Trigger;
export const DialogClose = RD.Close;

export function DialogContent({
  title, description, children, footer, className,
}: { title: string; description?: string; children?: ReactNode; footer?: ReactNode; className?: string }) {
  const ret = useContext(ReturnFocus);
  return (
    <RD.Portal>
      <RD.Overlay className="fixed inset-0 z-50 animate-fade-in bg-overlay backdrop-blur-[2px]" />
      <RD.Content
        onCloseAutoFocus={(e) => {
          const el = ret?.current;
          if (el?.isConnected) { e.preventDefault(); el.focus(); }
        }}
        className={cn(
          'fixed left-1/2 top-1/2 z-50 flex max-h-[85vh] w-[calc(100vw-32px)] max-w-lg -translate-x-1/2 -translate-y-1/2 animate-pop-in flex-col rounded-xl bg-elevated shadow-pop focus:outline-none',
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
          <RD.Close aria-label="Close dialog" className="-mr-2 -mt-1 rounded-md p-1.5 text-ink-subtle hover:bg-popover-hover hover:text-ink focus-ring">
            <X className="size-4" aria-hidden />
          </RD.Close>
        </div>
        <div className="overflow-y-auto px-6 py-3">{children}</div>
        {footer && <div className="flex justify-end gap-2 border-t border-hairline px-6 py-4">{footer}</div>}
      </RD.Content>
    </RD.Portal>
  );
}
