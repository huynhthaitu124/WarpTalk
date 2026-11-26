import { createContext, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, Outlet, useLocation, useNavigate } from 'react-router-dom';
import * as RD from '@radix-ui/react-dialog';
import {
  ArrowLeft, BookBookmark, Brain, Broadcast, Buildings, Check, CaretDown, CaretRight, CreditCard, FileText, GearSix, House, Keyboard, List, MagnifyingGlass, Moon, NotePencil, Palette, Plus, SidebarSimple, SignOut,
  SquaresFour, Sparkle, Sun, Users, X,
  ListChecks, ChartBar,
} from '@phosphor-icons/react';
import {
  Avatar, Brand, Button, CommandMenuProvider, Dropdown, DropdownContent, DropdownItem, DropdownLabel, DropdownSeparator,
  DropdownTrigger, Kbd, SidebarLink, SidebarNav, Tooltip, useCommandMenu, type CommandItem,
} from '@v2/ui';
import { useAuth, useProfile } from '@v2/lib/auth';
import { useTheme } from '@v2/lib/theme';
import { IS_DEV } from '@v2/lib/env';
import { canAdmin, useWorkspace } from '@v2/lib/workspace';
import { usePendingInvitations, useRoom } from '@v2/lib/queries';
import { useFeatures } from '@v2/lib/features';
import { useSession } from '@v2/lib/copilot';
import { cn } from '@v2/lib/cn';
import { QUALITY, TASKS } from '@v2/lib/strings';
import { visibleSettingsSections } from './settingsNav';
import { BillingBanner } from './Billing';

const SHOW_DESIGN = IS_DEV;

/** Legacy sidebar header: 20px gradient tile + 14px/600 workspace name + caret, search icon at the right. */
function WorkspaceSwitcher() {
  const { workspaces, current, select } = useWorkspace();
  const navigate = useNavigate();
  const name = current?.name ?? 'WarpTalk';
  return (
    <Dropdown>
      <DropdownTrigger asChild>
        <button
          type="button"
          data-testid="workspace-switcher"
          className="-ml-1.5 flex min-w-0 items-center gap-2 rounded-md px-1.5 py-1 text-left hover:bg-surface-3/60 focus-ring"
        >
          <span aria-hidden className="grid size-5 shrink-0 place-items-center rounded-md bg-gradient-to-br from-pink-500 to-rose-500 text-[9px] font-bold text-white">{name.replace(/[^A-Za-z0-9 ]/g, '').split(/\s+/).filter(Boolean).slice(0, 2).map((w) => w[0]!.toUpperCase()).join('') || 'W'}</span>
          <span className="min-w-0 truncate text-[14px] font-semibold tracking-[-0.3px] text-ink" data-testid="current-workspace">
            {current?.name ?? 'No workspace'}
          </span>
          <CaretDown className="size-3.5 shrink-0 text-ink-subtle" aria-hidden />
        </button>
      </DropdownTrigger>
      <DropdownContent className="w-60" align="start">
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

/** Desktop sidebar collapse state (the toggle lives in the sidebar header; the top bar re-opens it). */
const SidebarCollapse = createContext<{ collapsed: boolean; setCollapsed(v: boolean): void }>({ collapsed: false, setCollapsed: () => undefined });

function SidebarCollapseButton() {
  const { setCollapsed } = useContext(SidebarCollapse);
  return (
    <Tooltip content="Collapse sidebar">
      <button type="button" aria-label="Collapse sidebar" onClick={() => setCollapsed(true)} className="grid size-7 shrink-0 place-items-center rounded-md text-ink-muted hover:bg-surface-3/60 hover:text-ink focus-ring max-md:hidden">
        <SidebarSimple className="size-4" aria-hidden />
      </button>
    </Tooltip>
  );
}

function AccountMenu() {
  const { email, logout } = useAuth();
  const profile = useProfile();
  const { theme, toggle } = useTheme();
  const { current } = useWorkspace();
  const name = profile.data?.fullName ?? email ?? 'Account';
  const role = current ? current.myRole[0] + current.myRole.slice(1).toLowerCase() : null;
  return (
    <Dropdown>
      <DropdownTrigger asChild>
        <button type="button" data-testid="account-menu" className="flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left hover:bg-surface-3/60 focus-ring">
          <Avatar name={name} className="size-6 text-[9px]" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[12.5px] font-medium leading-tight text-ink" data-testid="account-name">{name}</span>
            <span className="block truncate text-[11px] leading-tight text-ink-muted">{role ? `${role} · ${email}` : email}</span>
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

const ico = (I: typeof House) => <I size={16} />;

/** Settings replaces the workspace nav (same canvas layer): "Back to app", uppercase group labels. */
function SettingsSidebar() {
  const { current, wsPath } = useWorkspace();
  const profile = useProfile();
  const loc = useLocation();
  const admin = canAdmin(current);
  const visible = visibleSettingsSections(admin, !!profile.data?.systemAdmin);
  const section = loc.pathname.split('/').filter(Boolean)[2];
  return (
    <>
      <div className="flex h-12 shrink-0 items-center px-3">
        <Link to={current ? wsPath('/home') : '/workspaces'} className="-ml-1.5 flex items-center gap-2 rounded-md px-1.5 py-1 text-body font-medium text-ink-muted no-underline hover:bg-surface-3/60 hover:text-ink hover:no-underline focus-ring">
          <ArrowLeft className="size-4" aria-hidden />Back to app
        </Link>
      </div>
      {current && <p className="truncate px-5 pb-1 text-body-sm text-ink-muted" data-testid="current-workspace" title={current.name}>{current.name}</p>}
      <nav aria-label="Settings sections" className="flex flex-col px-3 pb-3 pt-1">
        {visible.map((s, i) => (
          <div key={s.id} className="flex flex-col gap-px">
            {(i === 0 || visible[i - 1]!.group !== s.group) && <p className={cn('flex h-6 items-center px-2 text-body-sm font-medium uppercase tracking-[0.6px] text-ink-subtle', i > 0 && 'mt-4', 'mb-1')}>{s.group}</p>}
            <SidebarLink to={wsPath(`/settings/${s.id}`)} icon={<s.icon size={16} />} active={s.id === section}>{s.label}</SidebarLink>
          </div>
        ))}
      </nav>
    </>
  );
}

function Sidebar() {
  const { workspaces, loading, wsPath, current } = useWorkspace();
  const { userId } = useAuth();
  const loc = useLocation();
  const pending = usePendingInvitations(userId);
  const guest = !loading && workspaces.length === 0;
  const pendingCount = pending.data?.length ?? 0;
  const { features } = useFeatures();
  const inSettings = !guest && /^\/[^/]+\/settings(\/|$)/.test(loc.pathname);
  return (
    <div className="flex h-full flex-col">
      {inSettings ? <SettingsSidebar /> : (
        <>
          <div className="flex h-12 shrink-0 items-center justify-between gap-2 px-3">
            {guest ? <Brand /> : <WorkspaceSwitcher />}
            <div className="flex shrink-0 items-center gap-1">
              <SidebarCollapseButton />
              {!guest && (
                <Link
                  to={`${wsPath('/meetings')}?new=1`}
                  aria-label="New meeting"
                  title="New meeting"
                  className="grid size-8 place-items-center rounded-full border border-hairline/60 bg-surface-1 text-ink shadow-sm hover:bg-surface-2 focus-ring"
                >
                  <NotePencil className="size-4" aria-hidden />
                </Link>
              )}
            </div>
          </div>
          <div className="flex flex-col gap-5 overflow-y-auto px-3 pb-3 pt-1">
            {guest ? (
              // A person who only joined rooms by code has no workspace: no switcher, no workspace nav.
              <SidebarNav label="Primary">
                <SidebarLink to="/workspaces" icon={ico(Buildings)} badge={pendingCount || null}>Workspaces</SidebarLink>
                {features.translationMeetings && <SidebarLink to="/join" icon={ico(Keyboard)}>Join by code</SidebarLink>}
              </SidebarNav>
            ) : (
              <>
                <SidebarNav label="Primary">
                  <SidebarLink to={wsPath('/home')} icon={ico(House)}>Home</SidebarLink>
                  {features.copilot && (
                    <SidebarLink
                      to={wsPath('/sessions')}
                      icon={ico(Sparkle)}
                      actions={[{ label: 'Start copilot session', icon: <Plus size={14} weight="bold" />, to: `${wsPath('/sessions')}?new=1` }]}
                    >
                      Copilot
                    </SidebarLink>
                  )}
                  <SidebarLink to={wsPath('/tasks')} icon={ico(ListChecks)}>{TASKS.nav}</SidebarLink>
                  {features.translationMeetings && (
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
                  )}
                  {features.externalBridge && <SidebarLink to="/external" icon={ico(Broadcast)}>External meeting</SidebarLink>}
                </SidebarNav>
                <SidebarNav label="Workspace" heading="Workspace">
                  <SidebarLink to={wsPath('/members')} icon={ico(Users)} badge={pendingCount || null}>Members</SidebarLink>
                  <SidebarLink to={wsPath('/knowledge')} icon={ico(Brain)}>Knowledge</SidebarLink>
                  <SidebarLink to={wsPath('/documents')} icon={ico(FileText)}>Documents</SidebarLink>
                  <SidebarLink to={wsPath('/glossary')} icon={ico(BookBookmark)}>Glossary</SidebarLink>
                  {canAdmin(current) && <SidebarLink to={wsPath('/quality')} icon={ico(ChartBar)}>{QUALITY.nav}</SidebarLink>}
                  <SidebarLink to={wsPath('/billing')} icon={ico(CreditCard)}>Billing</SidebarLink>
                  <SidebarLink to={wsPath('/settings')} icon={ico(GearSix)}>Settings</SidebarLink>
                </SidebarNav>
              </>
            )}
          </div>
        </>
      )}
      <div className="mt-auto flex flex-col gap-2 p-3">
        <AccountMenu />
      </div>
    </div>
  );
}

function Crumb({ children, to }: { children: ReactNode; to?: string }) {
  return to ? <Link to={to} className="truncate text-ink-muted hover:text-ink">{children}</Link> : <span className="truncate text-ink" aria-current="page">{children}</span>;
}

const SETTINGS_LABEL: Record<string, string> = { general: 'General', ai: 'AI providers', advanced: 'Advanced', account: 'Account', 'global-glossary': 'Global glossary' };

/** Real breadcrumb: `Meetings / Board review`, `Settings / AI providers`, ... */
function Breadcrumb() {
  const loc = useLocation();
  const { current, wsPath } = useWorkspace();
  const parts = loc.pathname.split('/').filter(Boolean);
  const inWs = !!current && parts[0] === current.slug;
  const rest = inWs ? parts.slice(1) : parts;
  const roomId = (rest[0] === 'rooms' || rest[0] === 'meetings') && rest[1] ? rest[1] : undefined;
  const room = useRoom(roomId);
  const sess = useSession(rest[0] === 'sessions' ? rest[1] : undefined);

  const items: { label: ReactNode; to?: string }[] = [];
  const top = rest[0] ?? 'home';
  if (rest[0] === 'sessions' && rest[1]) {
    if (current) items.push({ label: 'Sessions', to: wsPath('/sessions') });
    items.push({ label: <span title={sess.data?.title}>{sess.data?.title ?? 'Session'}</span> });
  } else if (roomId) {
    if (current) items.push({ label: 'Meetings', to: wsPath('/meetings') });
    items.push({ label: <span title={room.data?.title}>{room.data?.title ?? 'Meeting'}</span> });
  } else if (top === 'settings') {
    items.push({ label: 'Settings', to: rest[1] ? wsPath('/settings') : undefined });
    if (rest[1]) items.push({ label: SETTINGS_LABEL[rest[1]] ?? rest[1] });
  } else {
    const label = { home: 'Home', sessions: 'Sessions', meetings: 'Meetings', rooms: 'Meetings', members: 'Members', billing: 'Billing', knowledge: 'Knowledge', documents: 'Documents', glossary: 'Glossary', tasks: TASKS.nav, quality: QUALITY.nav, external: 'External meeting', join: 'Join by code', workspaces: 'Workspaces' }[top] ?? top;
    items.push({ label });
  }
  const pageTitle = items.map((i) => (typeof i.label === 'string' ? i.label : '')).filter(Boolean).at(-1)
    ?? (rest[0] === 'sessions' && rest[1] ? sess.data?.title : undefined) ?? room.data?.title;
  useEffect(() => { document.title = pageTitle ? `${pageTitle} · WarpTalk` : 'WarpTalk'; }, [pageTitle]);
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

function TopBar({ onMenu }: { onMenu: (opener: HTMLElement) => void }) {
  const { setOpen } = useCommandMenu();
  const { theme, toggle } = useTheme();
  const { collapsed, setCollapsed } = useContext(SidebarCollapse);
  return (
    <header className="flex h-11 shrink-0 items-center gap-3 border-b border-hairline px-4 md:grid md:grid-cols-[1fr_minmax(0,420px)_1fr]">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <Button variant="ghost" size="icon" className="md:hidden" aria-label="Open navigation" onClick={(e) => onMenu(e.currentTarget)}><List className="size-4" aria-hidden /></Button>
        {collapsed && <Button variant="ghost" size="icon" className="max-md:hidden" aria-label="Expand sidebar" onClick={() => setCollapsed(false)}><SidebarSimple className="size-4" aria-hidden /></Button>}
        <Breadcrumb />
      </div>
      <button
        type="button"
        onClick={() => setOpen(true)}
        className="flex h-7 w-full items-center gap-2 whitespace-nowrap rounded-md border border-hairline bg-surface-1 px-2.5 text-body-sm text-ink-muted hover:border-hairline-strong focus-ring max-md:w-auto"
      >
        <MagnifyingGlass className="size-3.5" aria-hidden />
        <span className="flex-1 text-left max-md:hidden">Search or run a command…</span>
        <span className="sr-only md:hidden">Search</span>
        <Kbd className="max-sm:hidden">⌘K</Kbd>
      </button>
      <div className="flex shrink-0 items-center justify-end gap-2">
        <Tooltip content={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}>
          <button
            type="button"
            aria-label="Toggle theme"
            onClick={toggle}
            className="grid size-6 place-items-center rounded-pill border border-hairline bg-surface-1 text-ink-muted shadow-btn hover:text-ink focus-ring"
          >
            {theme === 'dark' ? <Sun className="size-3.5" aria-hidden /> : <Moon className="size-3.5" aria-hidden />}
          </button>
        </Tooltip>
      </div>
    </header>
  );
}

/** Mobile navigation: a Radix dialog (focus trap, Escape, focus return, scroll lock). */
function MobileDrawer({ open, onOpenChange, openerRef }: { open: boolean; onOpenChange(v: boolean): void; openerRef: { current: HTMLElement | null } }) {
  const loc = useLocation();
  const closeRef = useRef<HTMLButtonElement>(null);
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
        <RD.Content aria-describedby={undefined} onOpenAutoFocus={(e) => { e.preventDefault(); closeRef.current?.focus(); }} onCloseAutoFocus={(e) => { if (openerRef.current?.isConnected) { e.preventDefault(); openerRef.current.focus(); } }} className="fixed inset-y-0 left-0 z-40 w-64 max-w-[85vw] animate-fade-in bg-canvas focus:outline-none md:hidden">
          <RD.Title className="sr-only">Navigation</RD.Title>
          <RD.Close asChild>
            <Button ref={closeRef} variant="ghost" size="icon" className="absolute right-2 top-2" aria-label="Close menu"><X className="size-4" aria-hidden /></Button>
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
  const [collapsed, setCollapsed] = useState(false);
  const collapse = useMemo(() => ({ collapsed, setCollapsed }), [collapsed]);
  const openerRef = useRef<HTMLElement | null>(null);
  const admin = canAdmin(current);
  const { features } = useFeatures();

  const items = useMemo<CommandItem[]>(() => {
    const hasWs = workspaces.length > 0;
    const nav = (id: string, label: string, icon: ReactNode, to: string, shortcut?: string): CommandItem => ({ id, group: 'Navigate', label, icon, shortcut, run: () => navigate(to) });
    return [
      ...(hasWs ? [
        nav('go-home', 'Go to Home', <House className="size-4" />, wsPath('/home'), 'G H'),
        ...(features.copilot ? [nav('go-sessions', 'Go to Copilot sessions', <Sparkle className="size-4" />, wsPath('/sessions'), 'G C')] : []),
        ...(features.translationMeetings ? [nav('go-meetings', 'Go to Meetings', <SquaresFour className="size-4" />, wsPath('/meetings'), 'G M')] : []),
        nav('go-tasks', 'Go to My tasks', <ListChecks className="size-4" />, wsPath('/tasks'), 'G T'),
        ...(admin ? [nav('go-quality', 'Go to Suggestion quality', <ChartBar className="size-4" />, wsPath('/quality'))] : []),
        nav('go-members', 'Go to Members', <Users className="size-4" />, wsPath('/members'), 'G P'),
        nav('go-knowledge', 'Go to Knowledge', <Brain className="size-4" />, wsPath('/knowledge'), 'G K'),
        nav('go-documents', 'Go to Documents', <FileText className="size-4" />, wsPath('/documents')),
        nav('go-glossary', 'Go to Glossary', <BookBookmark className="size-4" />, wsPath('/glossary')),
        nav('go-settings', 'Go to Settings', <GearSix className="size-4" />, wsPath('/settings'), 'G S'),
        ...(admin ? [nav('go-ai', 'Go to AI providers', <Brain className="size-4" />, wsPath('/settings/ai'))] : []),
        ...(features.externalBridge ? [nav('go-external', 'Go to External meeting', <Broadcast className="size-4" />, '/external', 'G E')] : []),
      ] : []),
      nav('go-workspaces', 'Manage workspaces', <Buildings className="size-4" />, '/workspaces'),
      ...(SHOW_DESIGN ? [nav('go-design', 'Open design system', <Palette className="size-4" />, '/design')] : []),
      ...(hasWs && features.copilot ? [
        { id: 'new-session', group: 'Actions', label: 'Start copilot session', icon: <Sparkle className="size-4" />, shortcut: features.translationMeetings ? undefined : 'C', run: () => navigate(`${wsPath('/sessions')}?new=1`) },
      ] : []),
      ...(hasWs && features.translationMeetings ? [
        { id: 'new-room', group: 'Actions', label: 'Create meeting', icon: <Plus className="size-4" />, shortcut: 'C', run: () => navigate(`${wsPath('/meetings')}?new=1`) },
        { id: 'join-code', group: 'Actions', label: 'Join a meeting by code', icon: <Keyboard className="size-4" />, shortcut: 'J', run: () => navigate(`${wsPath('/meetings')}?join=1`) },
      ] : []),
      ...(!hasWs && features.translationMeetings ? [{ id: 'join-code', group: 'Actions', label: 'Join a meeting by code', icon: <Keyboard className="size-4" />, shortcut: 'J', run: () => navigate('/join') }] : []),
      { id: 'new-ws', group: 'Actions', label: 'Create workspace', icon: <Plus className="size-4" />, run: () => navigate('/workspaces?new=1') },
      { id: 'theme', group: 'Actions', label: 'Toggle theme', icon: <Sun className="size-4" />, run: toggle },
      ...workspaces.map((w) => ({ id: `ws-${w.id}`, group: 'Switch workspace', label: w.name, run: () => select(w.id) })),
      { id: 'logout', group: 'Account', label: 'Sign out', icon: <SignOut className="size-4" />, run: () => void logout() },
    ];
  }, [navigate, toggle, workspaces, select, logout, wsPath, admin, features]);

  return (
    <CommandMenuProvider items={items}>
      <SidebarCollapse.Provider value={collapse}>
      <div className={cn('flex h-dvh overflow-hidden bg-canvas', collapsed && 'md:pl-1.5')}>
        <aside className={cn('hidden w-56 shrink-0', !collapsed && 'md:block')} aria-label="Sidebar">
          <Sidebar />
        </aside>
        <MobileDrawer open={drawer} onOpenChange={setDrawer} openerRef={openerRef} />
        <div className="flex min-w-0 flex-1 flex-col">
          {/* The raised panel: the sidebar is the recessed layer under it. */}
          <div className="flex min-h-0 flex-1 flex-col overflow-hidden bg-panel md:my-1.5 md:mr-1.5 md:rounded-xl md:border md:border-hairline md:shadow-panel">
            <TopBar onMenu={(el) => { openerRef.current = el; setDrawer(true); }} />
            <main id="main" tabIndex={-1} className="min-h-0 flex-1 outline-none overflow-y-auto overflow-x-hidden">
              <BillingBanner />
              <Outlet />
            </main>
          </div>
        </div>
      </div>
      </SidebarCollapse.Provider>
    </CommandMenuProvider>
  );
}

/**
 * Moves focus to `#main` and puts `#main` in the URL. A plain `<a href="#main">` navigates natively behind the
 * back of Next's app router, whose history sync can then rewrite the URL back to its own (hash-less) canonical
 * one; going through the router's `navigate` (patched `history.pushState`) keeps both in agreement.
 */
export function SkipLink() {
  const navigate = useNavigate();
  return (
    <a
      href="#main"
      onClick={(e) => {
        const main = document.getElementById('main');
        if (!main) return;
        e.preventDefault();
        // The live URL, not router state: a redirect may be committed to history before the router re-renders.
        navigate({ pathname: window.location.pathname, search: window.location.search, hash: '#main' });
        main.focus();
        main.scrollIntoView();
      }}
      className="sr-only focus:not-sr-only focus:fixed focus:left-3 focus:top-3 focus:z-[90] focus:rounded-md focus:bg-primary focus:px-3 focus:py-2 focus:text-on-primary"
    >
      Skip to content
    </a>
  );
}

export { Link };
