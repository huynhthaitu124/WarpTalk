import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { Link, Navigate, useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { Alert, Brand, Button, Card, CardBody, Field, Input } from '@/ui';
import { useAuth } from '@/lib/auth';
import { errCode, errMsg } from './errors';

function AuthLayout({ title, subtitle, children, footer }: { title: string; subtitle?: string; children: ReactNode; footer?: ReactNode }) {
  return (
    <main className="flex min-h-dvh flex-col items-center justify-center gap-6 bg-canvas px-4 py-10">
      <Brand className="text-title" />
      <Card className="w-full max-w-sm">
        <CardBody className="flex flex-col gap-5 p-6">
          <div>
            <h1 className="text-card-title font-semibold tracking-[-0.4px] text-ink">{title}</h1>
            {subtitle && <p className="mt-1 text-body text-ink-subtle">{subtitle}</p>}
          </div>
          {children}
        </CardBody>
      </Card>
      {footer && <p className="text-body text-ink-subtle">{footer}</p>}
    </main>
  );
}

export function LoginPage() {
  const { login, status } = useAuth();
  const nav = useNavigate();
  const loc = useLocation();
  const [params] = useSearchParams();
  const [email, setEmail] = useState(params.get('email') ?? '');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<{ code?: string; message: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const from = (loc.state as { from?: string } | null)?.from ?? '/';

  if (status === 'authed') return <Navigate to={from} replace />;

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await login(email.trim(), password);
      nav(from, { replace: true });
    } catch (err) {
      setError({ code: errCode(err), message: errMsg(err) });
    } finally {
      setBusy(false);
    }
  }

  return (
    <AuthLayout title="Sign in" subtitle="Welcome back to WarpTalk." footer={<>New here? <Link to="/register">Create an account</Link></>}>
      <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
        <Field label="Email"><Input type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} /></Field>
        <Field label="Password"><Input type="password" autoComplete="current-password" required value={password} onChange={(e) => setPassword(e.target.value)} /></Field>
        {error && (
          <Alert tone="danger">
            {error.message}
            {error.code === 'EMAIL_NOT_VERIFIED' && <> <Link to={`/verify-email?email=${encodeURIComponent(email)}`}>Verify your email</Link>.</>}
          </Alert>
        )}
        <Button type="submit" variant="primary" size="lg" loading={busy}>Sign in</Button>
      </form>
    </AuthLayout>
  );
}

export function RegisterPage() {
  const { register, status } = useAuth();
  const nav = useNavigate();
  const [fullName, setFullName] = useState('');
  const [email, setEmail] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  if (status === 'authed') return <Navigate to="/" replace />;

  async function submit(e: FormEvent) {
    e.preventDefault();
    if (password.length < 8) { setError('Password must be at least 8 characters.'); return; }
    setBusy(true);
    setError(null);
    try {
      await register({ email: email.trim(), password, fullName: fullName.trim() });
      nav(`/verify-email?email=${encodeURIComponent(email.trim())}`, { replace: true });
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }
  return (
    <AuthLayout title="Create your account" subtitle="Real-time translated meetings in minutes." footer={<>Already have an account? <Link to="/login">Sign in</Link></>}>
      <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
        <Field label="Full name"><Input autoComplete="name" required value={fullName} onChange={(e) => setFullName(e.target.value)} /></Field>
        <Field label="Email"><Input type="email" autoComplete="email" required value={email} onChange={(e) => setEmail(e.target.value)} /></Field>
        <Field label="Password" hint="At least 8 characters."><Input type="password" autoComplete="new-password" required value={password} onChange={(e) => setPassword(e.target.value)} /></Field>
        {error && <Alert tone="danger">{error}</Alert>}
        <Button type="submit" variant="primary" size="lg" loading={busy}>Create account</Button>
      </form>
    </AuthLayout>
  );
}

export function VerifyEmailPage() {
  const { verifyEmail } = useAuth();
  const [params] = useSearchParams();
  const urlToken = params.get('token');
  const email = params.get('email');
  const [token, setToken] = useState(urlToken ?? '');
  const [state, setState] = useState<'idle' | 'busy' | 'done' | 'error'>('idle');
  const [error, setError] = useState('');
  const auto = useRef(false);

  async function verify(t: string) {
    setState('busy');
    setError('');
    try {
      await verifyEmail(t.trim());
      setState('done');
    } catch (err) {
      setError(errMsg(err));
      setState('error');
    }
  }
  useEffect(() => {
    if (urlToken && !auto.current) { auto.current = true; void verify(urlToken); }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [urlToken]);

  return (
    <AuthLayout title="Verify your email" subtitle={email ? `We sent a verification link to ${email}.` : 'Enter the verification token from your email.'} footer={<Link to="/login">Back to sign in</Link>}>
      {state === 'done' ? (
        <>
          <Alert tone="success" title="Email verified">You can now sign in.</Alert>
          <Link to={`/login${email ? `?email=${encodeURIComponent(email)}` : ''}`} className="inline-flex h-10 items-center justify-center rounded-md bg-primary px-5 font-medium text-on-primary no-underline hover:bg-primary-hover hover:no-underline">Continue to sign in</Link>
        </>
      ) : (
        <form onSubmit={(e) => { e.preventDefault(); void verify(token); }} className="flex flex-col gap-4">
          <Alert tone="info" title="Check your inbox">
            Open the link in the email, or paste the token below. In development the API only logs the token length (no mail is sent), so use the database or the E2E fixture to mark the account verified.
          </Alert>
          <Field label="Verification token"><Input autoComplete="one-time-code" required value={token} onChange={(e) => setToken(e.target.value)} /></Field>
          {state === 'error' && <Alert tone="danger">{error}</Alert>}
          <Button type="submit" variant="primary" size="lg" loading={state === 'busy'} disabled={!token.trim()}>Verify email</Button>
        </form>
      )}
    </AuthLayout>
  );
}
