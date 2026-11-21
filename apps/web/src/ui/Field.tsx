import { createContext, useContext, useId, type ReactNode } from 'react';
import { cn } from '@/lib/cn';

const FieldCtx = createContext<{ id: string; describedBy?: string; invalid: boolean } | null>(null);

export function useFieldProps() {
  const c = useContext(FieldCtx);
  return c ? { id: c.id, 'aria-describedby': c.describedBy, 'aria-invalid': c.invalid || undefined } : {};
}

/** Label + control + hint/error wiring. Controls inside pick id/aria-* up automatically. */
export function Field({
  label, hint, error, children, className,
}: { label: string; hint?: ReactNode; error?: ReactNode; children: ReactNode; className?: string }) {
  const id = useId();
  const descId = hint || error ? `${id}-desc` : undefined;
  return (
    <FieldCtx.Provider value={{ id, describedBy: descId, invalid: !!error }}>
      <div className={cn('flex flex-col gap-1.5', className)}>
        <label htmlFor={id} className="text-body-sm font-medium text-ink-muted">{label}</label>
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
