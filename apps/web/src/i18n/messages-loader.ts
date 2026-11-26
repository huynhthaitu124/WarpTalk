import { DEFAULT_LOCALE } from "@/i18n/locale-constants";
import { mergeWithFallback, type Messages } from "@/i18n/message-fallback";

const NAMESPACES = [
  "common",
  "auth",
  "landing",
  "legal",
  "validation",
  "home",
  "dashboard",
  "tasks",
  "voiceProfiles",
  "knowledge",
  "aiChat",
  "glossary",
  "documents",
  "settingsWorkspace",
  "settingsSecurity",
  "settingsMemberRoles",
  "settingsFeatures",
  "settingsPluginActivity",
  "settingsAuditLog",
  "settingsSessions",
  "settingsPreferences",
  "settingsProfile",
  "settingsNotifications",
  "settingsConnectedAccounts",
  "rooms",
  "artifacts",
  "members",
  "pluginsPage",
  "workspacePlugins",
  "schedules",
  "settingsBilling",
  "settingsBillingUsage",
  "settingsBillingInvoices",
  "adminChrome",
  "adminMisc",
  "adminOps",
  "adminWorkspaces",
  "adminUsers",
  "adminSubscriptions",
  "adminAnnouncements",
  "adminCms",
  "adminPlansSettings",
  "adminGlobalGlossary",
  "adminPlugins",
  "adminBillingLedger",
  "adminBillingPlansLegacy",
  "adminLists",
  "adminStaff",
  "adminPackages",
  "settingsBillingCatalog",
  "adminProviders",
  "adminFinance",
  "adminAccount",
  "adminInbox",
  "adminPlatformSettings",
  "platformStatus",
  "download",
  "meetingControlBar",
  "meetingCallChrome",
  "meetingChat",
  "meetingRoomPage",
  "meetingTranscript",
  "meetingSummary",
  "minutes",
  "meetingLive",
  "workspaceInsights",
  "workspaceInsightsUsage",
  "workspaceInsightsTools",
  "warpbotTools",
] as const;

async function loadLocale(locale: string): Promise<Messages> {
  const entries = await Promise.all(
    NAMESPACES.map(async (namespace) => {
      const mod = (await import(`../../messages/${locale}/${namespace}.json`)) as { default: unknown };
      return [namespace, mod.default] as const;
    }),
  );
  return Object.fromEntries(entries) as Messages;
}

/** Client-side replacement for the old `next-intl/server` request config (the app is a static export). */
export async function loadMessages(locale: string): Promise<Messages> {
  const own = await loadLocale(locale);
  return locale === DEFAULT_LOCALE ? own : mergeWithFallback(await loadLocale(DEFAULT_LOCALE), own);
}
