import { cn } from '@/lib/cn';

export function BrandMark({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 32 32" aria-hidden className={cn('size-6', className)}>
      <rect width="32" height="32" rx="8" fill="var(--primary)" />
      <path d="M7 11l4 10 5-8 5 8 4-10" fill="none" stroke="#fff" strokeWidth="2.4" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
export function Brand({ className }: { className?: string }) {
  return (
    <span className={cn('inline-flex items-center gap-2 text-body font-semibold tracking-[-0.2px] text-ink', className)}>
      <BrandMark />
      WarpTalk
    </span>
  );
}
