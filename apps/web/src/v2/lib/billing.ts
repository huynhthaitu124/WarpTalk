// Billing API (backend: /billing/plans, /workspaces/{id}/billing[/checkout|portal|usage|transactions]).
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { api } from './api';
import { getNative } from './native';
import { page } from './schemas';

const num = z.coerce.number();
const optNum = z.coerce.number().nullable().optional();

export const plan = z.object({
  id: z.string(),
  name: z.string(),
  slug: z.string().nullable().optional(),
  tier: z.union([z.string(), z.number()]).nullable().optional(),
  price: num,
  currency: z.string().nullable().optional(),
  billingCycle: z.string().nullable().optional(),
  creditsPerCycle: optNum,
  maxParticipants: optNum,
  maxLanguages: optNum,
  maxActiveRooms: optNum,
  overageCapCredits: optNum,
  rolloverCapCredits: optNum,
  features: z.unknown().optional(),
  purchasable: z.boolean().optional(),
});
export type Plan = z.infer<typeof plan>;

/** Identifiers (id, slug, name; lower-case) of the plan a subscription is on. */
export function subscriptionPlanKeys(sub: { plan?: unknown } | null | undefined): string[] {
  const p = sub?.plan;
  if (!p) return [];
  if (typeof p === 'string') return [p.toLowerCase()];
  const o = p as { id?: string; slug?: string | null; name?: string };
  return [o.id, o.slug, o.name].filter((x): x is string => !!x).map((x) => x.toLowerCase());
}

export const SERVICE_STATES = ['healthy', 'low_balance', 'in_overage', 'suspended'] as const;
export type ServiceState = (typeof SERVICE_STATES)[number];

const subscription = z.object({
  id: z.string().optional(),
  /** The API embeds the full plan; a bare slug/name string is tolerated. */
  plan: z.union([z.string(), plan.partial().extend({ id: z.string().optional() })]).nullable().optional(),
  status: z.string(),
  currentPeriodStart: z.string().nullable().optional(),
  currentPeriodEnd: z.string().nullable().optional(),
  autoRenew: z.boolean().optional(),
  trialEndsAt: z.string().nullable().optional(),
  cancelledAt: z.string().nullable().optional(),
});

export const billingOverview = z.object({
  workspaceId: z.string(),
  subscription: subscription.nullable().optional(),
  balance: num,
  creditsUsedThisCycle: num.optional(),
  serviceState: z.string(),
  suspendedReason: z.string().nullable().optional(),
  usage: z.object({
    from: z.string().optional(), to: z.string().optional(),
    creditsConsumed: num.optional(), audioSeconds: num.optional(), tokens: num.optional(), events: num.optional(),
  }).nullable().optional(),
});
export type BillingOverview = z.infer<typeof billingOverview>;

export const usageGroups = z.object({
  from: z.string().optional(), to: z.string().optional(), groupBy: z.string().optional(),
  groups: z.array(z.object({ key: z.string(), credits: num, quantity: num.optional(), events: num.optional() })),
});
export type UsageGroupBy = 'day' | 'feature' | 'member';

export const transaction = z.object({
  id: z.string(),
  amount: num,
  type: z.string(),
  chargeType: z.string().nullable().optional(),
  description: z.string().nullable().optional(),
  balanceAfter: num.nullable().optional(),
  referenceId: z.string().nullable().optional(),
  referenceType: z.string().nullable().optional(),
  createdAt: z.string(),
});

export const usePlans = () => useQuery({ queryKey: ['billing-plans'], queryFn: () => api('/billing/plans', { schema: z.array(plan) }), staleTime: 5 * 60_000 });

export const useBilling = (wsId: string | undefined) => useQuery({
  queryKey: ['billing', wsId],
  enabled: !!wsId,
  queryFn: () => api(`/workspaces/${wsId}/billing`, { schema: billingOverview }),
  staleTime: 30_000,
  retry: false,
});

export const useUsage = (wsId: string | undefined, groupBy: UsageGroupBy, days = 30) => useQuery({
  queryKey: ['billing-usage', wsId, groupBy, days],
  enabled: !!wsId,
  queryFn: () => {
    const to = new Date();
    const from = new Date(to.getTime() - days * 86_400_000);
    return api(`/workspaces/${wsId}/billing/usage`, { query: { from: from.toISOString(), to: to.toISOString(), groupBy }, schema: usageGroups });
  },
});

export const TX_PAGE_SIZE = 10;
export const useTransactions = (wsId: string | undefined, pageNo: number) => useQuery({
  queryKey: ['billing-tx', wsId, pageNo],
  enabled: !!wsId,
  queryFn: () => api(`/workspaces/${wsId}/billing/transactions`, { query: { page: pageNo, pageSize: TX_PAGE_SIZE }, schema: page(transaction) }),
  placeholderData: (prev) => prev,
});

const urlBody = z.object({ url: z.string() });

/** Opens a Stripe URL: the macOS host opens the system browser when its bridge offers `openExternal`, otherwise navigate. */
export function openExternalUrl(url: string) {
  const native = getNative() as (ReturnType<typeof getNative> & { openExternal?: (u: string) => Promise<unknown> }) | undefined;
  if (native && typeof native.openExternal === 'function') { void native.openExternal(url).catch(() => window.location.assign(url)); return; }
  window.location.assign(url);
}

export function useCheckout(wsId: string) {
  return useMutation({
    mutationFn: (body: { planId: string } | { topUpCredits: number }) => api(`/workspaces/${wsId}/billing/checkout`, { method: 'POST', body, schema: urlBody }),
    onSuccess: (r) => openExternalUrl(r.url),
  });
}
export function usePortal(wsId: string) {
  return useMutation({
    mutationFn: () => api(`/workspaces/${wsId}/billing/portal`, { method: 'POST', schema: urlBody }),
    onSuccess: (r) => openExternalUrl(r.url),
  });
}
export function useRefreshBilling(wsId: string) {
  const qc = useQueryClient();
  return () => Promise.all([qc.invalidateQueries({ queryKey: ['billing', wsId] }), qc.invalidateQueries({ queryKey: ['billing-tx', wsId] }), qc.invalidateQueries({ queryKey: ['billing-usage', wsId] })]);
}

export function billingErrorMessage(code: string | undefined, fallback: string): string {
  switch (code) {
    case 'STRIPE_NOT_CONFIGURED': return 'Online payments are not set up on this server yet. Contact your administrator.';
    case 'FORBIDDEN': return 'Only the workspace owner can change billing.';
    default: return fallback;
  }
}
