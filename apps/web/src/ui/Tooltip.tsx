import * as RT from '@radix-ui/react-tooltip';
import type { ReactNode } from 'react';

export const TooltipProvider = ({ children }: { children: ReactNode }) => (
  <RT.Provider delayDuration={300}>{children}</RT.Provider>
);

export function Tooltip({ content, children, side = 'top' }: { content: ReactNode; children: ReactNode; side?: 'top' | 'right' | 'bottom' | 'left' }) {
  return (
    <RT.Root>
      <RT.Trigger asChild>{children}</RT.Trigger>
      <RT.Portal>
        <RT.Content side={side} sideOffset={6} className="z-[70] animate-fade-in rounded-md bg-popover px-2 py-1 text-caption text-ink shadow-pop">
          {content}
        </RT.Content>
      </RT.Portal>
    </RT.Root>
  );
}
