import { cn } from '@v2/lib/cn';

/**
 * The real WarpTalk mark (legacy `warptalk-brand.tsx`): black-on-white PNGs recoloured per theme by
 * the `.brand-img` rule in `styles/index.css`. `compact` is the square icon, otherwise the wordmark.
 */
export function Brand({ compact = false, className }: { compact?: boolean; className?: string }) {
  return (
    <span className={cn('relative block shrink-0 overflow-hidden', compact ? 'h-4 w-[18px]' : 'h-4 w-[65px]', className)}>
      <img
        src={compact ? '/brand/warptalk-sidebar-icon.png' : '/brand/warptalk-sidebar-logo.png'}
        alt="WarpTalk"
        className="brand-img absolute inset-0 size-full object-contain object-left"
        draggable={false}
      />
    </span>
  );
}
