import { useEffect, useRef, useState } from 'react';
import { useNative } from '@v2/lib/native';
import { googleClientId } from '@v2/lib/env';
import { googleAvailable, loadGoogle } from '@v2/lib/google';

/**
 * Official "Sign in with Google" button (GIS `renderButton`). Renders nothing when `NEXT_PUBLIC_GOOGLE_CLIENT_ID`
 * is unset, or inside the macOS host (until its bridge offers a google sign-in method). `onCredential` gets the ID token.
 */
export function GoogleButton({ onCredential, text = 'signin_with', onLoadError }: { onCredential: (idToken: string) => void; text?: 'signin_with' | 'signup_with' | 'continue_with'; onLoadError?: (message: string) => void }) {
  useNative(); // re-evaluate once the native host announces itself
  const ref = useRef<HTMLDivElement>(null);
  const cb = useRef(onCredential);
  const err = useRef(onLoadError);
  const [ready, setReady] = useState(false);
  useEffect(() => { cb.current = onCredential; err.current = onLoadError; });
  const available = googleAvailable();

  useEffect(() => {
    if (!available) return;
    let alive = true;
    loadGoogle().then((id) => {
      if (!alive || !ref.current) return;
      id.initialize({ client_id: googleClientId(), callback: (r) => { if (r.credential) cb.current(r.credential); }, use_fedcm_for_prompt: true });
      const width = Math.min(400, Math.max(200, Math.floor(ref.current.getBoundingClientRect().width)));
      id.renderButton(ref.current, { type: 'standard', theme: 'outline', size: 'large', text, shape: 'rectangular', logo_alignment: 'left', width, locale: 'en' });
      setReady(true);
    }).catch((e: unknown) => { if (alive) err.current?.(e instanceof Error ? e.message : 'Could not load Google sign-in'); });
    return () => { alive = false; };
  }, [available, text]);

  if (!available) return null;
  return (
    <div className="flex flex-col gap-4" data-testid="google-signin">
      <div className="flex items-center gap-3 text-caption text-ink-subtle" aria-hidden><span className="h-px flex-1 bg-hairline" />or<span className="h-px flex-1 bg-hairline" /></div>
      <div ref={ref} className="flex min-h-10 w-full justify-center [color-scheme:light]" aria-busy={!ready} />
    </div>
  );
}
