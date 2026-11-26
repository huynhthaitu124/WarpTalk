import { useEffect, useState, type FormEvent } from 'react';
import { Link, useSearchParams } from 'react-router-dom';
import { Alert, Badge, Button, Card, CardBody, CardHeader, EmptyState, Field, Input, PageBody, PageHeader, SectionLabel, Skeleton, Table, TBody, TD, TH, THead, TR } from '@v2/ui';
import { cn } from '@v2/lib/cn';
import { useWorkspace } from '@v2/lib/workspace';
import {
  billingErrorMessage, subscriptionPlanKeys, TX_PAGE_SIZE, useBilling, useCheckout, usePlans, usePortal, useRefreshBilling, useTransactions, useUsage,
  type BillingOverview, type Plan, type UsageGroupBy,
} from '@v2/lib/billing';
import type { Workspace } from '@v2/lib/schemas';
import { errCode, errMsg } from './errors';

const nf = new Intl.NumberFormat('en-US', { maximumFractionDigits: 2 });
export const fmtCredits = (n: number) => nf.format(n);
const fmtDate = (s?: string | null) => (s ? new Date(s).toLocaleDateString('en-US', { year: 'numeric', month: 'short', day: 'numeric', timeZone: 'UTC' }) : '—');
const fmtDateTime = (s: string) => new Date(s).toLocaleString('en-US', { dateStyle: 'medium', timeStyle: 'short', timeZone: 'UTC' });

function money(p: Plan): string {
  if (!p.price) return 'Free';
  try { return new Intl.NumberFormat('en-US', { style: 'currency', currency: (p.currency || 'USD').toUpperCase(), maximumFractionDigits: 2 }).format(p.price); } catch { return `${p.price} ${p.currency ?? ''}`; }
}
const cycleLabel = (c?: string | null) => (!c ? '' : /year|annual/i.test(c) ? '/ year' : /month/i.test(c) ? '/ month' : `/ ${c.toLowerCase()}`);

const STATE_TEXT: Record<string, { tone: 'info' | 'warning' | 'danger'; title: string; body: string }> = {
  low_balance: { tone: 'warning', title: 'Credit balance is running low', body: 'Top up or upgrade your plan to keep live sessions going.' },
  in_overage: { tone: 'warning', title: 'You are using overage credits', body: 'The plan credits are used up. Usage continues up to the overage cap, then service pauses.' },
  suspended: { tone: 'danger', title: 'Service is suspended', body: 'Live transcription and AI features are paused for this workspace until credits are added.' },
};

/** 402 INSUFFICIENT_CREDITS from the API (session create, STT token): explains it and links to Billing. */
export function InsufficientCreditsAlert() {
  const { wsPath } = useWorkspace();
  return (
    <Alert tone="danger" title="Not enough credits">
      This workspace has run out of credits, so live transcription cannot start. <Link to={wsPath('/billing')} className="font-medium underline">Open billing</Link> to top up or upgrade.
    </Alert>
  );
}

/** Workspace-wide notice for low_balance / in_overage / suspended (rendered by the shell above every page). */
export function BillingBanner() {
  const { current, wsPath } = useWorkspace();
  const q = useBilling(current?.id);
  const s = q.data?.serviceState;
  const t = s ? STATE_TEXT[s] : undefined;
  if (!current || !t) return null;
  return (
    <div className="px-4 pt-3" data-testid="billing-banner" data-state={s}>
      <Alert tone={t.tone} title={t.title}>
        {t.body}{s === 'suspended' && q.data?.suspendedReason ? ` (${q.data.suspendedReason})` : ''}{' '}
        <Link to={wsPath('/billing')} className="font-medium underline">Open billing</Link>
      </Alert>
    </div>
  );
}

const STATUS_TONE: Record<string, 'success' | 'info' | 'warning' | 'danger' | 'neutral'> = { active: 'success', trialing: 'info', past_due: 'warning', cancelled: 'danger' };
const STATUS_LABEL: Record<string, string> = { active: 'Active', trialing: 'Trial', past_due: 'Past due', cancelled: 'Cancelled' };
const SERVICE_LABEL: Record<string, string> = { healthy: 'Healthy', low_balance: 'Low balance', in_overage: 'In overage', suspended: 'Suspended' };
const SERVICE_TONE: Record<string, 'success' | 'warning' | 'danger'> = { healthy: 'success', low_balance: 'warning', in_overage: 'warning', suspended: 'danger' };

function Stat({ label, value, hint }: { label: string; value: string; hint?: string }) {
  return (
    <div className="min-w-0 rounded-lg border border-hairline bg-surface-1 px-4 py-3">
      <p className="text-body-sm text-ink-muted">{label}</p>
      <p className="mt-1 text-headline font-semibold text-ink">{value}</p>
      {hint && <p className="mt-0.5 text-caption text-ink-subtle">{hint}</p>}
    </div>
  );
}

function PlanCard({ ov, ws, owner, plans }: { ov: BillingOverview; ws: Workspace; owner: boolean; plans: Plan[] }) {
  const portal = usePortal(ws.id);
  const sub = ov.subscription;
  const keys = subscriptionPlanKeys(sub);
  const current = plans.find((p) => [p.id, p.slug, p.name].some((x) => x && keys.includes(x.toLowerCase())));
  const embedded = sub && typeof sub.plan === 'object' && sub.plan ? sub.plan.name : undefined;
  return (
    <Card>
      <CardHeader
        title={current?.name ?? embedded ?? (typeof sub?.plan === 'string' ? sub.plan : undefined) ?? 'No subscription'}
        description={sub ? `Renews ${sub.autoRenew === false ? 'manually' : 'automatically'} · period ${fmtDate(sub.currentPeriodStart)} to ${fmtDate(sub.currentPeriodEnd)}` : 'This workspace is not on a paid plan yet. Pick one below.'}
        actions={sub ? <Badge tone={STATUS_TONE[sub.status] ?? 'neutral'}>{STATUS_LABEL[sub.status] ?? sub.status}</Badge> : undefined}
      />
      <CardBody className="flex flex-col gap-3">
        {sub?.status === 'trialing' && sub.trialEndsAt && <p className="text-body-sm text-ink-muted">Trial ends {fmtDate(sub.trialEndsAt)}.</p>}
        {sub?.status === 'past_due' && <Alert tone="warning">The last payment failed. Update your payment method in the billing portal.</Alert>}
        {sub?.cancelledAt && <p className="text-body-sm text-ink-muted">Cancelled on {fmtDate(sub.cancelledAt)}.</p>}
        {owner && (
          <div className="flex flex-wrap items-center gap-2">
            <Button onClick={() => portal.mutate()} loading={portal.isPending}>Manage in billing portal</Button>
            {portal.isError && <span role="alert" className="text-body-sm text-danger-text">{billingErrorMessage(errCode(portal.error), errMsg(portal.error))}</span>}
          </div>
        )}
      </CardBody>
    </Card>
  );
}

function PlansGrid({ ws, owner, plans, ov }: { ws: Workspace; owner: boolean; plans: Plan[]; ov: BillingOverview }) {
  const checkout = useCheckout(ws.id);
  const [pending, setPending] = useState<string | null>(null);
  const keys = subscriptionPlanKeys(ov.subscription);
  const isCurrent = (p: Plan) => [p.id, p.slug, p.name].some((x) => x && keys.includes(x.toLowerCase()));
  return (
    <section aria-label="Plans" className="flex flex-col gap-3">
      <SectionLabel>Plans</SectionLabel>
      {!owner && <Alert tone="info">Only the workspace owner can change the plan or buy credits. Ask your owner to upgrade.</Alert>}
      {checkout.isError && <Alert tone="danger">{billingErrorMessage(errCode(checkout.error), errMsg(checkout.error))}</Alert>}
      <ul className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        {plans.map((p) => (
          <li key={p.id} data-testid={`plan-${p.slug ?? p.id}`}>
            <Card className={cn('h-full', isCurrent(p) && 'border-primary')}>
              <CardBody className="flex h-full flex-col gap-3">
                <div className="flex items-center justify-between gap-2">
                  <h3 className="text-title font-semibold text-ink">{p.name}</h3>
                  {isCurrent(p) && <Badge tone="primary">Current</Badge>}
                </div>
                <p className="text-ink"><span className="text-headline font-semibold">{money(p)}</span> <span className="text-body-sm text-ink-muted">{p.price ? cycleLabel(p.billingCycle) : ''}</span></p>
                <ul className="flex flex-col gap-1 text-body-sm text-ink-muted">
                  {p.creditsPerCycle != null && <li>{fmtCredits(p.creditsPerCycle)} credits per cycle</li>}
                  {p.maxParticipants != null && <li>Up to {fmtCredits(p.maxParticipants)} participants</li>}
                  {p.maxLanguages != null && <li>Up to {fmtCredits(p.maxLanguages)} languages</li>}
                  {p.maxActiveRooms != null && <li>{fmtCredits(p.maxActiveRooms)} active rooms</li>}
                </ul>
                {owner && (
                  <div className="mt-auto pt-1">
                    {isCurrent(p) ? <Button disabled className="w-full">Current plan</Button>
                      : p.purchasable === false || !p.price ? <Button disabled className="w-full">{p.price ? 'Not available online' : 'Included'}</Button>
                      : <Button variant="primary" className="w-full" loading={checkout.isPending && pending === p.id} disabled={checkout.isPending} aria-label={`Upgrade to ${p.name}`} onClick={() => { setPending(p.id); checkout.mutate({ planId: p.id }); }}>Upgrade</Button>}
                  </div>
                )}
              </CardBody>
            </Card>
          </li>
        ))}
      </ul>
    </section>
  );
}

function TopUp({ ws }: { ws: Workspace }) {
  const checkout = useCheckout(ws.id);
  const [credits, setCredits] = useState('1000');
  const [fe, setFe] = useState<string | undefined>();
  function submit(e: FormEvent) {
    e.preventDefault();
    const n = Number(credits);
    if (!Number.isInteger(n) || n < 1) { setFe('Enter a whole number of credits, at least 1.'); return; }
    setFe(undefined);
    checkout.mutate({ topUpCredits: n });
  }
  return (
    <Card>
      <CardHeader title="Top up credits" description="One-off purchase, added to your balance after payment." />
      <CardBody>
        <form onSubmit={submit} noValidate className="flex flex-wrap items-start gap-3">
          <Field label="Credits" error={fe} className="w-44"><Input type="number" min={1} step={1} inputMode="numeric" value={credits} onChange={(e) => setCredits(e.target.value)} /></Field>
          <div className="pt-[26px]"><Button type="submit" variant="primary" loading={checkout.isPending}>Buy credits</Button></div>
        </form>
        {checkout.isError && <Alert tone="danger" className="mt-3">{billingErrorMessage(errCode(checkout.error), errMsg(checkout.error))}</Alert>}
      </CardBody>
    </Card>
  );
}

function UsageSection({ wsId }: { wsId: string }) {
  const [by, setBy] = useState<UsageGroupBy>('day');
  const q = useUsage(wsId, by);
  const groups = q.data?.groups ?? [];
  const max = Math.max(1, ...groups.map((g) => g.credits));
  const LABEL: Record<UsageGroupBy, string> = { day: 'By day', feature: 'By feature', member: 'By member' };
  return (
    <section aria-label="Usage" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <SectionLabel>Usage, last 30 days</SectionLabel>
        <div role="group" aria-label="Group usage by" className="flex gap-1">
          {(['day', 'feature', 'member'] as const).map((k) => (
            <button key={k} type="button" aria-pressed={by === k} onClick={() => setBy(k)}
              className={cn('h-7 rounded-pill border px-3 text-body-sm font-medium focus-ring', by === k ? 'border-transparent bg-surface-3 text-ink' : 'border-hairline text-ink-muted hover:text-ink')}>{LABEL[k]}</button>
          ))}
        </div>
      </div>
      {q.isLoading ? <Skeleton className="h-32" /> : q.isError ? <Alert tone="danger">{errMsg(q.error)}</Alert> : groups.length === 0 ? (
        <EmptyState title="No usage yet" description="Credits spent in this period will show up here." />
      ) : (
        <Table framed aria-label={`Credits used ${LABEL[by].toLowerCase()}`}>
          <THead><TR><TH>{by === 'day' ? 'Day' : by === 'feature' ? 'Feature' : 'Member'}</TH><TH className="text-right">Credits</TH><TH className="w-1/2 max-sm:hidden"><span className="sr-only">Share</span></TH></TR></THead>
          <TBody>
            {groups.map((g) => (
              <TR key={g.key}>
                <TD className="max-w-[16rem] truncate">{by === 'day' ? fmtDate(g.key) : g.key}</TD>
                <TD className="text-right tabular-nums">{fmtCredits(g.credits)}</TD>
                <TD className="max-sm:hidden"><div aria-hidden className="h-2 rounded-pill bg-surface-2"><div className="h-2 rounded-pill bg-primary" style={{ width: `${Math.max(2, (g.credits / max) * 100)}%` }} /></div></TD>
              </TR>
            ))}
          </TBody>
        </Table>
      )}
    </section>
  );
}

function Transactions({ wsId }: { wsId: string }) {
  const [pageNo, setPageNo] = useState(1);
  const q = useTransactions(wsId, pageNo);
  const total = q.data?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / TX_PAGE_SIZE));
  return (
    <section aria-label="Transactions" className="flex flex-col gap-3">
      <SectionLabel>Transactions</SectionLabel>
      {q.isLoading ? <Skeleton className="h-32" /> : q.isError ? <Alert tone="danger">{errMsg(q.error)}</Alert> : (q.data?.items.length ?? 0) === 0 ? (
        <EmptyState title="No transactions yet" description="Top-ups, plan credits and usage charges appear here." />
      ) : (
        <>
          <Table framed aria-label="Credit transactions">
            <THead><TR><TH>Date</TH><TH>Description</TH><TH className="text-right">Amount</TH><TH className="text-right max-sm:hidden">Balance</TH></TR></THead>
            <TBody>
              {q.data?.items.map((t) => (
                <TR key={t.id}>
                  <TD className="whitespace-nowrap">{fmtDateTime(t.createdAt)}</TD>
                  <TD className="max-w-[18rem] truncate" title={t.description ?? undefined}>{t.description || t.type}<span className="ml-2 text-caption text-ink-subtle">{t.chargeType ?? t.type}</span></TD>
                  <TD className={cn('text-right tabular-nums', t.amount < 0 ? 'text-danger-text' : 'text-success-text')}>{t.amount > 0 ? '+' : ''}{fmtCredits(t.amount)}</TD>
                  <TD className="text-right tabular-nums max-sm:hidden">{t.balanceAfter != null ? fmtCredits(t.balanceAfter) : '—'}</TD>
                </TR>
              ))}
            </TBody>
          </Table>
          <nav aria-label="Transactions pages" className="flex items-center justify-between gap-3 text-body-sm text-ink-muted">
            <span>Page {pageNo} of {pages} · {total} total</span>
            <div className="flex gap-2">
              <Button size="sm" onClick={() => setPageNo((p) => p - 1)} disabled={pageNo <= 1}>Previous</Button>
              <Button size="sm" onClick={() => setPageNo((p) => p + 1)} disabled={pageNo >= pages}>Next</Button>
            </div>
          </nav>
        </>
      )}
    </section>
  );
}

export function BillingPage() {
  const { current, loading } = useWorkspace();
  const [params] = useSearchParams();
  const checkout = params.get('checkout');
  const bill = useBilling(current?.id);
  const plans = usePlans();
  const refresh = useRefreshBilling(current?.id ?? '');
  // Back from Stripe: the webhook may land a moment after the redirect, so refetch now and once more shortly after.
  useEffect(() => {
    if (checkout !== 'success' || !current) return;
    void refresh();
    const t = setTimeout(() => void refresh(), 4000);
    return () => clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [checkout, current?.id]);

  if (loading) return <PageBody><Skeleton className="h-40" /></PageBody>;
  if (!current) return <PageBody><EmptyState title="No workspace" description="Create a workspace to manage billing." /></PageBody>;
  const owner = current.myRole === 'OWNER';
  const ov = bill.data;
  return (
    <>
      <PageHeader size="page" title="Billing" description={`Plan, credits and usage for ${current.name}.`} />
      <PageBody className="flex flex-col gap-6">
        {checkout === 'success' && <Alert tone="success" title="Payment received">Thank you. Your plan or credits update within a few seconds.</Alert>}
        {checkout === 'cancel' && <Alert tone="info" title="Checkout cancelled">Nothing was charged. You can pick a plan again whenever you are ready.</Alert>}
        {bill.isLoading ? <Skeleton className="h-40" /> : bill.isError || !ov ? <Alert tone="danger" title="Could not load billing">{errMsg(bill.error)}</Alert> : (
          <>
            <PlanCard ov={ov} ws={current} owner={owner} plans={plans.data ?? []} />
            <section aria-label="Balance" className="grid gap-3 sm:grid-cols-3">
              <Stat label="Credit balance" value={fmtCredits(ov.balance)} />
              <Stat label="Used this cycle" value={fmtCredits(ov.creditsUsedThisCycle ?? ov.usage?.creditsConsumed ?? 0)} hint={ov.usage ? `${fmtCredits(ov.usage.events ?? 0)} events` : undefined} />
              <div className="min-w-0 rounded-lg border border-hairline bg-surface-1 px-4 py-3">
                <p className="text-body-sm text-ink-muted">Service</p>
                <p className="mt-2"><Badge tone={SERVICE_TONE[ov.serviceState] ?? 'neutral'} data-testid="service-state">{SERVICE_LABEL[ov.serviceState] ?? ov.serviceState}</Badge></p>
                {ov.suspendedReason && <p className="mt-1 text-caption text-ink-subtle">{ov.suspendedReason}</p>}
              </div>
            </section>
            {plans.isError ? <Alert tone="danger">{errMsg(plans.error)}</Alert> : plans.isLoading ? <Skeleton className="h-40" /> : <PlansGrid ws={current} owner={owner} plans={plans.data ?? []} ov={ov} />}
            {owner && <TopUp ws={current} />}
            <UsageSection wsId={current.id} />
            <Transactions wsId={current.id} />
          </>
        )}
      </PageBody>
    </>
  );
}
