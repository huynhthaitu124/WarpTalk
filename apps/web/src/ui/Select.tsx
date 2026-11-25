import * as RS from '@radix-ui/react-select';
import type { ReactNode } from 'react';
import { Check, CaretDown } from '@phosphor-icons/react';
import { cn } from '@/lib/cn';
import { controlStyles } from './Input';
import { useFieldProps } from './Field';

export interface SelectOption { value: string; label: string; disabled?: boolean; icon?: ReactNode }

export function Select({
  value, onValueChange, options, placeholder = 'Select…', disabled, className, 'aria-label': ariaLabel, testId,
}: {
  value: string;
  onValueChange(v: string): void;
  options: SelectOption[];
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  'aria-label'?: string;
  testId?: string;
}) {
  const f = useFieldProps();
  const sel = options.find((o) => o.value === value);
  return (
    <RS.Root value={value} onValueChange={onValueChange} disabled={disabled}>
      <RS.Trigger
        {...f}
        aria-label={ariaLabel}
        data-testid={testId}
        className={cn(controlStyles, 'flex h-9 items-center justify-between gap-2 text-left data-[placeholder]:text-ink-tertiary', className)}
      >
        <RS.Value placeholder={placeholder}>{sel?.icon ? <span className="flex min-w-0 items-center gap-2">{sel.icon}<span className="truncate">{sel.label}</span></span> : undefined}</RS.Value>
        <RS.Icon><CaretDown className="size-4 text-ink-subtle" aria-hidden /></RS.Icon>
      </RS.Trigger>
      <RS.Portal>
        <RS.Content position="popper" sideOffset={4} className="z-[60] min-w-[var(--radix-select-trigger-width)] animate-pop-in overflow-hidden rounded-lg bg-popover p-1 shadow-pop">
          <RS.Viewport>
            {options.map((o) => (
              <RS.Item
                key={o.value}
                value={o.value}
                disabled={o.disabled}
                className="relative flex h-8 cursor-default select-none items-center rounded-md pl-8 pr-3 text-body text-ink outline-none data-[disabled]:opacity-40 data-[highlighted]:bg-popover-hover"
              >
                <RS.ItemIndicator className="absolute left-2"><Check weight="bold" className="size-4 text-primary" aria-hidden /></RS.ItemIndicator>
                {o.icon && <span className="mr-2 flex">{o.icon}</span>}
                <RS.ItemText>{o.label}</RS.ItemText>
              </RS.Item>
            ))}
          </RS.Viewport>
        </RS.Content>
      </RS.Portal>
    </RS.Root>
  );
}
