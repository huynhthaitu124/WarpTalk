import { forwardRef, type ButtonHTMLAttributes } from 'react';
import { cva, type VariantProps } from 'class-variance-authority';
import { CircleNotch } from '@phosphor-icons/react';
import { cn } from '@/lib/cn';

export const buttonStyles = cva(
  'inline-flex shrink-0 select-none items-center justify-center gap-2 whitespace-nowrap rounded-pill font-medium transition-colors focus-ring disabled:pointer-events-none disabled:opacity-50 aria-busy:cursor-progress',
  {
    variants: {
      variant: {
        /* Legacy primary: an ink pill. The lavender `accent` variant is for featured calls to action. */
        primary: 'bg-ink-btn text-on-ink-btn shadow-btn hover:opacity-90 active:opacity-80',
        accent: 'bg-primary text-on-primary shadow-btn hover:bg-primary-hover active:bg-primary-pressed',
        secondary: 'border border-hairline/60 bg-btn-2 text-ink shadow-btn hover:bg-btn-2-hover active:bg-btn-2-hover',
        ghost: 'rounded-md text-ink-muted hover:bg-surface-2 hover:text-ink active:bg-surface-3',
        danger: 'bg-danger text-on-danger hover:brightness-110 active:brightness-95',
        /* In-page destructive actions (End, Leave): a soft tint. Solid `danger` is for confirm dialogs. */
        'danger-soft': 'bg-danger-soft text-danger-text hover:brightness-95 active:brightness-90',
      },
      size: {
        sm: 'h-6 px-2.5 text-body-sm',
        md: 'h-7 pl-2.5 pr-3 text-body',
        lg: 'h-9 px-5 text-body',
        icon: 'size-7 p-0',
      },
    },
    defaultVariants: { variant: 'secondary', size: 'md' },
  },
);

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement>, VariantProps<typeof buttonStyles> {
  loading?: boolean;
}

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  { className, variant, size, loading, disabled, children, type = 'button', ...rest },
  ref,
) {
  return (
    <button
      ref={ref}
      type={type}
      className={cn(buttonStyles({ variant, size }), className)}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...rest}
    >
      {loading && <CircleNotch weight="bold" className="size-4 animate-spin" aria-hidden />}
      {children}
    </button>
  );
});
