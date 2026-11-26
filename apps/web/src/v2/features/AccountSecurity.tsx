import { useState, type FormEvent } from 'react';
import { Link } from 'react-router-dom';
import { useQueryClient } from '@tanstack/react-query';
import { Alert, Button, Field, Input, SectionLabel, SettingsGroup, SettingsRow, useToast } from '@v2/ui';
import { api } from '@v2/lib/api';
import { useProfile } from '@v2/lib/auth';
import { googleAvailable, googleErrorMessage } from '@v2/lib/google';
import { useNative } from '@v2/lib/native';
import { errCode, errMsg } from './errors';
import { GoogleButton } from './GoogleButton';

function PasswordSection({ hasPassword }: { hasPassword: boolean | undefined }) {
  const toast = useToast();
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const [confirm, setConfirm] = useState('');
  const [fe, setFe] = useState<{ current?: string; next?: string; confirm?: string }>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (hasPassword === false) {
    return (
      <section aria-label="Password">
        <SectionLabel>Password</SectionLabel>
        <SettingsGroup>
          <SettingsRow label="Set a password" description={<>This account signs in with Google and has no password yet. To add one, use <Link to="/forgot-password">Forgot password</Link> with your account email and choose a password from the link we send.</>}>
            <Button disabled aria-disabled>Set password</Button>
          </SettingsRow>
        </SettingsGroup>
      </section>
    );
  }

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;
    const v = {
      current: !current ? 'Enter your current password.' : undefined,
      next: next.length < 8 ? 'Password must be at least 8 characters.' : next === current ? 'Choose a password different from the current one.' : undefined,
      confirm: confirm !== next ? 'Passwords do not match.' : undefined,
    };
    setFe(v);
    setError(null);
    if (v.current || v.next || v.confirm) { requestAnimationFrame(() => form.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus()); return; }
    setBusy(true);
    try {
      await api('/me/password', { method: 'POST', body: { currentPassword: current, newPassword: next } });
      setCurrent(''); setNext(''); setConfirm('');
      toast({ tone: 'success', title: 'Password changed' });
    } catch (err) {
      const code = errCode(err);
      if (code === 'INVALID_CURRENT_PASSWORD') setFe({ current: 'Your current password is incorrect.' });
      else if (code === 'PASSWORD_NOT_SET') setError('This account has no password yet. Use Forgot password to set one.');
      else setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section aria-label="Password">
      <SectionLabel>Password</SectionLabel>
      <form onSubmit={submit} noValidate className="flex flex-col gap-4 rounded-lg border border-hairline bg-surface-1 p-5">
        <Field label="Current password" error={fe.current}><Input type="password" autoComplete="current-password" aria-required="true" value={current} onChange={(e) => setCurrent(e.target.value)} /></Field>
        <Field label="New password" hint="At least 8 characters." error={fe.next}><Input type="password" autoComplete="new-password" aria-required="true" value={next} onChange={(e) => setNext(e.target.value)} /></Field>
        <Field label="Confirm new password" error={fe.confirm}><Input type="password" autoComplete="new-password" aria-required="true" value={confirm} onChange={(e) => setConfirm(e.target.value)} /></Field>
        {error && <Alert tone="danger">{error}</Alert>}
        <div><Button type="submit" variant="primary" loading={busy}>Change password</Button></div>
      </form>
    </section>
  );
}

function GoogleSection({ hasPassword, linked }: { hasPassword: boolean | undefined; linked: boolean | undefined }) {
  useNative();
  const qc = useQueryClient();
  const toast = useToast();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (!googleAvailable() && !linked) return null;

  async function link(idToken: string) {
    setError(null);
    try {
      await api('/me/google/link', { method: 'POST', body: { idToken } });
      await qc.invalidateQueries({ queryKey: ['profile'] });
      toast({ tone: 'success', title: 'Google account linked' });
    } catch (err) { setError(googleErrorMessage(errCode(err), errMsg(err))); }
  }
  async function unlink() {
    setError(null);
    setBusy(true);
    try {
      await api('/me/google/link', { method: 'DELETE' });
      await qc.invalidateQueries({ queryKey: ['profile'] });
      toast({ tone: 'success', title: 'Google account unlinked' });
    } catch (err) { setError(googleErrorMessage(errCode(err), errMsg(err))); } finally { setBusy(false); }
  }

  const needPassword = hasPassword === false;
  return (
    <section aria-label="Google account">
      <SectionLabel>Google account</SectionLabel>
      <SettingsGroup>
        {linked ? (
          <SettingsRow label="Google is linked" description={needPassword ? 'You cannot unlink Google while this account has no password: you would be locked out. Set a password first (see Password above).' : 'You can sign in with Google or with your password.'}>
            <Button onClick={() => void unlink()} loading={busy} disabled={needPassword}>Unlink Google</Button>
          </SettingsRow>
        ) : (
          <div className="flex flex-col gap-3 px-5 py-4">
            <div><p className="text-body font-semibold text-ink">Link Google</p><p className="mt-0.5 text-body-sm text-ink-muted">Sign in with Google next time. The Google account must use the same email as this account.</p></div>
            <GoogleButton text="continue_with" onCredential={(t) => void link(t)} onLoadError={setError} />
          </div>
        )}
      </SettingsGroup>
      {error && <Alert tone="danger" className="mt-3">{error}</Alert>}
    </section>
  );
}

export function AccountSecurity() {
  const profile = useProfile();
  if (profile.isLoading) return null;
  return (
    <>
      <PasswordSection hasPassword={profile.data?.hasPassword} />
      <GoogleSection hasPassword={profile.data?.hasPassword} linked={profile.data?.googleLinked} />
    </>
  );
}
