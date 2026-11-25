import { Link, NavLink } from 'react-router-dom';
import { useState, type ReactNode } from 'react';
import { CaretDown } from '@phosphor-icons/react';
import { cn } from '@/lib/cn';

/**
 * A sidebar group. `heading` is rendered as a visible eyebrow label; without it `label` is only the
 * accessible name of the `<nav>`.
 */
export function SidebarNav({ children, label = 'Primary', heading }: { children: ReactNode; label?: string; heading?: string }) {
  const [open, setOpen] = useState(true);
  return (
    <nav aria-label={label} className="flex flex-col gap-px">
      {heading && (
        <button
          type="button"
          aria-expanded={open}
          onClick={() => setOpen((o) => !o)}
          className="mb-0.5 flex h-7 w-fit items-center gap-1 rounded-md px-2 text-body-sm font-medium text-ink-muted hover:bg-surface-3/60 hover:text-ink focus-ring"
        >
          {heading}
          <CaretDown aria-hidden size={10} weight="fill" className={cn('transition-transform', !open && '-rotate-90')} />
        </button>
      )}
      {open && children}
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
            'flex h-[30px] min-w-0 flex-1 items-center gap-2.5 rounded-[8px] px-2 text-body font-medium no-underline transition-colors focus-ring hover:no-underline',
            actions?.length && 'pr-14',
            (active ?? isActive) ? 'bg-surface-3 text-ink' : 'text-ink/85 hover:bg-surface-3/60 hover:text-ink',
          )
        }
      >
        {({ isActive }) => (
          <>
            <span aria-hidden className={cn('shrink-0 transition-colors', (active ?? isActive) ? 'text-ink' : 'text-ink-muted group-hover:text-ink')}>{icon}</span>
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
