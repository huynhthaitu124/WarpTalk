import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { Link, Navigate, useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { Eye, EyeSlash, Translate, Microphone, Brain } from '@phosphor-icons/react';
import { Alert, Brand, Button, Card, CardBody, Field, Input } from '@v2/ui';
import { useAuth } from '@v2/lib/auth';
import { api } from '@v2/lib/api';
import { googleErrorMessage } from '@v2/lib/google';
import { errCode, errMsg } from './errors';
import { GoogleButton } from './GoogleButton';

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

/** After a failed submit, move focus to the first field that reports an error. */
function focusFirstInvalid(form: HTMLFormElement | null) {
  requestAnimationFrame(() => form?.querySelector<HTMLElement>('[aria-invalid="true"]')?.focus());
}

function PasswordInput(props: Omit<React.InputHTMLAttributes<HTMLInputElement>, 'type'>) {
  const [show, setShow] = useState(false);
  return (
    <div className="relative">
      <Input {...props} type={show ? 'text' : 'password'} className="pr-10" />
      <button
        type="button"
        onClick={() => setShow((v) => !v)}
        aria-label={show ? 'Hide characters' : 'Show characters'}
        aria-pressed={show}
        className="absolute right-1 top-1/2 grid size-7 -translate-y-1/2 place-items-center rounded-md text-ink-subtle hover:bg-surface-2 hover:text-ink focus-ring"
      >
        {show ? <EyeSlash size={16} aria-hidden /> : <Eye size={16} aria-hidden />}
      </button>
    </div>
  );
}

function AuthLayout({ title, subtitle, children, footer }: { title: string; subtitle?: string; children: ReactNode; footer?: ReactNode }) {
  return (
    <div className="grid min-h-dvh bg-canvas lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
      <aside className="hidden flex-col justify-between border-r border-hairline bg-panel p-12 lg:flex" aria-hidden>
        <Brand className="h-8 w-[130px]" />
        <div className="max-w-md">
          <p className="text-display-md font-semibold text-ink">Your meeting copilot: never lose a thread.</p>
          <ul className="mt-8 flex flex-col gap-4 text-body-lg text-ink-muted">
            <li className="flex items-center gap-3"><Translate size={20} weight="duotone" />Live transcripts, suggestions and follow-ups</li>
            <li className="flex items-center gap-3"><Microphone size={20} weight="duotone" />Works with Google Meet, Zoom, Teams and in-person meetings</li>
            <li className="flex items-center gap-3"><Brain size={20} weight="duotone" />Minutes and action items, ready when the meeting ends</li>
          </ul>
        </div>
        <p className="text-caption text-ink-subtle">WarpTalk</p>
      </aside>
      <main id="main" tabIndex={-1} className="outline-none flex min-w-0 flex-col items-center justify-center gap-6 px-4 py-10">
        <Brand className="h-6 w-[98px] lg:hidden" />
        <Card className="w-full max-w-sm">
          <CardBody className="flex flex-col gap-5 p-6">
            <div>
              <h1 className="text-card-title font-semibold tracking-[-0.4px] text-ink">{title}</h1>
              {subtitle && <p className="mt-1 text-body text-ink-subtle">{subtitle}</p>}
            </div>
            {children}
          </CardBody>
        </Card>
        {footer && <p className="text-body text-ink-subtle [&_a]:underline">{footer}</p>}
      </main>
    </div>
  );
}

/** "Resend verification email": the API answers 202 whether or not the account exists, so the confirmation is neutral. */
function ResendVerification({ email: initial }: { email: string }) {
  const [email, setEmail] = useState(initial);
  const [state, setState] = useState<'idle' | 'busy' | 'sent'>('idle');
  const [error, setError] = useState('');
  const needsEmail = !initial;
  async function send() {
    if (!EMAIL.test(email.trim())) { setError('Enter a valid email address.'); return; }
    setError('');
    setState('busy');
    try {
      await api('/auth/resend-verification', { method: 'POST', body: { email: email.trim() }, auth: false });
      setState('sent');
    } catch (err) {
      setError(errMsg(err));
      setState('idle');
    }
  }
  if (state === 'sent') return <Alert tone="success">If that account exists and is not verified yet, we sent a new verification link.</Alert>;
  return (
    <div className="flex flex-col gap-3">
      {needsEmail && <Field label="Email for the new link" error={error || undefined}><Input type="email" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} /></Field>}
      {!needsEmail && error && <Alert tone="danger">{error}</Alert>}
      <Button type="button" onClick={() => void send()} loading={state === 'busy'}>Resend verification email</Button>
    </div>
  );
}

export function ForgotPasswordPage() {
  const [params] = useSearchParams();
  const [email, setEmail] = useState(params.get('email') ?? '');
  const [fe, setFe] = useState<string | undefined>();
  const [error, setError] = useState<string | null>(null);
  const [sent, setSent] = useState(false);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;
    const msg = !email.trim() ? 'Enter your email.' : !EMAIL.test(email.trim()) ? 'Enter a valid email address.' : undefined;
    setFe(msg);
    setError(null);
    if (msg) { focusFirstInvalid(form); return; }
    setBusy(true);
    try {
      await api('/auth/forgot-password', { method: 'POST', body: { email: email.trim() }, auth: false });
      setSent(true);
    } catch (err) {
      setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <AuthLayout title="Reset your password" subtitle="Enter your email and we will send you a link to choose a new password." footer={<Link to="/login">Back to sign in</Link>}>
      {sent ? (
        <Alert tone="success" title="Check your inbox">If an account exists for {email.trim()}, a reset link is on its way. The link expires after a short time.</Alert>
      ) : (
        <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
          <Field label="Email" error={fe}><Input type="email" autoComplete="email" aria-required="true" value={email} onChange={(e) => setEmail(e.target.value)} /></Field>
          {error && <Alert tone="danger">{error}</Alert>}
          <Button type="submit" variant="primary" size="lg" loading={busy}>Send reset link</Button>
        </form>
      )}
    </AuthLayout>
  );
}

export function ResetPasswordPage() {
  const [params] = useSearchParams();
  const nav = useNavigate();
  const token = params.get('token') ?? '';
  const [password, setPassword] = useState('');
  const [confirm, setConfirm] = useState('');
  const [fe, setFe] = useState<{ password?: string; confirm?: string }>({});
  const [error, setError] = useState<string | null>(null);
  const [invalid, setInvalid] = useState(!token);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;
    const next = {
      password: password.length < 8 ? 'Password must be at least 8 characters.' : undefined,
      confirm: confirm !== password ? 'Passwords do not match.' : undefined,
    };
    setFe(next);
    setError(null);
    if (next.password || next.confirm) { focusFirstInvalid(form); return; }
    setBusy(true);
    try {
      await api('/auth/reset-password', { method: 'POST', body: { token, newPassword: password }, auth: false });
      nav('/login?reset=1', { replace: true });
    } catch (err) {
      if (errCode(err) === 'INVALID_TOKEN') setInvalid(true); else setError(errMsg(err));
    } finally {
      setBusy(false);
    }
  }

  if (invalid) {
    return (
      <AuthLayout title="Reset link not valid" footer={<Link to="/login">Back to sign in</Link>}>
        <Alert tone="danger">This reset link is invalid, already used or has expired. Request a new one to continue.</Alert>
        <Link to="/forgot-password" className="inline-flex h-9 items-center justify-center rounded-md bg-primary px-5 font-medium text-on-primary no-underline hover:bg-primary-hover hover:no-underline">Request a new link</Link>
      </AuthLayout>
    );
  }
  return (
    <AuthLayout title="Choose a new password" subtitle="You will be asked to sign in again afterwards." footer={<Link to="/login">Back to sign in</Link>}>
      <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
        <Field label="New password" hint="At least 8 characters." error={fe.password}><PasswordInput autoComplete="new-password" aria-required="true" value={password} onChange={(e) => setPassword(e.target.value)} /></Field>
        <Field label="Confirm new password" error={fe.confirm}><PasswordInput autoComplete="new-password" aria-required="true" value={confirm} onChange={(e) => setConfirm(e.target.value)} /></Field>
        {error && <Alert tone="danger">{error}</Alert>}
        <Button type="submit" variant="primary" size="lg" loading={busy}>Reset password</Button>
      </form>
    </AuthLayout>
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
  const [fe, setFe] = useState<{ email?: string; password?: string }>({});
  const [busy, setBusy] = useState(false);
  const from = (loc.state as { from?: string } | null)?.from ?? '/';
  const { loginWithGoogle } = useAuth();

  if (status === 'authed') return <Navigate to={from} replace />;

  async function google(idToken: string) {
    setError(null);
    try {
      await loginWithGoogle(idToken);
      nav(from, { replace: true });
    } catch (err) {
      setError({ code: errCode(err), message: googleErrorMessage(errCode(err), errMsg(err)) });
    }
  }

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;
    const next = {
      email: !email.trim() ? 'Enter your email.' : !EMAIL.test(email.trim()) ? 'Enter a valid email address.' : undefined,
      password: !password ? 'Enter your password.' : undefined,
    };
    setFe(next);
    setError(null);
    if (next.email || next.password) { focusFirstInvalid(form); return; }
    setBusy(true);
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
      {params.get('reset') === '1' && <Alert tone="success" title="Password updated">Sign in with your new password.</Alert>}
      <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
        <Field label="Email" error={fe.email}><Input type="email" autoComplete="email" aria-required="true" value={email} onChange={(e) => setEmail(e.target.value)} /></Field>
        <Field label="Password" error={fe.password}><PasswordInput autoComplete="current-password" aria-required="true" value={password} onChange={(e) => setPassword(e.target.value)} /></Field>
        <p className="-mt-2 text-right text-body-sm"><Link to={`/forgot-password${email.trim() ? `?email=${encodeURIComponent(email.trim())}` : ''}`}>Forgot password?</Link></p>
        {error && (
          <Alert tone="danger">
            {error.message}
            {error.code === 'EMAIL_NOT_VERIFIED' && <> <Link to={`/verify-email?email=${encodeURIComponent(email)}`}>Verify your email</Link>.</>}
          </Alert>
        )}
        {error?.code === 'EMAIL_NOT_VERIFIED' && EMAIL.test(email.trim()) && <ResendVerification email={email.trim()} />}
        <Button type="submit" variant="primary" size="lg" loading={busy}>Sign in</Button>
      </form>
      <GoogleButton onCredential={(t) => void google(t)} onLoadError={(m) => setError({ message: m })} />
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
  const [fe, setFe] = useState<{ fullName?: string; email?: string; password?: string }>({});
  const [busy, setBusy] = useState(false);
  const { loginWithGoogle } = useAuth();
  if (status === 'authed') return <Navigate to="/" replace />;

  async function google(idToken: string) {
    setError(null);
    try {
      await loginWithGoogle(idToken);
      nav('/', { replace: true });
    } catch (err) {
      setError(googleErrorMessage(errCode(err), errMsg(err)));
    }
  }

  async function submit(e: FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const form = e.currentTarget;
    const next = {
      fullName: !fullName.trim() ? 'Enter your full name.' : undefined,
      email: !email.trim() ? 'Enter your email.' : !EMAIL.test(email.trim()) ? 'Enter a valid email address.' : undefined,
      password: password.length < 8 ? 'Password must be at least 8 characters.' : undefined,
    };
    setFe(next);
    setError(null);
    if (next.fullName || next.email || next.password) { focusFirstInvalid(form); return; }
    setBusy(true);
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
    <AuthLayout title="Create your account" subtitle="A meeting copilot with transcripts, suggestions and minutes." footer={<>Already have an account? <Link to="/login">Sign in</Link></>}>
      <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
        <Field label="Full name" error={fe.fullName}><Input autoComplete="name" aria-required="true" value={fullName} onChange={(e) => setFullName(e.target.value)} /></Field>
        <Field label="Email" error={fe.email}><Input type="email" autoComplete="email" aria-required="true" value={email} onChange={(e) => setEmail(e.target.value)} /></Field>
        <Field label="Password" hint="At least 8 characters." error={fe.password}><PasswordInput autoComplete="new-password" aria-required="true" value={password} onChange={(e) => setPassword(e.target.value)} /></Field>
        {error && <Alert tone="danger">{error}</Alert>}
        <Button type="submit" variant="primary" size="lg" loading={busy}>Create account</Button>
      </form>
      <GoogleButton text="signup_with" onCredential={(t) => void google(t)} onLoadError={setError} />
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
          <Link to={`/login${email ? `?email=${encodeURIComponent(email)}` : ''}`} className="inline-flex h-9 items-center justify-center rounded-md bg-primary px-5 font-medium text-on-primary no-underline hover:bg-primary-hover hover:no-underline">Continue to sign in</Link>
        </>
      ) : (
        <form onSubmit={(e) => { e.preventDefault(); void verify(token); }} className="flex flex-col gap-4">
          <Alert tone="info" title="Check your inbox">
            Open the link in the email we sent, or paste the verification token below.
          </Alert>
          <Field label="Verification token"><Input autoComplete="one-time-code" required value={token} onChange={(e) => setToken(e.target.value)} /></Field>
          {state === 'error' && <Alert tone="danger">{error}</Alert>}
          <Button type="submit" variant="primary" size="lg" loading={state === 'busy'} disabled={!token.trim()}>Verify email</Button>
          {state === 'error' && <ResendVerification email={email ?? ''} />}
        </form>
      )}
    </AuthLayout>
  );
}
