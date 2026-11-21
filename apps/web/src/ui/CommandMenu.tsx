import { Command } from 'cmdk';
import * as RD from '@radix-ui/react-dialog';
import { Search } from 'lucide-react';
import { createContext, useCallback, useContext, useEffect, useMemo, useState, type ReactNode } from 'react';
import { Kbd } from './Misc';

export interface CommandItem {
  id: string;
  label: string;
  group: string;
  icon?: ReactNode;
  shortcut?: string;
  keywords?: string[];
  run(): void;
}

const Ctx = createContext<{ open: boolean; setOpen(v: boolean): void } | null>(null);

export function CommandMenuProvider({ items, children }: { items: CommandItem[]; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        setOpen((v) => !v);
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);
  const value = useMemo(() => ({ open, setOpen }), [open]);
  const groups = useMemo(() => {
    const m = new Map<string, CommandItem[]>();
    for (const i of items) m.set(i.group, [...(m.get(i.group) ?? []), i]);
    return [...m.entries()];
  }, [items]);
  const run = useCallback((i: CommandItem) => { setOpen(false); i.run(); }, []);
  return (
    <Ctx.Provider value={value}>
      {children}
      <RD.Root open={open} onOpenChange={setOpen}>
        <RD.Portal>
          <RD.Overlay className="fixed inset-0 z-50 animate-fade-in bg-overlay backdrop-blur-[2px]" />
          <RD.Content aria-label="Command menu" className="fixed left-1/2 top-[18vh] z-50 w-[calc(100vw-32px)] max-w-xl -translate-x-1/2 animate-pop-in overflow-hidden rounded-xl border border-hairline-strong bg-surface-2 shadow-pop focus:outline-none">
            <RD.Title className="sr-only">Command menu</RD.Title>
            <RD.Description className="sr-only">Type to search commands. Use arrow keys to navigate and Enter to run.</RD.Description>
            <Command label="Search commands" loop>
              <div className="flex items-center gap-2 border-b border-hairline px-4">
                <Search className="size-4 text-ink-subtle" aria-hidden />
                <Command.Input placeholder="Type a command or search…" className="h-12 flex-1 bg-transparent text-body text-ink outline-none placeholder:text-ink-tertiary" />
                <Kbd>esc</Kbd>
              </div>
              <Command.List className="max-h-80 overflow-y-auto p-2">
                <Command.Empty className="px-3 py-6 text-center text-body-sm text-ink-subtle">No results.</Command.Empty>
                {groups.map(([g, list]) => (
                  <Command.Group key={g} heading={g} className="[&_[cmdk-group-heading]]:px-2 [&_[cmdk-group-heading]]:py-1.5 [&_[cmdk-group-heading]]:text-eyebrow [&_[cmdk-group-heading]]:font-medium [&_[cmdk-group-heading]]:uppercase [&_[cmdk-group-heading]]:tracking-[0.4px] [&_[cmdk-group-heading]]:text-ink-subtle">
                    {list.map((i) => (
                      <Command.Item key={i.id} value={`${i.group} ${i.label}`} keywords={i.keywords} onSelect={() => run(i)} className="flex h-9 cursor-default items-center gap-2.5 rounded-md px-2 text-body text-ink data-[selected=true]:bg-surface-4">
                        {i.icon && <span className="text-ink-subtle" aria-hidden>{i.icon}</span>}
                        <span className="flex-1">{i.label}</span>
                        {i.shortcut && <Kbd>{i.shortcut}</Kbd>}
                      </Command.Item>
                    ))}
                  </Command.Group>
                ))}
              </Command.List>
            </Command>
          </RD.Content>
        </RD.Portal>
      </RD.Root>
    </Ctx.Provider>
  );
}

export function useCommandMenu() {
  const c = useContext(Ctx);
  if (!c) throw new Error('useCommandMenu outside CommandMenuProvider');
  return c;
}
