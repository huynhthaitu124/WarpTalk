import { useMemo, useState, type FormEvent } from 'react';
import { CheckCircle, Key, Lock, WarningCircle } from '@phosphor-icons/react';
import { Alert, Badge, Button, Card, CardHeader, Checkbox, EmptyState, Field, Input, Select, Skeleton, Switch, useToast } from '@/ui';
import { ApiError } from '@/lib/api';
import {
  useAiConfig, useDeleteCredential, usePutAiConfig, usePutCredential, useTestCredential, type AiPurposeInput,
} from '@/lib/queries';
import { AI_PROVIDERS, AI_PURPOSES, type AiConfig, type AiProvider, type AiPurpose, type Workspace } from '@/lib/schemas';
import { formatWhen } from './common';
import { errCode, errMsg } from './errors';

export const PROVIDER_LABEL: Record<AiProvider, string> = { openai: 'OpenAI', anthropic: 'Anthropic', gemini: 'Google Gemini', openai_compatible: 'OpenAI-compatible endpoint' };
const PURPOSE_LABEL: Record<AiPurpose, { label: string; hint: string }> = {
  assistant: { label: 'Assistant', hint: 'Answers questions about your meetings and documents.' },
  summary: { label: 'Meeting summaries', hint: 'Summaries and action items after a meeting.' },
  suggestion_decide: { label: 'Suggestions: decide', hint: 'Decides whether a live suggestion is worth showing.' },
  suggestion_generate: { label: 'Suggestions: write', hint: 'Writes the live suggestion text.' },
  embedding: { label: 'Embeddings', hint: 'Indexes documents and transcripts for Knowledge search. Changing it re-indexes.' },
};

function ProviderRow({ ws, provider, config, disabled }: { ws: Workspace; provider: AiProvider; config: AiConfig; disabled: boolean }) {
  const cred = config.credentials.find((c) => c.provider === provider);
  const put = usePutCredential(ws.id);
  const del = useDeleteCredential(ws.id);
  const test = useTestCredential(ws.id);
  const toast = useToast();
  const [key, setKey] = useState('');
  const [baseUrl, setBaseUrl] = useState('');
  const [replacing, setReplacing] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const compat = provider === 'openai_compatible';
  const showForm = !cred || replacing;

  async function save(e: FormEvent) {
    e.preventDefault();
    try {
      await put.mutateAsync({ provider, apiKey: key.trim(), baseUrl: compat ? baseUrl.trim() : undefined });
      setKey(''); setReplacing(false); test.reset();
      toast({ tone: 'success', title: `${PROVIDER_LABEL[provider]} key saved` });
    } catch { /* inline */ }
  }
  async function remove() {
    try { await del.mutateAsync(provider); setConfirmDelete(false); test.reset(); toast({ tone: 'success', title: `${PROVIDER_LABEL[provider]} key removed` }); } catch { /* inline */ }
  }
  const name = PROVIDER_LABEL[provider];
  return (
    <li className="flex flex-col gap-3 px-5 py-4" data-testid={`provider-${provider}`}>
      <div className="flex flex-wrap items-center gap-2">
        <Key size={16} weight="duotone" aria-hidden className="text-ink-subtle" />
        <span className="text-body font-medium text-ink">{name}</span>
        {cred ? <Badge tone="success"><Lock size={12} aria-hidden />Key ending {cred.last4}</Badge> : <Badge>No key</Badge>}
        {cred?.validatedAt && <span className="text-caption text-ink-subtle">checked {formatWhen(cred.validatedAt)}</span>}
      </div>
      {cred && !replacing && (
        <div className="flex flex-wrap items-center gap-2">
          <Button size="sm" loading={test.isPending} disabled={disabled} onClick={() => test.mutate(provider)} aria-label={`Test key for ${name}`}>Test key</Button>
          <Button size="sm" disabled={disabled} onClick={() => setReplacing(true)} aria-label={`Replace key for ${name}`}>Replace key</Button>
          {confirmDelete ? (
            <>
              <Button size="sm" variant="danger" loading={del.isPending} onClick={() => void remove()} aria-label={`Confirm remove key for ${name}`}>Confirm remove</Button>
              <Button size="sm" variant="ghost" onClick={() => setConfirmDelete(false)}>Keep</Button>
            </>
          ) : (
            <Button size="sm" variant="danger-soft" disabled={disabled} onClick={() => setConfirmDelete(true)} aria-label={`Remove key for ${name}`}>Remove</Button>
          )}
          {test.data && (test.data.ok
            ? <span role="status" className="flex items-center gap-1 text-body-sm text-success-text"><CheckCircle size={16} weight="fill" aria-hidden />Works ({test.data.latencyMs} ms)</span>
            : <span role="alert" className="flex items-center gap-1 text-body-sm text-danger-text"><WarningCircle size={16} weight="fill" aria-hidden />Failed{test.data.errorCode ? `: ${test.data.errorCode}` : ''}</span>)}
          {test.isError && <span role="alert" className="text-body-sm text-danger-text">{errMsg(test.error)}</span>}
        </div>
      )}
      {showForm && (
        <form onSubmit={save} className="flex max-w-lg flex-col gap-3" noValidate>
          <Field label={`${name} API key`} hint="Stored encrypted. It is never shown again; only the last 4 characters are.">
            <Input type="password" autoComplete="off" spellCheck={false} disabled={disabled} value={key} onChange={(e) => setKey(e.target.value)} placeholder="Paste a new key" />
          </Field>
          {compat && (
            <Field label="Base URL" hint="For example https://llm.example.com/v1"><Input type="url" disabled={disabled} value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} /></Field>
          )}
          {put.isError && <Alert tone="danger" title={errCode(put.error) ?? undefined}>{errMsg(put.error)}</Alert>}
          <div className="flex gap-2">
            <Button type="submit" variant="primary" size="sm" loading={put.isPending} disabled={disabled || !key.trim() || (compat && !baseUrl.trim())} aria-label={`Save key for ${name}`}>Save key</Button>
            {replacing && <Button size="sm" variant="ghost" onClick={() => { setReplacing(false); setKey(''); put.reset(); }}>Cancel</Button>}
          </div>
        </form>
      )}
      {del.isError && <Alert tone="danger">{errMsg(del.error)}</Alert>}
    </li>
  );
}

type Draft = Record<AiPurpose, { provider: string; model: string; baseUrl: string; enabled: boolean; fallback: boolean; boundary: string }>;

function initialDraft(config: AiConfig): Draft {
  return Object.fromEntries(AI_PURPOSES.map((p) => {
    const c = config.purposes[p];
    return [p, { provider: c?.provider ?? '', model: c?.model ?? '', baseUrl: c?.baseUrl ?? '', enabled: c?.enabled ?? true, fallback: c?.fallbackPolicy === 'platform_if_allowed', boundary: c?.dataBoundary ?? 'external' }];
  })) as Draft;
}

function PurposesCard({ ws, config, disabled }: { ws: Workspace; config: AiConfig; disabled: boolean }) {
  const put = usePutAiConfig(ws.id);
  const toast = useToast();
  const [draft, setDraft] = useState<Draft>(() => initialDraft(config));
  const [ack, setAck] = useState(false);
  const [tried, setTried] = useState(false);
  const set = (p: AiPurpose, patch: Partial<Draft[AiPurpose]>) => setDraft((d) => ({ ...d, [p]: { ...d[p], ...patch } }));
  const hasKey = (prov: string) => config.credentials.some((c) => c.provider === prov);
  const needsAck = AI_PURPOSES.some((p) => draft[p].provider && draft[p].boundary === 'workspace_controlled');
  const options = [{ value: 'platform', label: 'Platform default' }, ...AI_PROVIDERS.map((p) => ({ value: p, label: PROVIDER_LABEL[p] }))];
  const problems = useMemo(() => AI_PURPOSES.flatMap((p) => {
    const d = draft[p];
    if (!d.provider) return [];
    if (!d.model.trim()) return [`${PURPOSE_LABEL[p].label}: choose a model.`];
    if (!hasKey(d.provider)) return [`${PURPOSE_LABEL[p].label}: add a ${PROVIDER_LABEL[d.provider as AiProvider]} key first.`];
    return [];
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }), [draft, config.credentials]);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setTried(true);
    if (problems.length > 0) return;
    const purposes: Record<string, AiPurposeInput | null> = {};
    for (const p of AI_PURPOSES) {
      const d = draft[p];
      purposes[p] = d.provider ? {
        provider: d.provider, model: d.model.trim(), baseUrl: d.provider === 'openai_compatible' ? d.baseUrl.trim() || null : null,
        enabled: d.enabled, fallbackPolicy: d.fallback ? 'platform_if_allowed' : 'none', dataBoundary: d.boundary,
      } : null;
    }
    try {
      const r = await put.mutateAsync({ purposes, acknowledgeWorkspaceControlled: needsAck && ack });
      toast({ tone: 'success', title: 'AI configuration saved', description: r.reindexRequired ? 'Embeddings changed: documents will be re-indexed.' : undefined });
    } catch { /* inline */ }
  }
  return (
    <Card>
      <CardHeader title="What uses which model" description="Pick a provider and model per task, or leave it on the platform default." />
      <form onSubmit={submit} noValidate>
        <ul className="divide-y divide-hairline">
          {AI_PURPOSES.map((p) => {
            const d = draft[p];
            const custom = !!d.provider;
            return (
              <li key={p} className="flex flex-col gap-3 px-5 py-4" data-testid={`purpose-${p}`}>
                <div>
                  <p className="text-body font-medium text-ink">{PURPOSE_LABEL[p].label}</p>
                  <p className="text-caption text-ink-subtle">{PURPOSE_LABEL[p].hint}</p>
                </div>
                <div className="grid gap-3 sm:grid-cols-2">
                  <Field label="Provider">
                    <Select aria-label={`${PURPOSE_LABEL[p].label} provider`} disabled={disabled} value={d.provider || 'platform'} onValueChange={(v) => set(p, { provider: v === 'platform' ? '' : v })} options={options} />
                  </Field>
                  {custom && <Field label="Model"><Input disabled={disabled} value={d.model} onChange={(e) => set(p, { model: e.target.value })} placeholder="e.g. gpt-4o-mini" maxLength={200} /></Field>}
                  {custom && d.provider === 'openai_compatible' && <Field label="Base URL" className="sm:col-span-2"><Input type="url" disabled={disabled} value={d.baseUrl} onChange={(e) => set(p, { baseUrl: e.target.value })} /></Field>}
                </div>
                {custom && (
                  <div className="flex flex-col gap-2">
                    <Switch label="Enabled" checked={d.enabled} disabled={disabled} onCheckedChange={(v) => set(p, { enabled: v })} />
                    <Checkbox label="Fall back to the platform default if my provider fails" checked={d.fallback} onCheckedChange={(v) => set(p, { fallback: v })} />
                    <Field label="Data boundary" hint="Where your provider processes data.">
                      <Select aria-label={`${PURPOSE_LABEL[p].label} data boundary`} disabled={disabled} value={d.boundary} onValueChange={(v) => set(p, { boundary: v })} options={[{ value: 'external', label: 'External provider' }, { value: 'workspace_controlled', label: 'Controlled by my workspace' }]} />
                    </Field>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
        <div className="flex flex-col gap-3 border-t border-hairline px-5 py-4">
          {needsAck && <Checkbox label="I confirm the workspace controls where this data is processed" checked={ack} onCheckedChange={setAck} />}
          {tried && problems.length > 0 && <Alert tone="warning"><ul className="list-disc pl-4">{problems.map((m) => <li key={m}>{m}</li>)}</ul></Alert>}
          {put.isError && <Alert tone="danger" title={errCode(put.error) ?? undefined}>{errMsg(put.error)}</Alert>}
          <div><Button type="submit" variant="primary" loading={put.isPending} disabled={disabled}>Save configuration</Button></div>
        </div>
      </form>
    </Card>
  );
}

/** Bring-your-own AI: provider keys (write-only, last 4 shown) and per-task models. Owner/admin only. */
export function AiSettings({ ws }: { ws: Workspace }) {
  const q = useAiConfig(ws.id, true);
  if (q.isLoading) return <Skeleton className="h-64" />;
  if (q.isError) {
    const forbidden = q.error instanceof ApiError && q.error.status === 403;
    return forbidden
      ? <EmptyState icon={<Lock size={20} weight="duotone" />} title="Owners and admins only" description="Ask a workspace owner or admin to manage AI providers." />
      : <Alert tone="danger" title="Could not load AI settings">{errMsg(q.error)}</Alert>;
  }
  const config = q.data!;
  const versionKey = AI_PURPOSES.map((p) => config.purposes[p]?.version ?? 0).join('.');
  return (
    <div className="flex flex-col gap-6">
      {!config.byoEnabled && <Alert tone="warning" title="Bring-your-own AI is switched off">This deployment has no key-encryption secret, so provider keys cannot be stored. Ask the platform operator to enable it.</Alert>}
      <Card>
        <CardHeader title="Provider keys" description="Keys are encrypted at rest and cannot be read back, only replaced or removed." />
        <ul className="divide-y divide-hairline" aria-label="Provider keys">
          {AI_PROVIDERS.map((p) => <ProviderRow key={p} ws={ws} provider={p} config={config} disabled={!config.byoEnabled} />)}
        </ul>
      </Card>
      <PurposesCard key={versionKey} ws={ws} config={config} disabled={!config.byoEnabled} />
    </div>
  );
}
