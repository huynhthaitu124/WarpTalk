import * as RC from '@radix-ui/react-checkbox';
import * as RSW from '@radix-ui/react-switch';
import { Check } from 'lucide-react';
import { useId, type ReactNode } from 'react';
import { cn } from '@/lib/cn';

export function Checkbox({
  checked, onCheckedChange, label, description, disabled, className,
}: { checked: boolean; onCheckedChange(v: boolean): void; label: ReactNode; description?: ReactNode; disabled?: boolean; className?: string }) {
  const id = useId();
  return (
    <div className={cn('flex items-start gap-2.5', className)}>
      <RC.Root
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={(v) => onCheckedChange(v === true)}
        className="mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-xs border border-hairline-tertiary bg-surface-1 transition-colors focus-ring data-[state=checked]:border-primary data-[state=checked]:bg-primary disabled:opacity-50"
      >
        <RC.Indicator><Check className="size-3 text-on-primary" strokeWidth={3} aria-hidden /></RC.Indicator>
      </RC.Root>
      <label htmlFor={id} className="flex flex-col text-body text-ink">
        {label}
        {description && <span className="text-caption text-ink-subtle">{description}</span>}
      </label>
    </div>
  );
}

export function Switch({
  checked, onCheckedChange, label, description, disabled, className,
}: { checked: boolean; onCheckedChange(v: boolean): void; label: ReactNode; description?: ReactNode; disabled?: boolean; className?: string }) {
  const id = useId();
  return (
    <div className={cn('flex items-center justify-between gap-4', className)}>
      <label htmlFor={id} className="flex flex-col text-body text-ink">
        {label}
        {description && <span className="text-caption text-ink-subtle">{description}</span>}
      </label>
      <RSW.Root
        id={id}
        checked={checked}
        disabled={disabled}
        onCheckedChange={onCheckedChange}
        className="relative h-5 w-9 shrink-0 rounded-pill bg-surface-4 transition-colors focus-ring data-[state=checked]:bg-primary disabled:opacity-50"
      >
        <RSW.Thumb className="block size-4 translate-x-0.5 rounded-pill bg-white shadow transition-transform data-[state=checked]:translate-x-[18px]" />
      </RSW.Root>
    </div>
  );
}
