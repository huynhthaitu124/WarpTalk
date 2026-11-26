import { createContext, useContext, useId, type ReactNode } from 'react';
import { cn } from '@v2/lib/cn';

const FieldCtx = createContext<{ id: string; describedBy?: string; invalid: boolean } | null>(null);

export function useFieldProps() {
  const c = useContext(FieldCtx);
  return c ? { id: c.id, 'aria-describedby': c.describedBy, 'aria-invalid': c.invalid || undefined } : {};
}

/** Label + control + hint/error wiring. Controls inside pick id/aria-* up automatically. */
export function Field({
  label, hint, error, children, className, required,
}: { label: string; hint?: ReactNode; error?: ReactNode; children: ReactNode; className?: string; required?: boolean }) {
  const id = useId();
  const descId = hint || error ? `${id}-desc` : undefined;
  return (
    <FieldCtx.Provider value={{ id, describedBy: descId, invalid: !!error }}>
      <div className={cn('flex flex-col gap-1.5', className)}>
        <label htmlFor={id} className="text-body-sm font-medium text-ink-muted">{label}{required && <span aria-hidden className="text-danger-text"> *</span>}</label>
        {children}
        {descId && (
          <p id={descId} role={error ? 'alert' : undefined} className={cn('text-caption', error ? 'text-danger-text' : 'text-ink-subtle')}>
            {error ?? hint}
          </p>
        )}
      </div>
    </FieldCtx.Provider>
  );
}

/** Settings-card row: bold label + muted hint on the left, the control on the right (label stays bound to it). */
export function FieldRow({
  label, hint, error, children, className,
}: { label: string; hint?: ReactNode; error?: ReactNode; children: ReactNode; className?: string }) {
  const id = useId();
  const descId = hint || error ? `${id}-desc` : undefined;
  return (
    <FieldCtx.Provider value={{ id, describedBy: descId, invalid: !!error }}>
      <div className={cn('flex flex-col gap-3 px-5 py-4 sm:flex-row sm:items-center sm:justify-between sm:gap-6', className)}>
        <div className="min-w-0">
          <label htmlFor={id} className="block text-body font-semibold text-ink">{label}</label>
          {descId && (
            <p id={descId} role={error ? 'alert' : undefined} className={cn('mt-0.5 text-body-sm', error ? 'text-danger-text' : 'text-ink-muted')}>{error ?? hint}</p>
          )}
        </div>
        <div className="min-w-0 sm:w-64 sm:shrink-0">{children}</div>
      </div>
    </FieldCtx.Provider>
  );
}
