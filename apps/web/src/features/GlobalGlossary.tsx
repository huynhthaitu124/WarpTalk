import { useState, type FormEvent } from 'react';
import { Archive, ClockCounterClockwise, PencilSimple, Plus, Trash, UploadSimple } from '@phosphor-icons/react';
import {
  Alert, Badge, Button, Card, CardBody, CardHeader, Dialog, DialogContent, Drawer, EmptyState, Field, Input, Select, Skeleton, StatusPill,
  Table, TBody, TD, TH, THead, TR, Textarea, useToast,
} from '@/ui';
import { LANGUAGES, languageLabel } from '@/lib/languages';
import { GLOBAL_STATUSES, type GlobalStatus, type GlobalTerm } from '@/lib/schemas';
import { useGlobalAction, useGlobalAudits, useGlobalSave, useGlobalTerms, type GlobalInput } from '@/lib/glossary';
import { formatWhen } from './common';
import { errMsg } from './errors';

const ANY = 'any';
const langOpts = [{ value: ANY, label: 'Any language' }, ...LANGUAGES.map((l) => ({ value: l.code, label: l.label }))];

function TermDialog({ term, onClose }: { term: GlobalTerm | 'new'; onClose(): void }) {
  const initial = term === 'new' ? undefined : term;
  const save = useGlobalSave(initial?.id);
  const toast = useToast();
  const [v, setV] = useState({
    term: initial?.term ?? '', preferredTranslation: initial?.preferredTranslation ?? '', sourceLanguage: initial?.sourceLanguage ?? ANY, targetLanguage: initial?.targetLanguage ?? ANY,
    businessDomain: initial?.businessDomain ?? '', definition: initial?.definition ?? '', usageNote: initial?.usageNote ?? '', priority: String(initial?.priority ?? 5),
  });
  const [tried, setTried] = useState(false);
  const set = (k: keyof typeof v) => (x: string) => setV((c) => ({ ...c, [k]: x }));
  const missing = !v.term.trim() || !v.preferredTranslation.trim();
  async function submit(e: FormEvent) {
    e.preventDefault();
    setTried(true);
    if (missing) return;
    const body: GlobalInput = {
      term: v.term.trim(), preferredTranslation: v.preferredTranslation.trim(),
      sourceLanguage: v.sourceLanguage === ANY ? null : v.sourceLanguage, targetLanguage: v.targetLanguage === ANY ? null : v.targetLanguage,
      businessDomain: v.businessDomain.trim() || null, definition: v.definition.trim() || null, usageNote: v.usageNote.trim() || null,
      priority: Number.isFinite(Number(v.priority)) ? Math.round(Number(v.priority)) : 5,
    };
    try { await save.mutateAsync(body); toast({ tone: 'success', title: initial ? 'Term saved' : 'Draft term created', description: body.term }); onClose(); } catch { /* inline */ }
  }
  return (
    <Dialog open onOpenChange={(o) => { if (!o) onClose(); }}>
      <DialogContent
        title={initial ? 'Edit global term' : 'New global term'}
        description={initial?.status === 'published' ? 'Editing a published term bumps its version and is audited.' : 'New terms start as drafts and apply to nobody until published.'}
        footer={<><Button onClick={onClose}>Cancel</Button><Button variant="primary" type="submit" form="global-term-form" loading={save.isPending}>{initial ? 'Save changes' : 'Create draft'}</Button></>}
      >
        <form id="global-term-form" onSubmit={submit} noValidate className="flex flex-col gap-4">
          <Field label="Term" required error={tried && !v.term.trim() ? 'Enter the term.' : undefined}><Input autoFocus maxLength={200} value={v.term} onChange={(e) => set('term')(e.target.value)} /></Field>
          <Field label="Preferred translation" required error={tried && !v.preferredTranslation.trim() ? 'Enter the preferred translation.' : undefined}><Input maxLength={200} value={v.preferredTranslation} onChange={(e) => set('preferredTranslation')(e.target.value)} /></Field>
          <div className="grid grid-cols-2 gap-3 max-sm:grid-cols-1">
            <Field label="Source language"><Select value={v.sourceLanguage} onValueChange={set('sourceLanguage')} options={langOpts} /></Field>
            <Field label="Target language"><Select value={v.targetLanguage} onValueChange={set('targetLanguage')} options={langOpts} /></Field>
          </div>
          <div className="grid grid-cols-2 gap-3 max-sm:grid-cols-1">
            <Field label="Business domain" hint="Optional."><Input maxLength={100} value={v.businessDomain} onChange={(e) => set('businessDomain')(e.target.value)} /></Field>
            <Field label="Priority" hint="Higher wins when terms overlap."><Input type="number" min={0} max={100} value={v.priority} onChange={(e) => set('priority')(e.target.value)} /></Field>
          </div>
          <Field label="Definition" hint="Optional."><Textarea maxLength={1000} value={v.definition} onChange={(e) => set('definition')(e.target.value)} /></Field>
          <Field label="Usage note" hint="Optional."><Textarea maxLength={1000} value={v.usageNote} onChange={(e) => set('usageNote')(e.target.value)} /></Field>
          {save.isError && <Alert tone="danger">{errMsg(save.error)}</Alert>}
        </form>
      </DialogContent>
    </Dialog>
  );
}

function AuditDrawer({ term, onClose }: { term: GlobalTerm | null; onClose(): void }) {
  const q = useGlobalAudits(term?.id);
  return (
    <Drawer open={!!term} onOpenChange={(o) => { if (!o) onClose(); }} title="Audit history" description={term ? `Every change to "${term.term}", newest first.` : undefined}>
      {q.isLoading ? <Skeleton className="h-24" /> : q.isError ? <Alert tone="danger">{errMsg(q.error)}</Alert> : (q.data ?? []).length === 0 ? (
        <p className="text-body text-ink-subtle">No audit entries yet.</p>
      ) : (
        <ol aria-label="Audit entries" className="flex flex-col gap-3">
          {[...(q.data ?? [])].sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt)).map((a) => (
            <li key={a.id} data-testid="audit-entry" className="rounded-lg border border-hairline bg-surface-1 p-3">
              <p className="flex items-center justify-between gap-2 text-body font-medium"><span className="capitalize">{a.action}</span><span className="text-caption font-normal text-ink-subtle">{formatWhen(a.createdAt)}</span></p>
              <p className="mt-0.5 break-all text-caption text-ink-subtle">By {a.actorUserId}</p>
              {a.afterJson != null && <pre className="mt-2 max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md bg-surface-2 p-2 font-mono text-caption text-ink-muted">{JSON.stringify(a.afterJson, null, 2)}</pre>}
            </li>
          ))}
        </ol>
      )}
    </Drawer>
  );
}

type Confirm = { kind: 'publish' | 'archive' | 'delete'; term: GlobalTerm };

/** System-administrator view of the global glossary: draft, publish, archive, delete and the audit trail. */
export function GlobalGlossaryAdmin() {
  const [status, setStatus] = useState<GlobalStatus | 'all'>('all');
  const q = useGlobalTerms(status, true);
  const act = useGlobalAction();
  const toast = useToast();
  const [editing, setEditing] = useState<GlobalTerm | 'new' | null>(null);
  const [audit, setAudit] = useState<GlobalTerm | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);
  const items = q.data?.items ?? [];

  async function run() {
    if (!confirm) return;
    try {
      await act.mutateAsync({ id: confirm.term.id, action: confirm.kind });
      toast({ tone: 'success', title: { publish: 'Term published', archive: 'Term archived', delete: 'Term deleted' }[confirm.kind], description: confirm.term.term });
      setConfirm(null);
    } catch { /* inline */ }
  }
  const verb = confirm ? { publish: 'Publish', archive: 'Archive', delete: 'Delete' }[confirm.kind] : '';
  return (
    <Card>
      <CardHeader
        title="Global glossary"
        description="Terms applied to every workspace once published. Only system administrators can change them."
        actions={<Button variant="primary" size="sm" onClick={() => setEditing('new')}><Plus size={14} weight="bold" aria-hidden />New term</Button>}
      />
      <CardBody className="border-b border-hairline">
        <div className="max-w-xs"><Field label="Status"><Select value={status} onValueChange={(v) => setStatus(v as GlobalStatus | 'all')} options={[{ value: 'all', label: 'All statuses' }, ...GLOBAL_STATUSES.map((s) => ({ value: s, label: s[0]!.toUpperCase() + s.slice(1) }))]} /></Field></div>
      </CardBody>
      {q.isLoading ? <CardBody><Skeleton className="h-24" /></CardBody> : q.isError ? <CardBody><Alert tone="danger">{errMsg(q.error)}</Alert></CardBody> : items.length === 0 ? (
        <CardBody><EmptyState title="No global terms" description="Create a draft term, review it, then publish it." /></CardBody>
      ) : (
        <Table bare aria-label="Global glossary terms">
          <THead><tr><TH>Term</TH><TH>Preferred translation</TH><TH className="max-md:hidden">Languages</TH><TH>Status</TH><TH className="max-md:hidden">Version</TH><TH><span className="sr-only">Actions</span></TH></tr></THead>
          <TBody>
            {items.map((t) => (
              <TR key={t.id} data-testid="global-term-row" data-status={t.status}>
                <TD className="max-w-48 truncate" title={t.term}>{t.term}{t.businessDomain && <Badge className="ml-2">{t.businessDomain}</Badge>}</TD>
                <TD className="max-w-48 truncate" title={t.preferredTranslation}>{t.preferredTranslation}</TD>
                <TD className="whitespace-nowrap text-ink-subtle max-md:hidden">{t.sourceLanguage && t.targetLanguage ? `${languageLabel(t.sourceLanguage)} → ${languageLabel(t.targetLanguage)}` : 'Any'}</TD>
                <TD><StatusPill status={t.status.toUpperCase()} /></TD>
                <TD className="text-ink-subtle max-md:hidden">v{t.version}</TD>
                <TD className="whitespace-nowrap text-right">
                  {t.status === 'draft' && <Button size="sm" variant="primary" onClick={() => { act.reset(); setConfirm({ kind: 'publish', term: t }); }} aria-label={`Publish ${t.term}`}><UploadSimple size={14} aria-hidden />Publish</Button>}
                  {t.status === 'published' && <Button size="sm" onClick={() => { act.reset(); setConfirm({ kind: 'archive', term: t }); }} aria-label={`Archive ${t.term}`}><Archive size={14} aria-hidden />Archive</Button>}
                  <Button variant="ghost" size="icon" aria-label={`Edit ${t.term}`} onClick={() => setEditing(t)}><PencilSimple size={16} aria-hidden /></Button>
                  <Button variant="ghost" size="icon" aria-label={`Audit history for ${t.term}`} onClick={() => setAudit(t)}><ClockCounterClockwise size={16} aria-hidden /></Button>
                  <Button variant="ghost" size="icon" aria-label={`Delete ${t.term}`} onClick={() => { act.reset(); setConfirm({ kind: 'delete', term: t }); }}><Trash size={16} aria-hidden /></Button>
                </TD>
              </TR>
            ))}
          </TBody>
        </Table>
      )}
      {editing && <TermDialog term={editing} onClose={() => setEditing(null)} />}
      <AuditDrawer term={audit} onClose={() => setAudit(null)} />
      <Dialog open={!!confirm} onOpenChange={(o) => { if (!o) setConfirm(null); }}>
        <DialogContent
          title={`${verb} "${confirm?.term.term ?? ''}"?`}
          description={confirm?.kind === 'publish' ? 'Published terms apply to every workspace right away (at most 200 can be published).' : confirm?.kind === 'archive' ? 'Archived terms stop applying. You can edit and publish them again later.' : 'The term is removed permanently. Its audit history is kept.'}
          footer={<><Button onClick={() => setConfirm(null)}>Cancel</Button><Button variant={confirm?.kind === 'delete' ? 'danger' : 'primary'} loading={act.isPending} onClick={() => void run()}>{verb}</Button></>}
        >
          {act.isError && <Alert tone="danger">{errMsg(act.error)}</Alert>}
        </DialogContent>
      </Dialog>
    </Card>
  );
}
