import { Link, NavLink } from 'react-router-dom';
import type { ReactNode } from 'react';
import { cn } from '@/lib/cn';

/**
 * A sidebar group. `heading` is rendered as a visible eyebrow label; without it `label` is only the
 * accessible name of the `<nav>`.
 */
export function SidebarNav({ children, label = 'Primary', heading }: { children: ReactNode; label?: string; heading?: string }) {
  return (
    <nav aria-label={label} className="flex flex-col gap-0.5">
      {heading && <p className="px-2 pb-1 pt-1 text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">{heading}</p>}
      {children}
    </nav>
  );
}

export interface SidebarAction { icon: ReactNode; label: string; to?: string; onClick?(): void }

/**
 * Legacy `linear-sidebar` row: 30px high, 13px/500 label, muted icon, active = surface-3 with ink,
 * hover = surface-3 at 60%. Optional hover actions (join by code, create) sit at the right edge and
 * are always in the tab order; `badge` is a quiet count.
 */
export function SidebarLink({
  to, icon, children, end, active, badge, actions,
}: { to: string; icon: ReactNode; children: ReactNode; end?: boolean; active?: boolean; badge?: string | number | null; actions?: SidebarAction[] }) {
  const actionCls = 'shrink-0 rounded p-1 text-ink-muted opacity-0 transition-opacity hover:bg-surface-4/60 hover:text-ink focus-ring focus-visible:opacity-100 group-hover:opacity-100 max-md:opacity-100';
  return (
    <div className="group relative flex items-center">
      <NavLink
        to={to}
        end={end}
        className={({ isActive }) =>
          cn(
            'flex h-[30px] min-w-0 flex-1 items-center gap-2.5 rounded-md px-2 text-body-sm font-medium tracking-tight no-underline transition-colors focus-ring hover:no-underline',
            actions?.length && 'pr-14',
            (active ?? isActive) ? 'bg-surface-3 text-ink' : 'text-ink/90 hover:bg-surface-3/60 hover:text-ink',
          )
        }
      >
        {({ isActive }) => (
          <>
            <span aria-hidden className={cn('shrink-0 transition-colors', (active ?? isActive) ? 'text-ink' : 'text-ink-muted/80 group-hover:text-ink/80')}>{icon}</span>
            <span className="truncate">{children}</span>
          </>
        )}
      </NavLink>
      {badge ? (
        <span aria-label={`${badge} pending`} className="pointer-events-none absolute right-2 grid h-[18px] min-w-[18px] place-items-center rounded-full bg-ink/[0.08] px-1.5 text-[11px] font-medium leading-none tabular-nums text-ink-muted">{badge}</span>
      ) : null}
      {actions?.length ? (
        <div className="absolute right-1 flex items-center">
          {actions.map((a) =>
            a.to ? (
              <Link key={a.label} to={a.to} aria-label={a.label} title={a.label} className={actionCls}>{a.icon}</Link>
            ) : (
              <button key={a.label} type="button" aria-label={a.label} title={a.label} onClick={a.onClick} className={actionCls}>{a.icon}</button>
            ),
          )}
        </div>
      ) : null}
    </div>
  );
}
