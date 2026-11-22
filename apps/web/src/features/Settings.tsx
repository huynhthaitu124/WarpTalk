import { useEffect, useMemo, useState, type FormEvent } from 'react';
import { Link, Navigate, useParams, useSearchParams } from 'react-router-dom';
import {
  Alert, Avatar, Badge, Button, Card, CardBody, CardHeader, EmptyState, Field, FieldRow, Input, PageHeader, SectionLabel, SettingsGroup, SettingsRow, Select, Skeleton, StatusPill, Switch, Tabs, TabsContent, TabsList, TabsTrigger, Textarea, useToast,
} from '@/ui';
import { cn } from '@/lib/cn';
import { AiSettings } from './AiSettings';
import { GlobalGlossaryAdmin } from './GlobalGlossary';
import { VoiceSection } from './VoiceProfile';
import { useAuth, useProfile } from '@/lib/auth';
import { canAdmin, useWorkspace } from '@/lib/workspace';
import {
  useEffectiveSettings, usePutSettings, useResetSettings, useRooms, useScopedSettings, useUpdateWorkspaceSettings, useWorkspaceSettings,
  type Namespace, type ScopeKind,
} from '@/lib/queries';
import { useTheme } from '@/lib/theme';
import type { Workspace, WorkspaceSettings } from '@/lib/schemas';
import { ApiError } from '@/lib/api';
import { errMsg } from './errors';
import { visibleSettingsSections } from './settingsNav';
import { IS_DEV } from '@/lib/env';

function ReadOnlyNote({ role }: { role: string }) {
  return <Alert tone="info" className="mb-4" title="Read-only">Your role is {role.toLowerCase()}. Only workspace owners and admins can change these settings.</Alert>;
}

function WorkspaceTab({ ws }: { ws: Workspace }) {
  const q = useWorkspaceSettings(ws.id);
  const save = useUpdateWorkspaceSettings(ws.id);
  const toast = useToast();
  const editable = canAdmin(ws);
  const [draft, setDraft] = useState<WorkspaceSettings | null>(null);
  useEffect(() => { if (q.data) setDraft(q.data); }, [q.data]);
  if (q.isLoading || !draft) return <Skeleton className="h-48" />;
  if (q.isError) return <Alert tone="danger">{errMsg(q.error)}</Alert>;
  const set = <K extends keyof WorkspaceSettings>(k: K, v: WorkspaceSettings[K]) => setDraft({ ...draft, [k]: v });
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!draft) return;
    try { await save.mutateAsync(draft); toast({ tone: 'success', title: 'Workspace settings saved' }); } catch { /* inline */ }
  }
  return (
    <form onSubmit={submit} noValidate className="flex flex-col gap-6">
      <section aria-label="Workspace">
        <SectionLabel>Workspace info</SectionLabel>
        <SettingsGroup>
          <SettingsRow label={ws.name} description={ws.description || 'The workspace you are configuring.'}><div className="sm:text-right"><StatusPill status={ws.myRole} /></div></SettingsRow>
        </SettingsGroup>
      </section>
      <section aria-label="Defaults">
        <SectionLabel>General workspace defaults</SectionLabel>
        {!editable && <ReadOnlyNote role={ws.myRole} />}
        <SettingsGroup>
          <FieldRow label="Max participants" hint="Between 1 and 1000.">
            <Input type="number" min={1} max={1000} value={draft.maxParticipantsLimit} disabled={!editable} onChange={(e) => set('maxParticipantsLimit', Number(e.target.value))} />
          </FieldRow>
          <FieldRow label="Audio retention (days)">
            <Input type="number" min={0} value={draft.audioRetentionDays} disabled={!editable} onChange={(e) => set('audioRetentionDays', Number(e.target.value))} />
          </FieldRow>
          <FieldRow label="Default AI model (optional)">
            <Input value={draft.defaultAiModel ?? ''} disabled={!editable} onChange={(e) => set('defaultAiModel', e.target.value || null)} />
          </FieldRow>
          <div className="px-5 py-4">
            <Switch label="Allow guest access" description="Let people outside the workspace join its rooms." checked={draft.allowGuestAccess} disabled={!editable} onCheckedChange={(v) => set('allowGuestAccess', v)} />
          </div>
        </SettingsGroup>
      </section>
      {save.isError && <Alert tone="danger">{errMsg(save.error)}</Alert>}
      <div><Button type="submit" variant="primary" loading={save.isPending} disabled={!editable}>Save changes</Button></div>
    </form>
  );
}

// ---- runtime settings (suggestion / pipeline)
/** The raw JSON editor is developer tooling: dev builds only. */
const SHOW_RAW = IS_DEV;
type Json = Record<string, unknown>;
type FieldDef = { key: string; label: string; kind: 'bool' | 'number'; step?: number; hint?: string };
const FIELDS: Record<Namespace, FieldDef[]> = {
  suggestion: [
    { key: 'enabled', label: 'Enable AI suggestions', kind: 'bool' },
    { key: 'min_confidence', label: 'Minimum confidence', kind: 'number', step: 0.05, hint: '0 to 1' },
    { key: 'cooldown_seconds', label: 'Cooldown (seconds)', kind: 'number', hint: '0 to 600' },
    { key: 'max_per_meeting', label: 'Max per meeting', kind: 'number', hint: '0 to 200 (0 disables)' },
    { key: 'min_words', label: 'Minimum words', kind: 'number', hint: '1 to 20' },
  ],
  pipeline: [
    { key: 'flash_mode_default', label: 'Flash mode by default', kind: 'bool' },
    { key: 'chunk_duration_ms', label: 'Chunk duration (ms)', kind: 'number', hint: '2000 to 15000' },
    { key: 'vad_threshold', label: 'VAD threshold', kind: 'number', step: 0.01 },
    { key: 'tts_cache_enabled', label: 'TTS cache', kind: 'bool' },
    { key: 'catch_up_max_speed', label: 'Catch-up max speed', kind: 'number', step: 0.05 },
  ],
};

/** f32 values arrive as 0.550000011920929: show what a human typed. */
function tidy(v: Json): Json {
  return Object.fromEntries(Object.entries(v).map(([k, x]) => [k, typeof x === 'number' && !Number.isInteger(x) ? Number(x.toPrecision(6)) : x]));
}

function RuntimeEditor({
  ns, scope, scopeId, initial, editable, overridden, version,
}: { ns: Namespace; scope: ScopeKind; scopeId: string; initial: Json; editable: boolean; overridden: boolean; version?: number | null }) {
  const put = usePutSettings(ns, scope, scopeId);
  const reset = useResetSettings(ns, scope, scopeId);
  const toast = useToast();
  const [draft, setDraft] = useState<Json>(initial);
  const [tab, setTab] = useState<'form' | 'json'>('form');
  const [raw, setRaw] = useState('');
  const [rawError, setRawError] = useState<string | null>(null);

  const parseRaw = (): Json | null => {
    try {
      const v: unknown = JSON.parse(raw);
      if (typeof v !== 'object' || v === null || Array.isArray(v)) throw new Error('Top-level value must be a JSON object');
      return v as Json;
    } catch (e) {
      setRawError(`Invalid JSON: ${e instanceof Error ? e.message : String(e)}`);
      return null;
    }
  };
  function switchTab(next: string) {
    if (next === 'json') { setRaw(JSON.stringify(draft, null, 2)); setRawError(null); setTab('json'); return; }
    const parsed = parseRaw();
    if (parsed) { setDraft(parsed); setRawError(null); setTab('form'); }
  }
  async function save() {
    let value = draft;
    if (tab === 'json') {
      const parsed = parseRaw();
      if (!parsed) return;
      value = parsed;
      setDraft(parsed);
      setRawError(null);
    }
    try { await put.mutateAsync(value); toast({ tone: 'success', title: 'Settings saved', description: `${ns} · ${scope}` }); } catch { /* inline */ }
  }
  async function doReset() {
    try { await reset.mutateAsync(); toast({ tone: 'success', title: 'Override removed', description: 'Falling back to the inherited value.' }); } catch { /* inline */ }
  }
  const apiError = put.error ?? reset.error;
  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center gap-2 text-body-sm text-ink-subtle">
        {overridden ? <Badge tone="primary">Overridden at this scope{version != null ? ` · v${version}` : ''}</Badge> : <Badge>Inherited</Badge>}
        <span>Saving writes the full document to this scope.</span>
      </div>
      <Tabs value={tab} onValueChange={switchTab}>
        <TabsList aria-label="Editor mode">
          <TabsTrigger value="form">Form</TabsTrigger>
          {SHOW_RAW && <TabsTrigger value="json">Raw JSON</TabsTrigger>}
        </TabsList>
        <TabsContent value="form">
          <div className="flex max-w-md flex-col gap-4">
            {FIELDS[ns].map((f) =>
              f.kind === 'bool' ? (
                <Switch key={f.key} label={f.label} checked={Boolean(draft[f.key])} disabled={!editable} onCheckedChange={(v) => setDraft({ ...draft, [f.key]: v })} />
              ) : (
                <Field key={f.key} label={f.label} hint={f.hint}>
                  <Input type="number" step={f.step ?? 1} value={String(draft[f.key] ?? '')} disabled={!editable} onChange={(e) => setDraft({ ...draft, [f.key]: e.target.value === '' ? '' : Number(e.target.value) })} />
                </Field>
              ),
            )}
          </div>
        </TabsContent>
        <TabsContent value="json">
          <Field label="Settings JSON" error={rawError ?? undefined}>
            <Textarea className="min-h-72 font-mono text-body-sm" spellCheck={false} value={raw} readOnly={!editable} onChange={(e) => { setRaw(e.target.value); setRawError(null); }} />
          </Field>
        </TabsContent>
      </Tabs>
      {apiError && (
        <Alert tone="danger" title={apiError instanceof ApiError ? `Rejected by the API (${apiError.code})` : 'Save failed'}>{errMsg(apiError)}</Alert>
      )}
      {editable && (
        <div className="flex gap-2">
          <Button variant="primary" onClick={() => void save()} loading={put.isPending}>Save settings</Button>
          <Button onClick={() => void doReset()} loading={reset.isPending} disabled={!overridden}>Reset to inherited</Button>
        </div>
      )}
    </div>
  );
}

function RuntimeTab({ ws }: { ws: Workspace }) {
  const { userId } = useAuth();
  const [ns, setNs] = useState<Namespace>('suggestion');
  const [scope, setScope] = useState<ScopeKind>('workspace');
  const rooms = useRooms(ws.id);
  const [roomId, setRoomId] = useState('');
  const roomItems = useMemo(() => rooms.data?.items ?? [], [rooms.data]);
  const selectedRoom = roomItems.find((r) => r.id === roomId) ?? null;
  useEffect(() => { if (scope === 'room' && !roomId && roomItems[0]) setRoomId(roomItems[0].id); }, [scope, roomId, roomItems]);

  const scopeId = scope === 'workspace' ? ws.id : roomId;
  const editable = canAdmin(ws) || (scope === 'room' && selectedRoom?.hostId === userId);
  const eff = useEffectiveSettings(ns, scope, scopeId || undefined, ws.id);
  const stored = useScopedSettings(ns, scope, scopeId || undefined, editable);

  const loading = eff.isLoading || (editable && stored.isLoading);
  const initial = tidy((stored.data?.value ?? eff.data?.value ?? {}) as Json);
  const overridden = stored.data?.value != null;

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-end gap-3">
        <Field label="Namespace" className="w-44">
          <Select value={ns} onValueChange={(v) => setNs(v as Namespace)} options={[{ value: 'suggestion', label: 'suggestion' }, { value: 'pipeline', label: 'pipeline' }]} />
        </Field>
        <Field label="Scope" className="w-44">
          <Select value={scope} onValueChange={(v) => setScope(v as ScopeKind)} options={[{ value: 'workspace', label: 'Workspace' }, { value: 'room', label: 'Room' }]} />
        </Field>
        {scope === 'room' && (
          <Field label="Room" className="w-64">
            <Select value={roomId} onValueChange={setRoomId} placeholder="No rooms" options={roomItems.map((r) => ({ value: r.id, label: r.title }))} />
          </Field>
        )}
      </div>
      {!editable && <ReadOnlyNote role={ws.myRole} />}
      {!scopeId ? (
        <EmptyState title="No room selected" description="Create a room first to configure room-scoped settings." />
      ) : loading ? (
        <Skeleton className="h-48" />
      ) : eff.isError && !stored.data ? (
        <Alert tone="danger">{errMsg(eff.error)}</Alert>
      ) : (
        <Card>
          <CardBody>
            <RuntimeEditor
              key={`${ns}-${scope}-${scopeId}-${stored.data?.version ?? 'none'}-${eff.dataUpdatedAt}`}
              ns={ns} scope={scope} scopeId={scopeId} initial={initial} editable={editable} overridden={overridden} version={stored.data?.version}
            />
          </CardBody>
        </Card>
      )}
    </div>
  );
}

function AccountTab() {
  const profile = useProfile();
  const { email, logout } = useAuth();
  const { theme, setTheme } = useTheme();
  return (
    <div className="flex flex-col gap-6">
      <section aria-label="Profile">
        <SectionLabel>Profile</SectionLabel>
        <SettingsGroup>
          <div className="flex items-center gap-3 px-5 py-4">
            <Avatar name={profile.data?.fullName ?? email ?? 'User'} size="lg" />
            <div><p className="text-body font-semibold">{profile.data?.fullName ?? '—'}</p><p className="text-body-sm text-ink-muted">{email}</p></div>
          </div>
        </SettingsGroup>
      </section>
      <section aria-label="Appearance">
        <SectionLabel>Appearance</SectionLabel>
        <SettingsGroup>
          <FieldRow label="Theme"><Select value={theme} onValueChange={(v) => setTheme(v as 'dark' | 'light')} options={[{ value: 'dark', label: 'Dark' }, { value: 'light', label: 'Light' }]} /></FieldRow>
        </SettingsGroup>
      </section>
      <VoiceSection />
      <div><Button onClick={() => void logout()}>Sign out</Button></div>
    </div>
  );
}

function SystemCard({ to }: { to: string }) {
  return (
    <Card className="mt-6">
      <CardHeader title="Global glossary" description="Platform-wide terms applied to every workspace. System administrators only." actions={<Link to={to} className="text-body-sm">Manage global glossary</Link>} />
    </Card>
  );
}

export function SettingsPage() {
  const profile = useProfile();
  const { current, loading, wsPath } = useWorkspace();
  const { section } = useParams();
  const [params] = useSearchParams();
  if (!section) {
    if (loading) return <Skeleton className="h-40" />; // wsPath() is only meaningful once the workspace is resolved
    // Old `?tab=` links.
    const t = params.get('tab');
    if (t === 'members') return <Navigate to={wsPath('/members')} replace />;
    const id = t === 'runtime' ? 'advanced' : t === 'account' ? 'account' : 'general';
    return <Navigate to={wsPath(`/settings/${id}`)} replace />;
  }
  const admin = canAdmin(current);
  const sysAdmin = !!profile.data?.systemAdmin;
  const visible = visibleSettingsSections(admin, sysAdmin);
  const active = visible.find((s) => s.id === section);
  return (
    <div className="mx-auto w-full max-w-[640px] px-4 pb-12 pt-8">
      <PageHeader size="settings" title="Settings" description={current ? `Manage ${current.name} and your account.` : 'Manage your account.'} flush />
      {/* Desktop: the sections live in the sidebar (it swaps to the settings nav). Mobile: a pill strip. */}
      <nav aria-label="Settings sections" className="mb-5 md:hidden">
        <ul className="flex gap-2 overflow-x-auto pb-1 [scrollbar-width:none]">
          {visible.map((s) => (
            <li key={s.id} className="shrink-0">
              <Link
                to={wsPath(`/settings/${s.id}`)}
                aria-current={s.id === section ? 'page' : undefined}
                className={cn('flex h-8 items-center gap-2 whitespace-nowrap rounded-pill border px-3.5 text-body font-medium no-underline hover:no-underline focus-ring', s.id === section ? 'border-transparent bg-surface-2 text-ink' : 'border-hairline/60 text-ink-muted hover:text-ink')}
              >
                <s.icon size={16} weight="duotone" aria-hidden />{s.label}
              </Link>
            </li>
          ))}
        </ul>
      </nav>
      <div className="min-w-0">
        {loading ? <Skeleton className="h-40" /> : !active ? (
          <EmptyState title="Not available" description="This section needs the workspace owner or admin role." action={<Link to={wsPath('/settings/general')}>Back to settings</Link>} />
        ) : active.id === 'account' ? <AccountTab />
          : !current ? <EmptyState title="No workspace" description="Create a workspace to configure it." />
          : active.id === 'general' ? <WorkspaceTab ws={current} />
          : active.id === 'ai' ? <AiSettings ws={current} />
          : active.id === 'global-glossary' ? <GlobalGlossaryAdmin />
          : <><RuntimeTab ws={current} />{sysAdmin && <SystemCard to={wsPath('/settings/global-glossary')} />}</>}
      </div>
    </div>
  );
}
