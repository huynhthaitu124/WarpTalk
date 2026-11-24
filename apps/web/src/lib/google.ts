// Google Identity Services (GIS) loader. The script is fetched lazily, only when a Google control mounts.
import { googleClientId } from './env';
import { getNative } from './native';

const SRC = 'https://accounts.google.com/gsi/client';

interface CredentialResponse { credential?: string }
export interface GoogleIdApi {
  initialize(cfg: { client_id: string; callback: (r: CredentialResponse) => void; auto_select?: boolean; use_fedcm_for_prompt?: boolean }): void;
  renderButton(el: HTMLElement, opts: Record<string, unknown>): void;
}
declare global { interface Window { google?: { accounts?: { id?: GoogleIdApi } } } }

let loading: Promise<GoogleIdApi> | null = null;

export function loadGoogle(): Promise<GoogleIdApi> {
  const ready = window.google?.accounts?.id;
  if (ready) return Promise.resolve(ready);
  if (loading) return loading;
  loading = new Promise<GoogleIdApi>((resolve, reject) => {
    const s = document.createElement('script');
    s.src = SRC;
    s.async = true;
    s.onload = () => {
      const id = window.google?.accounts?.id;
      if (id) resolve(id); else reject(new Error('Google script did not initialise'));
    };
    s.onerror = () => reject(new Error('Could not load Google sign-in'));
    document.head.appendChild(s);
  }).catch((e: unknown) => { loading = null; throw e; });
  return loading;
}

/**
 * Whether Google sign-in can be offered: needs a configured client id, and inside the macOS WebView the host
 * must expose a Google sign-in method (GIS popups/redirects do not work in WKWebView). The bridge has no such
 * method yet, so the button is simply hidden in the native host for now.
 */
export function googleAvailable(): boolean {
  if (!googleClientId()) return false;
  const native = getNative() as (ReturnType<typeof getNative> & { googleSignIn?: unknown }) | undefined;
  if (native) return typeof native.googleSignIn === 'function';
  return true;
}

/** User-facing messages for the Google endpoints (error codes from the API). */
export function googleErrorMessage(code: string | undefined, fallback: string): string {
  switch (code) {
    case 'INVALID_GOOGLE_TOKEN': return 'Google could not verify your sign-in. Please try again.';
    case 'GOOGLE_UNAVAILABLE': return 'Google sign-in is temporarily unavailable. Try again shortly or use your password.';
    case 'GOOGLE_NOT_CONFIGURED': return 'Google sign-in is not set up on this server.';
    case 'GOOGLE_LINK_REQUIRES_VERIFIED_EMAIL': return 'An account with this email already exists. Verify its email address first, then sign in with Google.';
    case 'GOOGLE_ACCOUNT_MISMATCH': return 'This email is already connected to a different Google account.';
    case 'GOOGLE_EMAIL_MISMATCH': return 'That Google account uses a different email than your WarpTalk account.';
    case 'GOOGLE_ALREADY_LINKED': return 'A Google account is already linked to your account.';
    case 'GOOGLE_ACCOUNT_IN_USE': return 'That Google account is already linked to another WarpTalk account.';
    case 'GOOGLE_UNLINK_REQUIRES_PASSWORD': return 'Set a password before unlinking Google, otherwise you could not sign in.';
    case 'ACCOUNT_LOCKED': return 'This account is temporarily locked. Try again later.';
    case 'ACCOUNT_DISABLED': return 'This account has been disabled.';
    default: return fallback;
  }
}
