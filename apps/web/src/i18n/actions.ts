import { LOCALE_COOKIE, LOCALE_COOKIE_MAX_AGE_SECONDS, isSupportedLocale, type Locale } from "@/i18n/locale-constants";

/** Persists a person's chosen UI locale (client-side: the app is a static export) and reloads the catalog. */
export async function setUserLocale(locale: Locale): Promise<void> {
  if (!isSupportedLocale(locale)) throw new Error("Unsupported locale");
  document.cookie = `${LOCALE_COOKIE}=${locale}; max-age=${LOCALE_COOKIE_MAX_AGE_SECONDS}; path=/; samesite=lax`;
  window.dispatchEvent(new CustomEvent("wt:locale", { detail: locale }));
}
