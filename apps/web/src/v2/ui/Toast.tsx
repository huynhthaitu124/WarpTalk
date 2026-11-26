import { createContext, useCallback, useContext, useMemo, useRef, useState, type ReactNode } from 'react';
import { CheckCircle, Info, Warning, X, XCircle } from '@phosphor-icons/react';
import { cn } from '@v2/lib/cn';

type Tone = 'info' | 'success' | 'warning' | 'danger';
interface ToastItem { id: number; title: string; description?: string; tone: Tone }

const Ctx = createContext<{ toast(t: Omit<ToastItem, 'id'>): void } | null>(null);

const ICON = { info: Info, success: CheckCircle, warning: Warning, danger: XCircle };
const COLOR = { info: 'text-info', success: 'text-success', warning: 'text-warning', danger: 'text-danger' };

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const seq = useRef(0);
  const dismiss = useCallback((id: number) => setItems((l) => l.filter((t) => t.id !== id)), []);
  const toast = useCallback((t: Omit<ToastItem, 'id'>) => {
    const id = ++seq.current;
    setItems((l) => [...l.slice(-3), { ...t, id }]);
    setTimeout(() => dismiss(id), t.tone === 'danger' ? 8000 : 4500);
  }, [dismiss]);
  const value = useMemo(() => ({ toast }), [toast]);
  return (
    <Ctx.Provider value={value}>
      {children}
      <div role="region" aria-live="polite" aria-label="Notifications" className="pointer-events-none fixed bottom-4 right-4 z-[80] flex w-80 max-w-[calc(100vw-32px)] flex-col gap-2">
        {items.map((t) => {
          const Icon = ICON[t.tone];
          return (
            <div key={t.id} role="status" data-testid="toast" className="pointer-events-auto flex animate-slide-up items-start gap-3 rounded-lg bg-popover p-3 shadow-pop">
              <Icon weight="fill" className={cn('mt-0.5 size-4 shrink-0', COLOR[t.tone])} aria-hidden />
              <div className="min-w-0 flex-1">
                <p className="text-body font-medium text-ink">{t.title}</p>
                {t.description && <p className="mt-0.5 text-body-sm text-ink-subtle">{t.description}</p>}
              </div>
              <button type="button" aria-label="Dismiss notification" onClick={() => dismiss(t.id)} className="rounded-md p-1 text-ink-subtle hover:bg-popover-hover hover:text-ink focus-ring">
                <X className="size-3.5" aria-hidden />
              </button>
            </div>
          );
        })}
      </div>
    </Ctx.Provider>
  );
}

export function useToast() {
  const c = useContext(Ctx);
  if (!c) throw new Error('useToast outside ToastProvider');
  return c.toast;
}
