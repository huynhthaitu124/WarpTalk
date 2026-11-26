import { useEffect, useRef, useState, type FormEvent, type ReactNode } from 'react';
import { Link, Navigate, useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { Eye, EyeSlash, Translate, Microphone, Brain } from '@phosphor-icons/react';
import { Alert, Brand, Button, Card, CardBody, Field, Input } from '@v2/ui';
import { useAuth } from '@v2/lib/auth';
import { errCode, errMsg } from './errors';

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
          <p className="text-display-md font-semibold text-ink">Every voice, in every language, live.</p>
          <ul className="mt-8 flex flex-col gap-4 text-body-lg text-ink-muted">
            <li className="flex items-center gap-3"><Translate size={20} weight="duotone" />Live captions and dubbed audio in your language</li>
            <li className="flex items-center gap-3"><Microphone size={20} weight="duotone" />Works with Google Meet, Zoom and Teams</li>
            <li className="flex items-center gap-3"><Brain size={20} weight="duotone" />Meetings become searchable knowledge</li>
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

  if (status === 'authed') return <Navigate to={from} replace />;

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
      <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
        <Field label="Email" error={fe.email}><Input type="email" autoComplete="email" aria-required="true" value={email} onChange={(e) => setEmail(e.target.value)} /></Field>
        <Field label="Password" error={fe.password}><PasswordInput autoComplete="current-password" aria-required="true" value={password} onChange={(e) => setPassword(e.target.value)} /></Field>
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
  const [fe, setFe] = useState<{ fullName?: string; email?: string; password?: string }>({});
  const [busy, setBusy] = useState(false);
  if (status === 'authed') return <Navigate to="/" replace />;

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
    <AuthLayout title="Create your account" subtitle="Real-time translated meetings in minutes." footer={<>Already have an account? <Link to="/login">Sign in</Link></>}>
      <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
        <Field label="Full name" error={fe.fullName}><Input autoComplete="name" aria-required="true" value={fullName} onChange={(e) => setFullName(e.target.value)} /></Field>
        <Field label="Email" error={fe.email}><Input type="email" autoComplete="email" aria-required="true" value={email} onChange={(e) => setEmail(e.target.value)} /></Field>
        <Field label="Password" hint="At least 8 characters." error={fe.password}><PasswordInput autoComplete="new-password" aria-required="true" value={password} onChange={(e) => setPassword(e.target.value)} /></Field>
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
        </form>
      )}
    </AuthLayout>
  );
}
