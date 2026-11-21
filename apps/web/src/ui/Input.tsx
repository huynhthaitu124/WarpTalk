import { forwardRef, type InputHTMLAttributes, type TextareaHTMLAttributes } from 'react';
import { cn } from '@/lib/cn';
import { useFieldProps } from './Field';

export const controlStyles =
  'w-full rounded-md border border-hairline-strong bg-surface-1 px-3 text-body text-ink placeholder:text-ink-tertiary transition-colors hover:border-hairline-tertiary focus-ring focus-visible:border-primary disabled:cursor-not-allowed disabled:opacity-50 aria-[invalid=true]:border-danger';

export const Input = forwardRef<HTMLInputElement, InputHTMLAttributes<HTMLInputElement>>(function Input(
  { className, ...rest }, ref,
) {
  const f = useFieldProps();
  return <input ref={ref} className={cn(controlStyles, 'h-9', className)} {...f} {...rest} />;
});

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaHTMLAttributes<HTMLTextAreaElement>>(function Textarea(
  { className, ...rest }, ref,
) {
  const f = useFieldProps();
  return <textarea ref={ref} className={cn(controlStyles, 'min-h-20 py-2 leading-relaxed', className)} {...f} {...rest} />;
});
