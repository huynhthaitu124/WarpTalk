import { useMemo, useState } from 'react';
import { Link, Outlet, useLocation, useNavigate } from 'react-router-dom';
import {
  Check, ChevronsUpDown, DoorOpen, KeyRound, LogOut, Menu, Moon, Plus, Search, Settings, Sun, Video, X, Palette,
} from 'lucide-react';
import {
  Avatar, Brand, Button, CommandMenuProvider, Dropdown, DropdownContent, DropdownItem, DropdownLabel, DropdownSeparator,
  DropdownTrigger, Kbd, SidebarLink, SidebarNav, Tooltip, useCommandMenu, type CommandItem,
} from '@/ui';
import { useAuth, useProfile } from '@/lib/auth';
import { useTheme } from '@/lib/theme';
import { useWorkspace } from '@/lib/workspace';
import { cn } from '@/lib/cn';

function WorkspaceSwitcher() {
  const { workspaces, current, select } = useWorkspace();
  const navigate = useNavigate();
  return (
    <Dropdown>
      <DropdownTrigger asChild>
        <button
          type="button"
          aria-label={`Workspace: ${current?.name ?? 'none'}. Switch workspace`}
          className="flex h-10 w-full items-center gap-2.5 rounded-md px-2 text-left hover:bg-surface-2 focus-ring"
        >
          <Avatar name={current?.name ?? 'WarpTalk'} size="sm" />
          <span className="min-w-0 flex-1 truncate text-body font-medium text-ink" data-testid="current-workspace">
            {current?.name ?? 'No workspace'}
          </span>
          <ChevronsUpDown className="size-4 text-ink-subtle" aria-hidden />
        </button>
      </DropdownTrigger>
      <DropdownContent className="w-60">
        <DropdownLabel>Workspaces</DropdownLabel>
        {workspaces.map((w) => (
          <DropdownItem key={w.id} icon={w.id === current?.id ? <Check className="size-4" /> : <span className="size-4" />} onSelect={() => select(w.id)}>
            {w.name}
          </DropdownItem>
        ))}
        <DropdownSeparator />
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
        <button type="button" aria-label="Account menu" className="flex h-11 w-full items-center gap-2.5 rounded-md px-2 text-left hover:bg-surface-2 focus-ring">
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
        <DropdownItem icon={<LogOut className="size-4" />} onSelect={() => void logout()}>
          Sign out
        </DropdownItem>
      </DropdownContent>
    </Dropdown>
  );
}

function Sidebar({ onNavigate }: { onNavigate?: () => void }) {
  return (
    <div className="flex h-full flex-col gap-4 p-3" onClick={(e) => { if ((e.target as HTMLElement).closest('a')) onNavigate?.(); }}>
      <div className="flex items-center justify-between px-1 pt-1"><Brand /></div>
      <WorkspaceSwitcher />
      <SidebarNav>
        <SidebarLink to="/rooms" icon={<DoorOpen className="size-4" />}>Rooms</SidebarLink>
        <SidebarLink to="/meetings" icon={<Video className="size-4" />}>Meetings</SidebarLink>
        <SidebarLink to="/join" icon={<KeyRound className="size-4" />}>Join by code</SidebarLink>
        <SidebarLink to="/settings" icon={<Settings className="size-4" />}>Settings</SidebarLink>
      </SidebarNav>
      <div className="mt-auto flex flex-col gap-1">
        <SidebarNav label="Resources">
          <SidebarLink to="/design" icon={<Palette className="size-4" />}>Design system</SidebarLink>
        </SidebarNav>
        <AccountMenu />
      </div>
    </div>
  );
}

function TopBar({ onMenu }: { onMenu: () => void }) {
  const { setOpen } = useCommandMenu();
  const { theme, toggle } = useTheme();
  const loc = useLocation();
  const crumb = loc.pathname.split('/')[1] || 'rooms';
  return (
    <header className="flex h-12 shrink-0 items-center gap-3 border-b border-hairline bg-canvas px-4">
      <Button variant="ghost" size="icon" className="md:hidden" aria-label="Open navigation" onClick={onMenu}><Menu className="size-4" aria-hidden /></Button>
      <span className="hidden text-body font-medium capitalize text-ink-muted md:block">{crumb}</span>
      <div className="ml-auto flex items-center gap-2">
        <button
          type="button"
          onClick={() => setOpen(true)}
          aria-label="Open command menu"
          className="flex h-8 w-56 items-center gap-2 rounded-md border border-hairline-strong bg-surface-1 px-2.5 text-body-sm text-ink-subtle hover:border-hairline-tertiary focus-ring max-sm:w-auto"
        >
          <Search className="size-4" aria-hidden />
          <span className="flex-1 text-left max-sm:hidden">Search or run a command…</span>
          <Kbd>⌘K</Kbd>
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

export function Shell() {
  const navigate = useNavigate();
  const { logout } = useAuth();
  const { workspaces, select } = useWorkspace();
  const { toggle } = useTheme();
  const [drawer, setDrawer] = useState(false);

  const items = useMemo<CommandItem[]>(() => [
    { id: 'go-rooms', group: 'Navigate', label: 'Go to Rooms', icon: <DoorOpen className="size-4" />, shortcut: 'G R', run: () => navigate('/rooms') },
    { id: 'go-meetings', group: 'Navigate', label: 'Go to Meetings', icon: <Video className="size-4" />, run: () => navigate('/meetings') },
    { id: 'go-join', group: 'Navigate', label: 'Join a room by code', icon: <KeyRound className="size-4" />, run: () => navigate('/join') },
    { id: 'go-settings', group: 'Navigate', label: 'Go to Settings', icon: <Settings className="size-4" />, run: () => navigate('/settings') },
    { id: 'go-design', group: 'Navigate', label: 'Open design system', icon: <Palette className="size-4" />, run: () => navigate('/design') },
    { id: 'new-room', group: 'Actions', label: 'Create room', icon: <Plus className="size-4" />, shortcut: 'C', run: () => navigate('/rooms?new=1') },
    { id: 'new-ws', group: 'Actions', label: 'Create workspace', icon: <Plus className="size-4" />, run: () => navigate('/workspaces?new=1') },
    { id: 'theme', group: 'Actions', label: 'Toggle theme', icon: <Sun className="size-4" />, run: toggle },
    ...workspaces.map((w) => ({ id: `ws-${w.id}`, group: 'Switch workspace', label: w.name, run: () => select(w.id) })),
    { id: 'logout', group: 'Account', label: 'Sign out', icon: <LogOut className="size-4" />, run: () => void logout() },
  ], [navigate, toggle, workspaces, select, logout]);

  return (
    <CommandMenuProvider items={items}>
      <div className="flex h-dvh bg-canvas">
        <aside className="hidden w-60 shrink-0 border-r border-hairline bg-canvas md:block" aria-label="Sidebar">
          <Sidebar />
        </aside>
        {drawer && (
          <div className="fixed inset-0 z-40 md:hidden">
            <button type="button" aria-label="Close navigation" className="absolute inset-0 bg-overlay" onClick={() => setDrawer(false)} />
            <aside className={cn('absolute inset-y-0 left-0 w-64 animate-fade-in border-r border-hairline bg-canvas')} aria-label="Sidebar">
              <Button variant="ghost" size="icon" className="absolute right-2 top-2" aria-label="Close menu" onClick={() => setDrawer(false)}><X className="size-4" aria-hidden /></Button>
              <Sidebar onNavigate={() => setDrawer(false)} />
            </aside>
          </div>
        )}
        <div className="flex min-w-0 flex-1 flex-col">
          <TopBar onMenu={() => setDrawer(true)} />
          <main id="main" className="min-h-0 flex-1 overflow-y-auto bg-panel">
            <div className="mx-auto max-w-6xl px-6 py-8 max-sm:px-4">
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
