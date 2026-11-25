import { useId, type ReactNode } from 'react';
import { cn } from '@/lib/cn';

/** Flags drawn as SVG (emoji flags do not render on Windows and would leak into text content). */
function Art({ code }: { code: string }) {
  const id = useId();
  switch (code) {
    case 'vi':
      return (
        <svg viewBox="0 0 30 20" preserveAspectRatio="xMidYMid slice" className="size-full">
          <rect width="30" height="20" fill="#DA251D" />
          <polygon fill="#FFDD00" points="15,4 16.35,8.15 20.71,8.15 17.18,10.71 18.53,14.85 15,12.29 11.47,14.85 12.82,10.71 9.29,8.15 13.65,8.15" />
        </svg>
      );
    case 'en':
      return (
        <svg viewBox="0 0 60 30" preserveAspectRatio="xMidYMid slice" className="size-full">
          <clipPath id={id}><path d="M30,15 h30 v15 z v15 h-30 z h-30 v-15 z v-15 h30 z" /></clipPath>
          <rect width="60" height="30" fill="#012169" />
          <path d="M0,0 L60,30 M60,0 L0,30" stroke="#fff" strokeWidth="6" />
          <path d="M0,0 L60,30 M60,0 L0,30" clipPath={`url(#${id})`} stroke="#C8102E" strokeWidth="4" />
          <path d="M30,0 v30 M0,15 h60" stroke="#fff" strokeWidth="10" />
          <path d="M30,0 v30 M0,15 h60" stroke="#C8102E" strokeWidth="6" />
        </svg>
      );
    case 'ja':
      return (
        <svg viewBox="0 0 30 20" preserveAspectRatio="xMidYMid slice" className="size-full">
          <rect width="30" height="20" fill="#fff" />
          <circle cx="15" cy="10" r="6" fill="#BC002D" />
        </svg>
      );
    default:
      return <span className="grid size-full place-items-center bg-surface-3 text-[8px] font-semibold uppercase leading-none text-ink-muted">{code.slice(0, 2)}</span>;
  }
}

export function Flag({ code, className }: { code: string; className?: string }) {
  return (
    <span aria-hidden className={cn('inline-block h-3.5 w-5 shrink-0 overflow-hidden rounded-[3px] shadow-[0_0_0_1px_rgba(0,0,0,0.08)]', className)}>
      <Art code={code} />
    </span>
  );
}

/** Label with its flag in front, for menus and inline text. */
export function FlagLabel({ code, children }: { code: string; children: ReactNode }) {
  return <span className="inline-flex min-w-0 items-center gap-2"><Flag code={code} /><span className="truncate">{children}</span></span>;
}
