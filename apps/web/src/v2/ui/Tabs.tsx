import * as RT from '@radix-ui/react-tabs';
import type { ComponentProps, HTMLAttributes } from 'react';
import { cn } from '@v2/lib/cn';

export const Tabs = RT.Root;
export const TabsContent = ({ className, ...p }: ComponentProps<typeof RT.Content>) => (
  <RT.Content className={cn('focus-ring', className)} {...p} />
);
/** Legacy pill tabs: 1px soft border idle, `surface-2` fill when active. */
export const TabsList = ({ className, ...p }: ComponentProps<typeof RT.List>) => (
  <RT.List className={cn('flex gap-2 overflow-x-auto [scrollbar-width:none] [&::-webkit-scrollbar]:hidden', className)} {...p} />
);
export const TabsTrigger = ({ className, ...p }: ComponentProps<typeof RT.Trigger>) => (
  <RT.Trigger
    className={cn(
      'inline-flex h-8 shrink-0 items-center gap-1.5 whitespace-nowrap rounded-pill border border-hairline/60 px-4 text-body font-medium text-ink-muted transition-colors hover:text-ink focus-ring data-[state=active]:border-transparent data-[state=active]:bg-surface-2 data-[state=active]:text-ink',
      className,
    )}
    {...p}
  />
);
/** 18px count badge for a tab (`Invited 1`). */
export const TabCount = ({ className, ...p }: HTMLAttributes<HTMLSpanElement>) => (
  <span className={cn('grid h-[18px] min-w-[18px] place-items-center rounded-pill bg-ink/[0.08] px-1.5 text-[11px] font-medium leading-none tabular-nums text-ink-muted', className)} {...p} />
);
