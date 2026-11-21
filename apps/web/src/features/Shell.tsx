import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { Link, Outlet, useLocation, useNavigate } from 'react-router-dom';
import * as RD from '@radix-ui/react-dialog';
import {
  Brain, Broadcast, Buildings, Check, CaretRight, CaretUpDown, GearSix, House, Keyboard, List, MagnifyingGlass, Moon, Palette, Plus, SignOut,
  SquaresFour, Sun, Users, X,
} from '@phosphor-icons/react';
import {
  Avatar, Brand, Button, CommandMenuProvider, Dropdown, DropdownContent, DropdownItem, DropdownLabel, DropdownSeparator,
  DropdownTrigger, Kbd, SidebarLink, SidebarNav, Tooltip, useCommandMenu, type CommandItem,
} from '@/ui';
import { useAuth, useProfile } from '@/lib/auth';
import { useTheme } from '@/lib/theme';
import { canAdmin, useWorkspace } from '@/lib/workspace';
import { usePendingInvitations, useRoom } from '@/lib/queries';

const SHOW_DESIGN = import.meta.env.DEV;

function WorkspaceSwitcher() {
  const { workspaces, current, select } = useWorkspace();
  const navigate = useNavigate();
  return (
    <Dropdown>
      <DropdownTrigger asChild>
        <button
          type="button"
          data-testid="workspace-switcher"
          className="flex h-10 w-full items-center gap-2.5 rounded-md px-2 text-left hover:bg-surface-2 focus-ring"
        >
          <Avatar name={current?.name ?? 'WarpTalk'} size="sm" />
          <span className="min-w-0 flex-1 truncate text-body font-medium text-ink" data-testid="current-workspace">
            {current?.name ?? 'No workspace'}
          </span>
          <CaretUpDown className="size-4 text-ink-subtle" aria-hidden />
        </button>
      </DropdownTrigger>
      <DropdownContent className="w-60">
        <DropdownLabel>Workspaces</DropdownLabel>
        {workspaces.map((w) => (
          <DropdownItem key={w.id} icon={w.id === current?.id ? <Check weight="bold" className="size-4" /> : <span className="size-4" />} onSelect={() => select(w.id)}>
            {w.name}
          </DropdownItem>
        ))}
        <DropdownSeparator />
        <DropdownItem icon={<Buildings weight="duotone" className="size-4" />} onSelect={() => navigate('/workspaces')}>
          Manage workspaces
        </DropdownItem>
        <DropdownItem icon={<Plus className="size-4" />} onSelect={() => navigate('/workspaces?new=1')}>
          Create workspace
        </DropdownItem>
      </DropdownContent>
    </Dropdown>
  );
}

function AccountMenu() {
  const { email, logout } = useAuth();
  const profile = useProfile();
  const { theme, toggle } = useTheme();
  const name = profile.data?.fullName ?? email ?? 'Account';
  return (
    <Dropdown>
      <DropdownTrigger asChild>
        <button type="button" data-testid="account-menu" className="flex h-11 w-full items-center gap-2.5 rounded-md px-2 text-left hover:bg-surface-2 focus-ring">
          <Avatar name={name} size="sm" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-body-sm font-medium text-ink" data-testid="account-name">{name}</span>
            <span className="block truncate text-caption text-ink-subtle">{email}</span>
          </span>
        </button>
      </DropdownTrigger>
      <DropdownContent side="top" className="w-56">
        <DropdownItem icon={theme === 'dark' ? <Sun className="size-4" /> : <Moon className="size-4" />} onSelect={toggle}>
          {theme === 'dark' ? 'Light theme' : 'Dark theme'}
        </DropdownItem>
        <DropdownSeparator />
        <DropdownItem icon={<SignOut className="size-4" />} onSelect={() => void logout()}>
          Sign out
        </DropdownItem>
      </DropdownContent>
    </Dropdown>
  );
}

const ico = (I: typeof House) => <I weight="duotone" size={16} />;

function Sidebar() {
  const { current, workspaces, loading, wsPath } = useWorkspace();
  const { userId } = useAuth();
  const pending = usePendingInvitations(userId);
  const guest = !loading && workspaces.length === 0;
  const pendingCount = pending.data?.length ?? 0;
  return (
    <div className="flex h-full flex-col gap-4 p-3">
      <div className="flex items-center px-1 pt-1"><Brand /></div>
      {guest ? (
        // A person who only joined rooms by code has no workspace: no switcher, no workspace nav.
        <SidebarNav label="Primary">
          <SidebarLink to="/workspaces" icon={ico(Buildings)} badge={pendingCount || null}>Workspaces</SidebarLink>
          <SidebarLink to="/join" icon={ico(Keyboard)}>Join by code</SidebarLink>
        </SidebarNav>
      ) : (
        <>
          <WorkspaceSwitcher />
          <SidebarNav label="Primary">
            <SidebarLink to={wsPath('/home')} icon={ico(House)}>Home</SidebarLink>
            <SidebarLink
              to={wsPath('/meetings')}
              icon={ico(SquaresFour)}
              actions={[
                { label: 'Join by code', icon: <Keyboard size={14} weight="bold" />, to: `${wsPath('/meetings')}?join=1` },
                { label: 'New meeting', icon: <Plus size={14} weight="bold" />, to: `${wsPath('/meetings')}?new=1` },
              ]}
            >
              Meetings
            </SidebarLink>
            <SidebarLink to="/external" icon={ico(Broadcast)}>External meeting</SidebarLink>
          </SidebarNav>
          <SidebarNav label="Workspace" heading="Workspace">
            <SidebarLink to={wsPath('/members')} icon={ico(Users)} badge={pendingCount || null}>Members</SidebarLink>
            <SidebarLink to={wsPath('/knowledge')} icon={ico(Brain)}>Knowledge</SidebarLink>
            <SidebarLink to={wsPath('/settings')} icon={ico(GearSix)}>Settings</SidebarLink>
          </SidebarNav>
        </>
      )}
      <div className="mt-auto flex flex-col gap-1">
        {SHOW_DESIGN && (
          <SidebarNav label="Resources">
            <SidebarLink to="/design" icon={ico(Palette)}>Design system</SidebarLink>
          </SidebarNav>
        )}
        <AccountMenu />
      </div>
    </div>
  );
}

function Crumb({ children, to }: { children: ReactNode; to?: string }) {
  return to ? <Link to={to} className="truncate text-ink-subtle hover:text-ink">{children}</Link> : <span className="truncate text-ink" aria-current="page">{children}</span>;
}

const SETTINGS_LABEL: Record<string, string> = { general: 'General', ai: 'AI providers', advanced: 'Advanced', account: 'Account' };

/** Real breadcrumb: `Meetings / Board review`, `Settings / AI providers`, ... */
function Breadcrumb() {
  const loc = useLocation();
  const { current, wsPath } = useWorkspace();
  const parts = loc.pathname.split('/').filter(Boolean);
  const inWs = !!current && parts[0] === current.slug;
  const rest = inWs ? parts.slice(1) : parts;
  const roomId = (rest[0] === 'rooms' || rest[0] === 'meetings') && rest[1] ? rest[1] : undefined;
  const room = useRoom(roomId);

  const items: { label: ReactNode; to?: string }[] = [];
  const top = rest[0] ?? 'home';
  if (roomId) {
    if (current) items.push({ label: 'Meetings', to: wsPath('/meetings') });
    items.push({ label: <span title={room.data?.title}>{room.data?.title ?? 'Meeting'}</span> });
  } else if (top === 'settings') {
    items.push({ label: 'Settings', to: rest[1] ? wsPath('/settings') : undefined });
    if (rest[1]) items.push({ label: SETTINGS_LABEL[rest[1]] ?? rest[1] });
  } else {
    const label = { home: 'Home', meetings: 'Meetings', rooms: 'Meetings', members: 'Members', knowledge: 'Knowledge', external: 'External meeting', join: 'Join by code', workspaces: 'Workspaces' }[top] ?? top;
    items.push({ label });
  }
  return (
    <nav aria-label="Breadcrumb" className="min-w-0 flex-1">
      <ol className="flex min-w-0 items-center gap-1.5 text-body font-medium">
        {items.map((it, i) => (
          <li key={i} className="flex min-w-0 items-center gap-1.5 last:flex-1 max-sm:[&:not(:last-child)]:hidden">
            {i > 0 && <CaretRight size={12} className="shrink-0 text-ink-tertiary max-sm:hidden" aria-hidden />}
            <Crumb to={i < items.length - 1 ? it.to : undefined}>{it.label}</Crumb>
          </li>
        ))}
      </ol>
    </nav>
  );
}

function TopBar({ onMenu }: { onMenu: () => void }) {
  const { setOpen } = useCommandMenu();
  const { theme, toggle } = useTheme();
  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-hairline bg-canvas px-4">
      <Button variant="ghost" size="icon" className="md:hidden" aria-label="Open navigation" onClick={onMenu}><List className="size-4" aria-hidden /></Button>
      <Breadcrumb />
      <div className="ml-auto flex shrink-0 items-center gap-2">
        <button
          type="button"
          onClick={() => setOpen(true)}
          className="flex h-8 w-64 items-center gap-2 whitespace-nowrap rounded-md border border-control bg-surface-1 px-2.5 text-body-sm text-ink-subtle hover:border-ink-subtle focus-ring max-md:w-auto"
        >
          <MagnifyingGlass className="size-4" aria-hidden />
          <span className="flex-1 text-left max-md:hidden">Search or run a command…</span>
          <span className="sr-only md:hidden">Search</span>
          <Kbd className="max-sm:hidden">⌘K</Kbd>
        </button>
        <Tooltip content={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}>
          <Button variant="ghost" size="icon" aria-label="Toggle theme" onClick={toggle}>
            {theme === 'dark' ? <Sun className="size-4" aria-hidden /> : <Moon className="size-4" aria-hidden />}
          </Button>
        </Tooltip>
      </div>
    </header>
  );
}

/** Mobile navigation: a Radix dialog (focus trap, Escape, focus return, scroll lock). */
function MobileDrawer({ open, onOpenChange }: { open: boolean; onOpenChange(v: boolean): void }) {
  const loc = useLocation();
  // Navigating (or widening the window to desktop) closes it.
  useEffect(() => { onOpenChange(false); }, [loc.pathname, loc.search, onOpenChange]);
  useEffect(() => {
    const mq = window.matchMedia('(min-width: 768px)');
    const f = () => { if (mq.matches) onOpenChange(false); };
    mq.addEventListener('change', f);
    return () => mq.removeEventListener('change', f);
  }, [onOpenChange]);
  return (
    <RD.Root open={open} onOpenChange={onOpenChange}>
      <RD.Portal>
        <RD.Overlay className="fixed inset-0 z-40 animate-fade-in bg-overlay md:hidden" />
        <RD.Content aria-describedby={undefined} className="fixed inset-y-0 left-0 z-40 w-64 max-w-[85vw] animate-fade-in border-r border-hairline bg-canvas focus:outline-none md:hidden">
          <RD.Title className="sr-only">Navigation</RD.Title>
          <RD.Close asChild>
            <Button variant="ghost" size="icon" className="absolute right-2 top-2" aria-label="Close menu"><X className="size-4" aria-hidden /></Button>
          </RD.Close>
          <Sidebar />
        </RD.Content>
      </RD.Portal>
    </RD.Root>
  );
}

export function Shell() {
  const navigate = useNavigate();
  const { logout } = useAuth();
  const { workspaces, current, select, wsPath } = useWorkspace();
  const { toggle } = useTheme();
  const [drawer, setDrawer] = useState(false);
  const admin = canAdmin(current);

  const items = useMemo<CommandItem[]>(() => {
    const hasWs = workspaces.length > 0;
    const nav = (id: string, label: string, icon: ReactNode, to: string, shortcut?: string): CommandItem => ({ id, group: 'Navigate', label, icon, shortcut, run: () => navigate(to) });
    return [
      ...(hasWs ? [
        nav('go-home', 'Go to Home', <House className="size-4" />, wsPath('/home'), 'G H'),
        nav('go-meetings', 'Go to Meetings', <SquaresFour className="size-4" />, wsPath('/meetings'), 'G M'),
        nav('go-members', 'Go to Members', <Users className="size-4" />, wsPath('/members'), 'G P'),
        nav('go-knowledge', 'Go to Knowledge', <Brain className="size-4" />, wsPath('/knowledge'), 'G K'),
        nav('go-settings', 'Go to Settings', <GearSix className="size-4" />, wsPath('/settings'), 'G S'),
        ...(admin ? [nav('go-ai', 'Go to AI providers', <Brain className="size-4" />, wsPath('/settings/ai'))] : []),
        nav('go-external', 'Go to External meeting', <Broadcast className="size-4" />, '/external', 'G E'),
      ] : []),
      nav('go-workspaces', 'Manage workspaces', <Buildings className="size-4" />, '/workspaces'),
      ...(SHOW_DESIGN ? [nav('go-design', 'Open design system', <Palette className="size-4" />, '/design')] : []),
      ...(hasWs ? [
        { id: 'new-room', group: 'Actions', label: 'Create meeting', icon: <Plus className="size-4" />, shortcut: 'C', run: () => navigate(`${wsPath('/meetings')}?new=1`) },
        { id: 'join-code', group: 'Actions', label: 'Join a meeting by code', icon: <Keyboard className="size-4" />, shortcut: 'J', run: () => navigate(`${wsPath('/meetings')}?join=1`) },
      ] : [{ id: 'join-code', group: 'Actions', label: 'Join a meeting by code', icon: <Keyboard className="size-4" />, shortcut: 'J', run: () => navigate('/join') }]),
      { id: 'new-ws', group: 'Actions', label: 'Create workspace', icon: <Plus className="size-4" />, run: () => navigate('/workspaces?new=1') },
      { id: 'theme', group: 'Actions', label: 'Toggle theme', icon: <Sun className="size-4" />, run: toggle },
      ...workspaces.map((w) => ({ id: `ws-${w.id}`, group: 'Switch workspace', label: w.name, run: () => select(w.id) })),
      { id: 'logout', group: 'Account', label: 'Sign out', icon: <SignOut className="size-4" />, run: () => void logout() },
    ];
  }, [navigate, toggle, workspaces, select, logout, wsPath, admin]);

  return (
    <CommandMenuProvider items={items}>
      <div className="flex h-dvh bg-canvas">
        <aside className="hidden w-60 shrink-0 border-r border-hairline bg-canvas md:block" aria-label="Sidebar">
          <Sidebar />
        </aside>
        <MobileDrawer open={drawer} onOpenChange={setDrawer} />
        <div className="flex min-w-0 flex-1 flex-col">
          <TopBar onMenu={() => setDrawer(true)} />
          <main id="main" className="min-h-0 flex-1 overflow-y-auto overflow-x-hidden bg-panel">
            <div className="mx-auto min-w-0 max-w-6xl px-6 py-8 max-sm:px-4 max-sm:py-6">
              <Outlet />
            </div>
          </main>
        </div>
      </div>
    </CommandMenuProvider>
  );
}

export function SkipLink() {
  return (
    <a href="#main" className="sr-only focus:not-sr-only focus:fixed focus:left-3 focus:top-3 focus:z-[90] focus:rounded-md focus:bg-primary focus:px-3 focus:py-2 focus:text-on-primary">
      Skip to content
    </a>
  );
}

export { Link };
