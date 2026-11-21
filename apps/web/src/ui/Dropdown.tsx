import * as DM from '@radix-ui/react-dropdown-menu';
import type { ComponentProps, ReactNode } from 'react';
import { cn } from '@/lib/cn';

export const Dropdown = DM.Root;
export const DropdownTrigger = DM.Trigger;
export const DropdownLabel = ({ className, ...p }: ComponentProps<typeof DM.Label>) => (
  <DM.Label className={cn('px-2 py-1.5 text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle', className)} {...p} />
);
export const DropdownSeparator = () => <DM.Separator className="my-1 h-px bg-hairline" />;

export function DropdownContent({ className, align = 'start', ...p }: ComponentProps<typeof DM.Content>) {
  return (
    <DM.Portal>
      <DM.Content
        align={align}
        sideOffset={6}
        className={cn('z-[60] min-w-52 animate-pop-in rounded-lg border border-hairline-strong bg-surface-3 p-1 shadow-pop', className)}
        {...p}
      />
    </DM.Portal>
  );
}

export function DropdownItem({
  className, icon, shortcut, danger, children, ...p
}: ComponentProps<typeof DM.Item> & { icon?: ReactNode; shortcut?: ReactNode; danger?: boolean }) {
  return (
    <DM.Item
      className={cn(
        'flex h-8 cursor-default select-none items-center gap-2 rounded-md px-2 text-body outline-none data-[disabled]:opacity-40 data-[highlighted]:bg-surface-4',
        danger ? 'text-danger-text' : 'text-ink',
        className,
      )}
      {...p}
    >
      {icon && <span className="text-ink-subtle" aria-hidden>{icon}</span>}
      <span className="flex-1 truncate">{children}</span>
      {shortcut && <span className="text-caption text-ink-tertiary">{shortcut}</span>}
    </DM.Item>
  );
}
