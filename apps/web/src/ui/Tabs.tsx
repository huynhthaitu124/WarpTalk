import * as RT from '@radix-ui/react-tabs';
import type { ComponentProps } from 'react';
import { cn } from '@/lib/cn';

export const Tabs = RT.Root;
export const TabsContent = ({ className, ...p }: ComponentProps<typeof RT.Content>) => (
  <RT.Content className={cn('mt-5 focus-ring', className)} {...p} />
);
export const TabsList = ({ className, ...p }: ComponentProps<typeof RT.List>) => (
  <RT.List className={cn('flex gap-1 overflow-x-auto border-b border-hairline [scrollbar-width:none] [&::-webkit-scrollbar]:hidden', className)} {...p} />
);
export const TabsTrigger = ({ className, ...p }: ComponentProps<typeof RT.Trigger>) => (
  <RT.Trigger
    className={cn(
      '-mb-px h-9 shrink-0 whitespace-nowrap border-b-2 border-transparent px-3 text-body font-medium text-ink-subtle transition-colors hover:text-ink focus-ring data-[state=active]:border-primary data-[state=active]:text-ink',
      className,
    )}
    {...p}
  />
);
