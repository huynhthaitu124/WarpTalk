import { NavLink } from 'react-router-dom';
import type { ReactNode } from 'react';
import { cn } from '@/lib/cn';

export function SidebarNav({ children, label = 'Primary' }: { children: ReactNode; label?: string }) {
  return <nav aria-label={label} className="flex flex-col gap-0.5">{children}</nav>;
}

export function SidebarLink({ to, icon, children, end, active }: { to: string; icon: ReactNode; children: ReactNode; end?: boolean; active?: boolean }) {
  return (
    <NavLink
      to={to}
      end={end}
      className={({ isActive }) =>
        cn(
          'flex h-8 items-center gap-2.5 rounded-md px-2.5 text-body font-medium no-underline transition-colors focus-ring hover:no-underline',
          (active ?? isActive) ? 'bg-surface-3 text-ink' : 'text-ink-subtle hover:bg-surface-2 hover:text-ink',
        )
      }
    >
      <span aria-hidden className="shrink-0">{icon}</span>
      {children}
    </NavLink>
  );
}
