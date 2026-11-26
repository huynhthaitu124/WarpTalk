"use client";

import Link from "@/lib/next-shim/link";

import { useEffect, useMemo, useRef, useState } from "react";
import { usePathname, useRouter } from "@/lib/next-shim/navigation";
import { useTranslations } from "next-intl";
import gsap from "gsap";
import { LinearSidebar } from "@/components/layout/linear-sidebar";
import {
  Plus,
  Question,
  SidebarSimple,
  Spinner,
} from "@phosphor-icons/react/dist/ssr";
import { useUIStore } from "@/stores/ui-store";
import { CreateRoomDialog } from "@/components/rooms/create-room-dialog";
import { SearchMeetingDialog } from "@/components/rooms/search-meeting-dialog";
import { GlobalChatbot } from "@/components/layout/global-chatbot";
import { NotificationPopover } from "@/components/notifications/notification-popover";
import { NotificationSoundToggle } from "@/components/layout/notification-sound-toggle";
import { ThemeToggleButton } from "@/components/layout/theme-toggle-button";
import { LanguageSwitcher } from "@/components/layout/language-switcher";
import { HeaderSearch } from "@/components/layout/header-search";
import { AnnouncementHost } from "@/components/announcements/announcement-host";
import { PlatformStatusBanner } from "@/components/platform/platform-status-banner";
import { MeetingInviteBanner } from "@/components/rooms/meeting-invite-banner";
import { MeetingStartedBanner } from "@/components/rooms/meeting-started-banner";
import { WorkspaceTabs, buildTabOptions, resolveCurrentTab } from "@/components/layout/workspace-tabs";
import { WorkspaceMembersPanel } from "@/components/layout/workspace-members-panel";

import { useIsSystemAdmin } from "@/hooks/use-is-system-admin";
import { AdminCommandPalette, AdminHeaderSearch } from "@/components/admin/admin-command-palette";
import { startProactiveRefresh } from "@/lib/api/client";
import { cn } from "@/lib/utils";
import { adminPageLabelKey } from "@/lib/admin/admin-page-title";
import {
  isLiveMeetingPath,
  isWorkspaceActivationPath,
} from "@/lib/workspace/workspace-routes";
import { useWorkspaceStore } from "@/stores/workspace-store";
import { ProductTour } from "@/components/onboarding/product-tour";
import { useOnboardingStore } from "@/stores/onboarding-store";
import { useWorkspaceTabsStore } from "@/stores/workspace-tabs-store";
import { useAuthStore } from "@/stores/auth-store";
import { getErrorStatus } from "@/lib/api/retry-policy";
import {
  useTranslationRoom,
} from "@/hooks/use-translationRooms";
import { useWorkspaces, useSelectWorkspace, useCreateWorkspace } from "@/hooks/use-workspace";
import { applySelectedWorkspace } from "@/lib/workspace/apply-selected-workspace";
import { ADMIN_CONSOLE_ENABLED, PERSONAL_WORKSPACE_NAME, WORKSPACE_UI_ENABLED, isHiddenWorkspaceOnboarding } from "@/lib/feature-flags";
import {
  preferRememberedWorkspace,
  recallLastWorkspaceSlug,
} from "@/lib/workspace/last-workspace";


/**
 * Whether the sidebar should be showing its icon-only rail YET.
 *
 * LinearSidebar renders two different trees — a rail and a full sidebar — and React swaps them
 * the instant the flag changes, while AnimatedWidthPanel spends 420ms tweening the width. The two
 * halves were never connected, so closing read as: every label vanishes at once, and only then
 * does the panel slide shut.
 *
 * Opening swaps immediately — the labels should be arriving as the panel widens. Closing holds
 * the full tree for a beat so the labels are carried out by the narrowing panel (which is
 * overflow-hidden, so they are clipped away rather than deleted) and only then becomes the rail.
 * The delay is deliberately shorter than the width tween: the swap lands while the panel is
 * still moving, so it is never a visible step on a sidebar that has already stopped.
 */
function useRailSwapDelay(open: boolean, delayMs: number) {
  const [collapsed, setCollapsed] = useState(!open);

  useEffect(() => {
    if (open) {
      setCollapsed(false);
      return;
    }
    const timer = window.setTimeout(() => setCollapsed(true), delayMs);
    return () => window.clearTimeout(timer);
  }, [open, delayMs]);

  return collapsed;
}

function AnimatedWidthPanel({
  open,
  width,
  collapsedWidth = 0,
  side,
  className,
  children,
}: {
  open: boolean;
  width: number;
  collapsedWidth?: number;
  side: "left" | "right";
  className?: string;
  children: React.ReactNode;
}) {
  const [initialWidth] = useState(() => (open ? width : collapsedWidth));
  const panelRef = useRef<HTMLDivElement | null>(null);
  const contentRef = useRef<HTMLDivElement | null>(null);
  const hasMounted = useRef(false);

  useEffect(() => {
    const panel = panelRef.current;
    const content = contentRef.current;
    if (!panel || !content) return;

    if (!hasMounted.current) {
      gsap.set(panel, { width: open ? width : collapsedWidth });
      gsap.set(content, {
        autoAlpha: open || collapsedWidth > 0 ? 1 : 0,
        x: open || collapsedWidth > 0 ? 0 : side === "left" ? -14 : 14,
      });
      hasMounted.current = true;
      return;
    }

    gsap.killTweensOf([panel, content]);
    gsap.to(panel, {
      width: open ? width : collapsedWidth,
      duration: 0.42,
      ease: "power3.inOut",
    });
    gsap.to(content, {
      autoAlpha: open || collapsedWidth > 0 ? 1 : 0,
      x: open || collapsedWidth > 0 ? 0 : side === "left" ? -14 : 14,
      duration: 0.28,
      ease: open ? "power3.out" : "power2.in",
    });
  }, [collapsedWidth, open, side, width]);

  return (
    <div
      ref={panelRef}
      aria-hidden={collapsedWidth === 0 && !open}
      className={cn(
        "h-full shrink-0 overflow-hidden",
        collapsedWidth === 0 && !open && "pointer-events-none",
        className,
      )}
      style={{ width: initialWidth }}
    >
      <div ref={contentRef} className="h-full" style={{ width }}>
        {children}
      </div>
    </div>
  );
}

export default function AppLayout({ children }: { children: React.ReactNode }) {
  const t = useTranslations("common");
  const pathname = usePathname();
  const router = useRouter();
  const {
    rightSidebarOpen,
    toggleRightSidebar,
    leftSidebarOpen,
    toggleLeftSidebar,
  } = useUIStore();
  // 160ms against the panel's 420ms width tween — see useRailSwapDelay.
  const railCollapsed = useRailSwapDelay(leftSidebarOpen, 160);
  const activeWorkspaceId = useWorkspaceStore((state) => state.activeWorkspaceId);
  const activeWorkspaceSlug = useWorkspaceStore((state) => state.activeWorkspaceSlug);
  const setActiveWorkspace = useWorkspaceStore((state) => state.setActiveWorkspace);
  const addWorkspaceTab = useWorkspaceTabsStore((state) => state.addTab);
  const currentUserId = useAuthStore((state) => state.user?.id);
  const isAuthenticated = useAuthStore((state) => state.isAuthenticated);
  const logout = useAuthStore((state) => state.logout);
  const openTour = useOnboardingStore((state) => state.openTour);
  const tourSeenAtByUser = useOnboardingStore((state) => state.tourSeenAtByUser);
  const [mounted, setMounted] = useState(false);
  
  // `isError` and `refetch` were not read. The gate below spun on `!activeWorkspaceId`, and a
  // failed workspaces query leaves that null forever — so any failure here painted a spinner
  // with no message, no retry and no way out, indistinguishable from a slow network. It is the
  // first request the shell makes, so it is also the one most likely to meet a cold gateway.
  const {
    data: workspacesData,
    isLoading: workspacesLoading,
    isError: workspacesFailed,
    error: workspacesError,
    refetch: refetchWorkspaces,
  } = useWorkspaces(1, 100);
  const selectWorkspace = useSelectWorkspace();
  const createWorkspace = useCreateWorkspace();
  // One silent personal-workspace creation per mount; the list refetch that follows must not
  // start a second one before it lands.
  const personalWorkspaceRequested = useRef(false);

  const roomId = (() => {
    const segments = pathname.split('/').filter(Boolean);
    if (segments.length >= 3 && segments[1] === 'rooms') {
      const id = segments[2];
      if (/^[0-9a-fA-F-]{36}$/.test(id)) return id;
    }
    if (segments.length >= 2 && segments[0] === 'room') {
      const id = segments[1];
      if (/^[0-9a-fA-F-]{36}$/.test(id)) return id;
    }
    return undefined;
  })();

  const roomQuery = useTranslationRoom(roomId ?? "");
  const roomTitle = roomId && roomQuery?.data ? roomQuery.data.title : undefined;
  const workspaceTabScope = activeWorkspaceSlug || "global";
  const workspaceTabsT = useTranslations("common.workspaceTabs");
  const workspaceTabOptions = useMemo(
    () => buildTabOptions(activeWorkspaceSlug || "workspace", workspaceTabsT),
    [activeWorkspaceSlug, workspaceTabsT]
  );
  const currentWorkspaceTab = useMemo(
    () => resolveCurrentTab(pathname, workspaceTabOptions),
    [pathname, workspaceTabOptions]
  );

  const isAdminRoute = pathname === "/admin" || pathname.startsWith("/admin/");
  /**
   * The routes that get NO portal drawn around them.
   *
   * The first four are the ones that run before a workspace is chosen. `/{slug}/activate` is the
   * fifth and arrives from the other direction — a workspace exists, and has not been paid for.
   * It belongs in this list for the same reason they do: a sidebar full of destinations that all
   * redirect back to the page you are on is not navigation, it is the product with its doors
   * locked, which was the exact complaint about the screen this route replaced.
   *
   * `isAdminRoute` is subtracted because the activation matcher is `/{anything}/activate` — it
   * cannot tell a workspace slug from a top-level route on its own.
   */
  const isOnboardingRoute =
    !isHiddenWorkspaceOnboarding(pathname) && (
    pathname === "/workspace" ||
    pathname === "/workspace/plans" ||
    pathname === "/workspace/create" ||
    pathname === "/workspace/join" ||
    (!isAdminRoute && isWorkspaceActivationPath(pathname)));
  const isSystemAdmin = useIsSystemAdmin();
  // Decides more than the header divider: it is also what tells the meeting dock to stop
  // floating (`floating={!isLiveMeetingRoute}`). Miss the live route and the minimised
  // window floats on top of the meeting it is a copy of.
  const isLiveMeetingRoute = isLiveMeetingPath(pathname);
  // Starts the token's refresh timer for a session that was already in place on load.
  //
  // From an effect, deliberately. The same call at module scope in the api client took
  // production down with a temporal-dead-zone error, because it read the auth store while
  // the two modules were still evaluating each other. By the time an effect runs, every
  // module has finished — which is the only guarantee that actually holds.
  useEffect(() => {
    startProactiveRefresh();
  }, []);

  useEffect(() => {
    const handle = requestAnimationFrame(() => setMounted(true));
    return () => cancelAnimationFrame(handle);
  }, []);

  /**
   * The first-run tour, once — and only once the shell it describes is actually on screen.
   *
   * Two conditions, both load-bearing. Without a workspace slug the sidebar has no destinations
   * to point at, and the tour would open against a shell that is still a spinner. The delay
   * covers the rest: the panel width tween runs for 420ms, and a spotlight measured mid-tween
   * lands next to the control rather than on it.
   *
   * The check is repeated inside the timer rather than only in the dependency array, because
   * The record is persisted and zustand rehydrates it after the first client render — reading
   * it once at effect time would show a returning user the tour they finished last week.
   */
  useEffect(() => {
    // Keyed by user, so signing out and back in does not re-run a tour this person already
    // dismissed — which is exactly what the previous single flag did, because it was cleared
    // on every sign-in for account isolation.
    if (!currentUserId || !activeWorkspaceSlug) return;
    if (tourSeenAtByUser[currentUserId] != null) return;

    const timer = setTimeout(() => {
      if (!useOnboardingStore.getState().hasSeenTour(currentUserId)) openTour();
    }, 900);
    return () => clearTimeout(timer);
  }, [tourSeenAtByUser, currentUserId, activeWorkspaceSlug, openTour]);

  useEffect(() => {
    if (mounted && !isAuthenticated) {
      router.replace("/login");
    }
  }, [mounted, isAuthenticated, router]);

  useEffect(() => {
    if (!mounted || !isAuthenticated || isOnboardingRoute || isAdminRoute || workspacesLoading) return;
    if (selectWorkspace.isPending) return;

    if (!activeWorkspaceId) {
      // WT-347: the workspace this account was last in, when it is still one of theirs;
      // otherwise the first in the list, as before.
      const firstWs = preferRememberedWorkspace(
        workspacesData?.items ?? [],
        recallLastWorkspaceSlug(currentUserId),
      );
      if (firstWs) {
        // Hydrated from the SELECT RESPONSE, not from the list row. The list's shape varies by
        // endpoint — hence the `"membershipType" in firstWs` guards this replaced — and the
        // select call is the one authority on what this user's role in this workspace is. It is
        // awaited so a failed selection redirects instead of leaving the shell holding a
        // workspace the server never confirmed.
        void (async () => {
          try {
            const selection = await selectWorkspace.mutateAsync(firstWs.id);
            applySelectedWorkspace(selection, setActiveWorkspace);
          } catch {
            router.replace("/workspace");
          }
        })();
      } else if (ADMIN_CONSOLE_ENABLED && isSystemAdmin) {
        // A platform admin with no workspace of their own is not a new user who has yet to make
        // one — they administer the platform the workspaces live in. Sending them to
        // /workspace made the master account look like it had signed up by mistake, and the
        // only way on was to create a workspace nobody wanted. The admin portal is their home.
        router.replace("/admin");
      } else if (!WORKSPACE_UI_ENABLED) {
        // Workspaces are hidden: an account with none gets its personal one here, without a
        // screen, and lands in it.
        if (personalWorkspaceRequested.current) return;
        personalWorkspaceRequested.current = true;
        void (async () => {
          try {
            const created = await createWorkspace.mutateAsync({ name: PERSONAL_WORKSPACE_NAME });
            const selection = await selectWorkspace.mutateAsync(created.id);
            applySelectedWorkspace(selection, setActiveWorkspace);
          } catch {
            personalWorkspaceRequested.current = false;
          }
        })();
      } else {
        router.replace("/workspace");
      }
    }
  }, [activeWorkspaceId, workspacesData, workspacesLoading, isOnboardingRoute, isAdminRoute, isSystemAdmin, selectWorkspace, createWorkspace, setActiveWorkspace, router, mounted, isAuthenticated, currentUserId]);

  // A hidden onboarding address (/workspace, /workspace/create, …) forwards to the workspace home
  // once there is a workspace to land in.
  useEffect(() => {
    if (activeWorkspaceSlug && isHiddenWorkspaceOnboarding(pathname)) {
      router.replace(`/${activeWorkspaceSlug}/home`);
    }
  }, [activeWorkspaceSlug, pathname, router]);

  if (!mounted || !isAuthenticated) {
    return (
      <div className="flex h-dvh w-screen items-center justify-center bg-canvas">
        <Spinner className="h-6 w-6 animate-spin text-ink-muted" />
      </div>
    );
  }

  if (isOnboardingRoute) {
    return <>{children}</>;
  }

  // Checked before the spinner, because a failure and a slow load are the same picture and
  // only one of them ends. Retry rather than reload: the session is fine, one request was not.
  if (!isAdminRoute && workspacesFailed) {
    return (
      <div className="flex h-dvh w-screen items-center justify-center bg-canvas p-6">
        {/* Maintenance mode answers this very request with 503: say so, not only "could not load". */}
        <PlatformStatusBanner variant="public" />
        <div className="max-w-sm space-y-4 text-center">
          <h1 className="text-base font-semibold text-ink">Could not load your workspaces</h1>
          <p className="text-sm leading-relaxed text-ink-muted">
            {getErrorStatus(workspacesError) === null
              ? "The server could not be reached. Check your connection and try again."
              : "The server refused that request. Trying again may work; if it does not, sign out and back in."}
          </p>
          <div className="flex items-center justify-center gap-2">
            <button
              type="button"
              onClick={() => void refetchWorkspaces()}
              className="rounded-md bg-ink px-3 py-1.5 text-[13px] font-medium text-canvas transition-opacity hover:opacity-90"
            >
              Try again
            </button>
            <button
              type="button"
              onClick={() => {
                logout();
                router.replace("/login");
              }}
              className="rounded-md border border-border px-3 py-1.5 text-[13px] text-ink-muted transition-colors hover:text-ink"
            >
              Sign out
            </button>
          </div>
        </div>
      </div>
    );
  }

  if (!isAdminRoute && (!activeWorkspaceId || workspacesLoading)) {
    return (
      <div className="flex h-dvh w-screen items-center justify-center bg-canvas">
        <Spinner className="h-6 w-6 animate-spin text-ink-muted" />
      </div>
    );
  }

  function handleAddCurrentWorkspaceTab() {
    if (!currentWorkspaceTab) return;
    addWorkspaceTab(workspaceTabScope, currentWorkspaceTab);
  }

  return (
    <div className="relative h-dvh flex overflow-hidden bg-canvas text-ink">
      <AnimatedWidthPanel
        open={leftSidebarOpen}
        width={224}
        collapsedWidth={64}
        side="left"
      >
        <LinearSidebar collapsed={railCollapsed} />
      </AnimatedWidthPanel>
      {/* Main Column */}
      <div className="relative flex flex-col flex-1 overflow-hidden min-w-0">
        {/* Main content box */}
        {/* The box every workspace page renders inside. `bg-panel`, not `bg-surface-1`: this is the
            page's ground, and surface-1 is the card colour — painted here, every card in the
            product sat on its own colour and vanished (owner's call, 2026-09-16). */}
        <div className="relative flex flex-col flex-1 overflow-hidden mt-1.5 mr-1.5 mb-0 rounded-xl border border-border bg-panel shadow-sm">
          {/* Top bar */}
        <header
          className={cn(
            // Three columns since the search sits between the breadcrumb and the icons.
            // This was two, and adding a third child silently wrapped the icon cluster onto a
            // second grid row inside a 44px-tall header — the notification bell, theme toggle,
            // help and the right-panel toggle all vanished. Nothing failed: not typecheck, not
            // lint, not the build. Only looking at it showed anything was wrong.
            //
            // The middle column is capped rather than 1fr so the search does not stretch across
            // a wide window, and minmax(0,…) on the outer two lets a long breadcrumb truncate
            // instead of pushing the icons off the edge.
            "h-[44px] grid grid-cols-[minmax(0,1fr)_minmax(0,420px)_minmax(0,1fr)] items-center gap-3 px-4 shrink-0",
            !isLiveMeetingRoute && "border-b border-border",
          )}
        >
          <div className="flex min-w-0 items-center gap-1.5 text-[13px] text-ink-muted">
            <button
              onClick={toggleLeftSidebar}
              className="flex size-6 items-center justify-center rounded-[6px] border border-transparent hover:bg-surface-2 hover:text-ink transition-colors mr-1"
              title={leftSidebarOpen ? t("topbar.collapseSidebar") : t("topbar.expandSidebar")}
              aria-label={leftSidebarOpen ? t("topbar.collapseSidebar") : t("topbar.expandSidebar")}
            >
              <SidebarSimple size={13} weight="bold" />
            </button>
            {(() => {
              const parts: { label: string; href?: string }[] = [];
              const segments = pathname.split('/').filter(Boolean);

              if (segments.length >= 1) {
                const firstSeg = segments[0];
                if (firstSeg === "admin") {
                  // One map for every admin page (lib/admin/admin-page-title.ts); "Insights" is
                  // /admin's own title, never the fallback for a page the map does not know.
                  const key = adminPageLabelKey(pathname);
                  if (key) parts.push({ label: t(`sidebar.adminNav.items.${key}`) });
                } else if (firstSeg === "settings" && segments[1] === "plugins") {
                  // The personal page, outside any workspace slug: "My connections", never the raw
                  // segment, and never "Plugins", which is the workspace page's name.
                  parts.push({ label: t("sidebar.settingsNav.myConnections") });
                } else if (firstSeg === "voice-profiles") {
                  parts.push({ label: t("sidebar.nav.voiceProfiles") });
                } else if (firstSeg === "join") {
                  parts.push({ label: t("sidebar.joinDialog.title") });
                } else if (firstSeg === "room") {
                  parts.push({ label: t("sidebar.nav.meetings"), href: `/${activeWorkspaceSlug || "workspace"}/rooms` });
                  const rId = segments[1];
                  if (rId) {
                    parts.push({ label: roomTitle || t("topbar.loading") });
                  }
                } else if (segments.length >= 2) {
                  const slug = firstSeg;
                  const feature = segments[1];

                  if (feature === "rooms") {
                    parts.push({ label: t("sidebar.nav.meetings"), href: `/${slug}/rooms` });
                    const sub = segments[2];
                    if (sub) {
                      parts.push({ label: roomTitle || t("topbar.loading") });
                    }
                  } else if (feature === "artifacts") {
                    parts.push({ label: t("sidebar.nav.artifacts") });
                  } else if (feature === "dashboard") {
                    parts.push({ label: t("sidebar.nav.dashboard") });
                  } else if (feature === "home") {
                    parts.push({ label: t("sidebar.nav.home") });
                  } else if (feature === "members") {
                    parts.push({ label: t("sidebar.nav.members") });
                  } else if (feature === "documents") {
                    parts.push({ label: t("sidebar.nav.documents") });
                  } else if (feature === "schedules") {
                    parts.push({ label: t("sidebar.nav.schedules") });
                  } else if (feature === "voice-profiles") {
                    parts.push({ label: t("sidebar.nav.voiceProfiles") });
                  } else if (feature === "glossary") {
                    parts.push({ label: t("sidebar.nav.glossary") });
                  } else if (feature === "tasks") {
                    parts.push({ label: t("sidebar.nav.myTasks") });
                  } else if (feature === "knowledge") {
                    parts.push({ label: t("sidebar.nav.knowledge") });
                  } else if (feature === "settings") {
                    const sub = segments[2];
                    const SETTINGS_SUB_KEYS: Record<string, string> = {
                      plugins: "workspacePlugins",
                      "plugin-activity": "pluginActivity",
                      billing: "billing",
                      features: "features",
                      "member-roles": "memberRoles",
                      security: "security",
                      "audit-log": "auditLog",
                    };
                    const ACCOUNT_LEAF_KEYS: Record<string, string> = {
                      profile: "profile",
                      preferences: "preferences",
                      notifications: "notifications",
                      "connected-accounts": "connectedAccounts",
                      sessions: "sessionsDevices",
                    };
                    if (sub === "account") {
                      parts.push({ label: t("sidebar.settingsNav.settingsLabel"), href: `/${slug}/settings` });
                      const leaf = segments[3];
                      const leafKey = leaf ? ACCOUNT_LEAF_KEYS[leaf] : undefined;
                      parts.push({ label: leafKey ? t(`sidebar.settingsNav.${leafKey}`) : t("topbar.account") });
                    } else {
                      const subKey = sub ? SETTINGS_SUB_KEYS[sub] : undefined;
                      if (subKey) {
                        parts.push({ label: t("sidebar.settingsNav.settingsLabel"), href: `/${slug}/settings` });
                        parts.push({ label: t(`sidebar.settingsNav.${subKey}`) });
                      } else {
                        parts.push({ label: t("sidebar.settingsNav.settingsLabel") });
                      }
                    }
                  } else if (feature === "insights") {
                    // WT-878: the workspace Insights page, named as the main nav names it.
                    parts.push({ label: t("sidebar.nav.insights") });
                  } else if (feature === "tools") {
                    parts.push({ label: t("sidebar.nav.tools") });
                  } else if (feature === "billing") {
                    parts.push({ label: t("sidebar.settingsNav.billing") });
                  } else if (feature === "payment") {
                    parts.push({ label: t("topbar.payment") });
                    const sub = segments[2];
                    if (sub) {
                      parts.push({ label: sub === "plans" ? t("topbar.plans") : sub });
                    }
                  } else {
                    parts.push({ label: feature });
                  }
                } else {
                  parts.push({ label: t("sidebar.workspaceFallback") });
                }
              } else {
                parts.push({ label: t("sidebar.workspaceFallback") });
              }

              return parts.map((part, index) => {
                return (
                  <span key={index} className="flex items-center gap-1.5">
                    {part.href && index < parts.length - 1 ? (
                      <Link href={part.href} className="hover:text-ink cursor-pointer transition-colors">
                        {part.label}
                      </Link>
                    ) : (
                      <span className={index === parts.length - 1 ? "text-ink font-medium max-w-[300px] truncate" : "hover:text-ink cursor-pointer transition-colors capitalize"}>
                        {part.label}
                      </span>
                    )}
                    {index < parts.length - 1 && <span className="text-ink-muted/40">/</span>}
                  </span>
                );
              });
            })()}
            {currentWorkspaceTab ? (
              <button
                type="button"
                onClick={handleAddCurrentWorkspaceTab}
                className="ml-0.5 grid size-5 shrink-0 place-items-center rounded-[6px] border border-transparent text-ink-muted transition-colors hover:border-border hover:bg-surface-2 hover:text-ink focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/40"
                title={t("topbar.addTab", { title: currentWorkspaceTab.title })}
                aria-label={t("topbar.addTab", { title: currentWorkspaceTab.title })}
              >
                <Plus size={11} weight="bold" />
              </button>
            ) : null}
          </div>

          {/*
            Search sits between the breadcrumb and the icon cluster, which is where it was
            designed to go — the Topbar that owned it was simply never mounted, so the header
            has been running without it. min-w-0 so the breadcrumb, not the search box, is
            what gives up space on a narrow window.
          */}
          <div className="hidden min-w-0 flex-1 justify-center px-4 md:flex">
            {/* The admin portal has no rooms to paste a code for: its box opens the admin command
                palette (pages, records, actions) instead. See admin-command-palette.tsx. */}
            {isAdminRoute ? <AdminHeaderSearch /> : <HeaderSearch />}
          </div>

          <div className="flex items-center justify-end gap-1.5 text-ink-muted">
            <NotificationPopover />
            <NotificationSoundToggle />
            <ThemeToggleButton />
            <LanguageSwitcher compact />
            {/* This was a button with no onClick — the only affordance in the header that did
                nothing at all. It opens the tour now, which is also where the tour's last step
                points, so somebody who skipped it knows where it went. */}
            <button
              type="button"
              data-tour="help-button"
              onClick={openTour}
              title={t("topbar.showMeAround")}
              aria-label={t("topbar.showMeAround")}
              className="flex size-6 items-center justify-center rounded-full border border-hairline bg-surface-1 shadow-[0_1px_2px_rgba(0,0,0,0.04)] hover:bg-surface-2 hover:text-ink transition-colors"
            >
              <Question size={12} weight="bold" />
            </button>
            <div className="w-[1px] h-3.5 bg-border mx-1" />
            <button
              onClick={toggleRightSidebar}
              className="flex size-6 items-center justify-center rounded-[6px] border border-hairline bg-surface-1 shadow-[0_1px_2px_rgba(0,0,0,0.04)] hover:bg-surface-2 hover:text-ink transition-colors"
            >
              <SidebarSimple size={13} weight="bold" />
            </button>
          </div>
        </header>

        {!isAdminRoute && <WorkspaceTabs />}

        <ProductTour />

        <div className="flex flex-1 min-h-0 overflow-hidden">
          {/* A non-scrolling frame around the scrolling main column, so anything pinned to the
              content area — the meeting notices — stays put while the page scrolls under
              it. `<main>` itself cannot serve: it IS the scroll container. */}
          <div className="relative flex min-w-0 flex-1 flex-col">
          {/* Published announcements (admin → Announcements): the top banner sits above the
              scroll container so it stays put while the page scrolls; the modal and toasts float.
              Not on the admin console, where the CMS previews them, and not inside a live
              meeting, where nothing should compete. The bell panel and the workspace home render
              the other two placements. */}
          {/* Maintenance mode (platform setting general.maintenance.enabled): on every route,
              admin included — staff are the ones who need to remember to turn it off. */}
          <PlatformStatusBanner />
          <AnnouncementHost enabled={!isAdminRoute && !isLiveMeetingRoute && !isOnboardingRoute} />
          <main className="relative min-h-0 flex-1 overflow-y-auto">
            {children}
          </main>

            {/* Both meeting notices live in ONE stack, and the stack — not either card — owns the
                corner. They are independent (being invited to Thursday's review does not stop this
                morning's standup going live), so both can be on screen at once; positioned
                separately they would have been drawn on top of each other.

                Bottom-RIGHT, moved from bottom-left at the owner's request. The other two things
                that claim a corner here are the WarpBot launcher and the mini meeting dock, which
                sit outside and below this main content box, so they no longer collide. The
                invitation is listed first — it is the one asking a question. */}
            <div className="pointer-events-none absolute inset-x-0 bottom-0 z-30 flex flex-col items-end gap-2 p-4">
              <MeetingInviteBanner />
              <MeetingStartedBanner />
            </div>
          </div>

          {/* Right Sidebar (Context/Properties) */}
          {WORKSPACE_UI_ENABLED && !isAdminRoute && !isLiveMeetingRoute && !pathname.startsWith('/rooms/') && (
            <AnimatedWidthPanel
              open={rightSidebarOpen}
              width={260}
              side="right"
              className="bg-panel"
            >
              {/* The rail shares the page's ground rather than the card colour: it sits inside the
                  same box as the content, so painting it surface-1 made a 260px white slab down
                  the side of a grey page. */}
              <aside className="flex h-full w-[260px] shrink-0 flex-col overflow-hidden border-l border-border bg-panel">
              {/* Members, not "Properties".
                  The panel used to be a header over the sentence "Select an item to view its
                  properties and actions" — and nothing in the app ever published an item for it
                  to describe, so that sentence was the whole feature. 260px had been reserved
                  for something that never arrived.

                  Properties is meant to return here for a selected item; it is not built in this
                  change because there is still no selection to read. Adding a store nothing
                  writes to would be the same placeholder again, one layer deeper. */}
              <div className="flex items-center px-4 h-[38px] border-b border-border">
                <span className="text-[12px] font-medium text-ink">Members</span>
              </div>
              <div className="flex-1 p-4 overflow-y-auto">
                <WorkspaceMembersPanel
                  workspaceId={activeWorkspaceId}
                  workspaceSlug={activeWorkspaceSlug}
                />
              </div>
            </aside>
            </AnimatedWidthPanel>
          )}
        </div>
        </div>
        
        {/* Global Chatbot outside main content box */}
        <div className="shrink-0 mr-1.5">
          <GlobalChatbot />
        </div>
      </div>
      <CreateRoomDialog />
      {/* One palette per shell, never both: each owns ⌘K, and two would answer the same keypress. */}
      {isAdminRoute ? <AdminCommandPalette /> : <SearchMeetingDialog />}
    </div>
  );
}
