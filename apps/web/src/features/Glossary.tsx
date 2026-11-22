import { useRef, useState, type FormEvent } from 'react';
import { Link, useNavigate, useParams } from 'react-router-dom';
import { BookBookmark, DownloadSimple, PencilSimple, Plus, Trash, UploadSimple } from '@phosphor-icons/react';
import {
  Alert, Badge, Button, Checkbox, Dialog, DialogContent, EmptyState, Field, GroupHeader, Input, PageBody, PageHeader, Select, Skeleton,
  Table, TBody, TD, TH, THead, TR, Textarea, useToast,
} from '@/ui';
import { LANGUAGES, languageLabel } from '@/lib/languages';
import { canAdmin, useWorkspace } from '@/lib/workspace';
import {
  exportGlossaryCsv, useAddTerm, useCreateGlossary, useDeleteGlossary, useDeleteTerm, useGlossaries, useGlossary, useImportTerms, useTerms,
  useUpdateGlossary, useUpdateTerm, type GlossaryInput, type TermInput,
} from '@/lib/glossary';
import { useQuery } from '@tanstack/react-query';
import { z } from 'zod';
import { api } from '@/lib/api';
import { globalTerm, type Glossary, type GlossaryTerm } from '@/lib/schemas';
import { formatWhen } from './common';
import { errMsg } from './errors';

const NAME_MAX = 150;
const DESC_MAX = 2000;
const TERM_MAX = 200;
const NOTES_MAX = 1000;

function ReadOnlyNote() {
  return <Alert tone="info" className="mb-4" title="Read-only">Only workspace owners and admins can change glossaries. You can read and export them.</Alert>;
}

function langOptions(...extra: string[]) {
  const known = new Set<string>(LANGUAGES.map((l) => l.code));
  return [...LANGUAGES.map((l) => ({ value: l.code, label: l.label })), ...extra.filter((c) => c && !known.has(c)).map((c) => ({ value: c, label: languageLabel(c) }))];
}

function Pair({ g }: { g: Pick<Glossary, 'sourceLanguage' | 'targetLanguage'> }) {
  return <span className="whitespace-nowrap">{languageLabel(g.sourceLanguage)} → {languageLabel(g.targetLanguage)}</span>;
}

/** Create or edit a glossary (name, description, language pair). */
function GlossaryDialog({ open, onOpenChange, initial, onSave, pending, error, title, cta }: {
  open: boolean; onOpenChange(v: boolean): void; initial?: Glossary; onSave(v: GlossaryInput): Promise<void>; pending: boolean; error?: unknown; title: string; cta: string;
}) {
  const [name, setName] = useState(initial?.name ?? '');
  const [description, setDescription] = useState(initial?.description ?? '');
  const [source, setSource] = useState(initial?.sourceLanguage ?? 'vi');
  const [target, setTarget] = useState(initial?.targetLanguage ?? 'en');
  const [tried, setTried] = useState(false);
  const nameError = tried && !name.trim() ? 'Enter a name.' : undefined;
  const langError = source === target ? 'Source and target language must differ.' : undefined;
  async function submit(e: FormEvent) {
    e.preventDefault();
    setTried(true);
    if (!name.trim() || langError) return;
    try { await onSave({ name: name.trim(), description: description.trim(), sourceLanguage: source, targetLanguage: target }); } catch { /* shown inline */ }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        title={title}
        description="A glossary pins how specific terms are translated for one language pair."
        footer={<><Button onClick={() => onOpenChange(false)}>Cancel</Button><Button variant="primary" type="submit" form="glossary-form" loading={pending}>{cta}</Button></>}
      >
        <form id="glossary-form" onSubmit={submit} noValidate className="flex flex-col gap-4">
          <Field label="Name" required error={nameError}><Input autoFocus maxLength={NAME_MAX} value={name} onChange={(e) => setName(e.target.value)} /></Field>
          <Field label="Description" hint="Optional."><Textarea maxLength={DESC_MAX} value={description} onChange={(e) => setDescription(e.target.value)} /></Field>
          <div className="grid grid-cols-2 gap-3 max-sm:grid-cols-1">
            <Field label="Source language"><Select value={source} onValueChange={setSource} options={langOptions(source)} /></Field>
            <Field label="Target language" error={langError}><Select value={target} onValueChange={setTarget} options={langOptions(target)} /></Field>
          </div>
          {error != null && <Alert tone="danger">{errMsg(error)}</Alert>}
        </form>
      </DialogContent>
    </Dialog>
  );
}

function ConfirmDelete({ open, onOpenChange, title, body, cta, onConfirm, pending, error }: {
  open: boolean; onOpenChange(v: boolean): void; title: string; body: string; cta: string; onConfirm(): void; pending: boolean; error?: unknown;
}) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title={title} description={body} footer={<><Button onClick={() => onOpenChange(false)}>Cancel</Button><Button variant="danger" loading={pending} onClick={onConfirm}>{cta}</Button></>}>
        {error != null && <Alert tone="danger">{errMsg(error)}</Alert>}
      </DialogContent>
    </Dialog>
  );
}

/** Published global terms (managed by system administrators), shown read-only to everybody. */
function GlobalTermsCard() {
  const q = useQuery({
    queryKey: ['global-published'],
    retry: false,
    queryFn: () => api('/glossaries/global', { schema: z.array(globalTerm) }),
  });
  const items = q.data ?? [];
  if (q.isLoading || q.isError || items.length === 0) return null;
  return (
    <section aria-label="Global terms" className="mt-6">
      <GroupHeader><h2 className="font-medium text-ink">{`Global terms (${items.length})`}</h2><span className="truncate max-sm:hidden">· Maintained by WarpTalk and applied to every workspace. Read-only.</span></GroupHeader>
      <Table bare aria-label="Global glossary terms">
        <THead><tr><TH>Term</TH><TH>Preferred translation</TH><TH className="max-sm:hidden">Languages</TH></tr></THead>
        <TBody>
          {items.map((t) => (
            <TR key={t.id}>
              <TD className="max-w-56 truncate" title={t.term}>{t.term}</TD>
              <TD className="max-w-56 truncate" title={t.preferredTranslation}>{t.preferredTranslation}</TD>
              <TD className="whitespace-nowrap text-ink-subtle max-sm:hidden">{t.sourceLanguage && t.targetLanguage ? <Pair g={{ sourceLanguage: t.sourceLanguage, targetLanguage: t.targetLanguage }} /> : 'Any'}</TD>
            </TR>
          ))}
        </TBody>
      </Table>
    </section>
  );
}

// ------------------------------------------------------------------ list page

export function GlossaryListPage() {
  const { current, loading, wsPath } = useWorkspace();
  const nav = useNavigate();
  const toast = useToast();
  const list = useGlossaries(current?.id);
  const create = useCreateGlossary(current?.id ?? '');
  const [creating, setCreating] = useState(false);
  if (loading) return <PageBody><Skeleton className="h-40" /></PageBody>;
  if (!current) return <PageBody><EmptyState title="No workspace" description="Create or join a workspace to manage its glossaries." /></PageBody>;
  const writer = canAdmin(current);
  const items = list.data ?? [];
  return (
    <>
      <PageHeader
        title="Glossary"
        description="Preferred translations for names and terms. Translation and captions use them in this workspace."
        actions={writer ? <Button variant="primary" onClick={() => { create.reset(); setCreating(true); }}><Plus size={16} weight="bold" aria-hidden />New glossary</Button> : undefined}
      />
      {!writer && <PageBody className="pb-0"><ReadOnlyNote /></PageBody>}
      {list.isLoading ? <PageBody><Skeleton className="h-40" /></PageBody> : list.isError ? <PageBody><Alert tone="danger" title="Could not load glossaries">{errMsg(list.error)}</Alert></PageBody> : items.length === 0 ? (
        <PageBody><EmptyState
          icon={<BookBookmark size={20} weight="duotone" />}
          title="No glossaries yet"
          description={writer ? 'Create a glossary for a language pair, then add terms one by one or import a CSV.' : 'Owners and admins have not created any glossary for this workspace yet.'}
          action={writer ? <Button variant="primary" onClick={() => { create.reset(); setCreating(true); }}>New glossary</Button> : undefined}
        /></PageBody>
      ) : (
        <div>
          <div className="max-sm:hidden">
            <Table bare aria-label="Glossaries">
              <THead><tr><TH>Name</TH><TH>Languages</TH><TH>Terms</TH><TH>Updated</TH></tr></THead>
              <TBody>
                {items.map((g) => (
                  <TR key={g.id} data-testid="glossary-row">
                    <TD className="max-w-80"><Link to={wsPath(`/glossary/${g.id}`)} className="block truncate font-medium text-ink hover:text-link" title={g.name}>{g.name}</Link>{g.description && <span className="block truncate text-caption text-ink-subtle">{g.description}</span>}</TD>
                    <TD className="text-ink-muted"><Pair g={g} /></TD>
                    <TD className="text-ink-muted">{g.termCount}</TD>
                    <TD className="whitespace-nowrap text-ink-subtle" data-testid="glossary-updated">{formatWhen(g.updatedAt ?? g.createdAt)}</TD>
                  </TR>
                ))}
              </TBody>
            </Table>
          </div>
          <ul className="sm:hidden" aria-label="Glossaries (list)">
            {items.map((g) => (
              <li key={g.id} className="flex flex-col gap-1 border-b border-row px-4 py-3">
                <Link to={wsPath(`/glossary/${g.id}`)} className="truncate text-body font-medium text-ink hover:text-link">{g.name}</Link>
                <span className="text-caption text-ink-subtle"><Pair g={g} /> · {g.termCount} terms</span>
              </li>
            ))}
          </ul>
        </div>
      )}
      <GlobalTermsCard />
      {writer && creating && (
        <GlossaryDialog
          open
          onOpenChange={setCreating}
          title="New glossary"
          cta="Create glossary"
          pending={create.isPending}
          error={create.error}
          onSave={async (v) => {
            const g = await create.mutateAsync(v);
            toast({ tone: 'success', title: 'Glossary created', description: g.name });
            setCreating(false);
            nav(wsPath(`/glossary/${g.id}`));
          }}
        />
      )}
    </>
  );
}

// ------------------------------------------------------------------ detail page

/** The inline add/edit form: one grid row that works in the table and on phones. */
function TermForm({ initial, onSave, onCancel, pending, error, submitLabel, label }: {
  initial?: GlossaryTerm; onSave(v: TermInput): Promise<void>; onCancel?: () => void; pending: boolean; error?: unknown; submitLabel: string; label: string;
}) {
  const [source, setSource] = useState(initial?.sourceTerm ?? '');
  const [target, setTarget] = useState(initial?.targetTerm ?? '');
  const [cs, setCs] = useState(initial?.caseSensitive ?? false);
  const [notes, setNotes] = useState(initial?.notes ?? '');
  const [tried, setTried] = useState(false);
  const bad = !source.trim() || !target.trim();
  async function submit(e: FormEvent) {
    e.preventDefault();
    setTried(true);
    if (bad) return;
    try {
      await onSave({ sourceTerm: source.trim(), targetTerm: target.trim(), caseSensitive: cs, notes: notes.trim() || null });
      if (!initial) { setSource(''); setTarget(''); setCs(false); setNotes(''); setTried(false); }
    } catch { /* shown inline */ }
  }
  return (
    <form onSubmit={submit} noValidate aria-label={label} className="flex flex-col gap-2 px-4 py-3" data-testid={initial ? 'term-edit-form' : 'term-add-form'}>
      <div className="grid grid-cols-[1fr_1fr_1fr_auto] items-center gap-2 max-lg:grid-cols-2 max-sm:grid-cols-1">
        <Input aria-label="Source term" placeholder="Source term" maxLength={TERM_MAX} value={source} aria-invalid={tried && !source.trim() ? true : undefined} onChange={(e) => setSource(e.target.value)} />
        <Input aria-label="Target term" placeholder="Target term" maxLength={TERM_MAX} value={target} aria-invalid={tried && !target.trim() ? true : undefined} onChange={(e) => setTarget(e.target.value)} />
        <Input aria-label="Notes" placeholder="Notes (optional)" maxLength={NOTES_MAX} value={notes} onChange={(e) => setNotes(e.target.value)} />
        <div className="flex items-center gap-3 max-lg:col-span-2 max-sm:col-span-1">
          <Checkbox checked={cs} onCheckedChange={setCs} label="Case sensitive" />
          <Button type="submit" variant="primary" size="sm" loading={pending}>{submitLabel}</Button>
          {onCancel && <Button type="button" size="sm" onClick={onCancel}>Cancel</Button>}
        </div>
      </div>
      {tried && bad && <p role="alert" className="text-caption text-danger-text">Both the source term and the target term are required.</p>}
      {error != null && <p role="alert" className="text-caption text-danger-text">{errMsg(error)}</p>}
    </form>
  );
}

function TermsCard({ g, writer }: { g: Glossary; writer: boolean }) {
  const terms = useTerms(g.id);
  const add = useAddTerm(g.id);
  const update = useUpdateTerm(g.id);
  const del = useDeleteTerm(g.id);
  const toast = useToast();
  const [editing, setEditing] = useState<string | null>(null);
  const [removing, setRemoving] = useState<GlossaryTerm | null>(null);
  const items = terms.data ?? [];
  const [q, setQ] = useState('');
  const shown = q.trim() ? items.filter((t) => `${t.sourceTerm} ${t.targetTerm} ${t.notes ?? ''}`.toLowerCase().includes(q.trim().toLowerCase())) : items;
  return (
    <section aria-label="Terms" className="mt-4">
      <GroupHeader className="pr-2">
        <h2 className="font-medium text-ink">{`Terms (${items.length})`}</h2><span aria-hidden>·</span><Pair g={g} />
        {items.length > 5 && <Input aria-label="Filter terms" placeholder="Filter terms" className="ml-auto h-6 w-48 rounded-pill text-body-sm" value={q} onChange={(e) => setQ(e.target.value)} />}
      </GroupHeader>
      {writer && (
        <div className="border-b border-row">
          <TermForm
            label="Add term"
            submitLabel="Add term"
            pending={add.isPending}
            error={add.error}
            onSave={async (v) => { await add.mutateAsync(v); toast({ tone: 'success', title: 'Term added', description: `${v.sourceTerm} → ${v.targetTerm}` }); }}
          />
        </div>
      )}
      {terms.isLoading ? <PageBody><Skeleton className="h-24" /></PageBody> : terms.isError ? <PageBody><Alert tone="danger">{errMsg(terms.error)}</Alert></PageBody> : shown.length === 0 ? (
        <PageBody><p className="text-body text-ink-muted">{items.length === 0 ? (writer ? 'No terms yet. Add the first one above or import a CSV.' : 'This glossary has no terms yet.') : 'No terms match the filter.'}</p></PageBody>
      ) : (
        <>
          <div className="max-sm:hidden">
            <Table bare aria-label="Glossary terms">
              <THead><tr><TH>{languageLabel(g.sourceLanguage)}</TH><TH>{languageLabel(g.targetLanguage)}</TH><TH>Case</TH><TH>Notes</TH>{writer && <TH className="w-24"><span className="sr-only">Actions</span></TH>}</tr></THead>
              <TBody>
                {shown.map((t) => editing === t.id ? (
                  <TR key={t.id}><TD colSpan={writer ? 5 : 4} className="h-auto p-0">
                    <TermForm initial={t} label={`Edit term ${t.sourceTerm}`} submitLabel="Save" pending={update.isPending} error={update.error} onCancel={() => { setEditing(null); update.reset(); }}
                      onSave={async (v) => { await update.mutateAsync({ id: t.id, ...v }); setEditing(null); toast({ tone: 'success', title: 'Term saved' }); }} />
                  </TD></TR>
                ) : (
                  <TR key={t.id} data-testid="term-row">
                    <TD className="max-w-64 truncate" title={t.sourceTerm}>{t.sourceTerm}</TD>
                    <TD className="max-w-64 truncate" title={t.targetTerm}>{t.targetTerm}</TD>
                    <TD>{t.caseSensitive ? <Badge>Case sensitive</Badge> : <span className="text-ink-subtle">Any</span>}</TD>
                    <TD className="max-w-64 truncate text-ink-muted" title={t.notes ?? undefined}>{t.notes || '—'}</TD>
                    {writer && (
                      <TD className="whitespace-nowrap text-right">
                        <Button variant="ghost" size="icon" aria-label={`Edit term ${t.sourceTerm}`} onClick={() => { update.reset(); setEditing(t.id); }}><PencilSimple size={16} aria-hidden /></Button>
                        <Button variant="ghost" size="icon" aria-label={`Delete term ${t.sourceTerm}`} onClick={() => { del.reset(); setRemoving(t); }}><Trash size={16} aria-hidden /></Button>
                      </TD>
                    )}
                  </TR>
                ))}
              </TBody>
            </Table>
          </div>
          <ul className="sm:hidden" aria-label="Glossary terms (list)">
            {shown.map((t) => editing === t.id ? (
              <li key={t.id}><TermForm initial={t} label={`Edit term ${t.sourceTerm}`} submitLabel="Save" pending={update.isPending} error={update.error} onCancel={() => { setEditing(null); update.reset(); }}
                onSave={async (v) => { await update.mutateAsync({ id: t.id, ...v }); setEditing(null); toast({ tone: 'success', title: 'Term saved' }); }} /></li>
            ) : (
              <li key={t.id} className="flex items-start gap-2 border-b border-row px-4 py-3">
                <div className="min-w-0 flex-1">
                  <p className="break-words text-body font-medium">{t.sourceTerm} <span className="text-ink-subtle">→</span> {t.targetTerm}</p>
                  {(t.notes || t.caseSensitive) && <p className="break-words text-caption text-ink-subtle">{t.caseSensitive ? 'Case sensitive. ' : ''}{t.notes}</p>}
                </div>
                {writer && (
                  <>
                    <Button variant="ghost" size="icon" aria-label={`Edit term ${t.sourceTerm}`} onClick={() => { update.reset(); setEditing(t.id); }}><PencilSimple size={16} aria-hidden /></Button>
                    <Button variant="ghost" size="icon" aria-label={`Delete term ${t.sourceTerm}`} onClick={() => { del.reset(); setRemoving(t); }}><Trash size={16} aria-hidden /></Button>
                  </>
                )}
              </li>
            ))}
          </ul>
        </>
      )}
      <ConfirmDelete
        open={!!removing}
        onOpenChange={(v) => { if (!v) setRemoving(null); }}
        title={`Delete term "${removing?.sourceTerm ?? ''}"?`}
        body="Translations stop using this term. This cannot be undone."
        cta="Delete term"
        pending={del.isPending}
        error={del.error}
        onConfirm={() => { if (removing) void del.mutateAsync(removing.id).then(() => { setRemoving(null); toast({ tone: 'success', title: 'Term deleted' }); }).catch(() => undefined); }}
      />
    </section>
  );
}

export function GlossaryDetailPage() {
  const { glossaryId = '' } = useParams();
  const { current, loading, wsPath } = useWorkspace();
  const nav = useNavigate();
  const toast = useToast();
  const g = useGlossary(glossaryId);
  const update = useUpdateGlossary(glossaryId, current?.id ?? '');
  const del = useDeleteGlossary(current?.id ?? '');
  const importer = useImportTerms(glossaryId);
  const file = useRef<HTMLInputElement>(null);
  const [editing, setEditing] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [result, setResult] = useState<{ imported: number; skipped: number } | null>(null);
  const [importError, setImportError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  if (loading || g.isLoading) return <PageBody><Skeleton className="h-40" /></PageBody>;
  if (g.isError || !g.data) return <PageBody><Alert tone="danger" title="Glossary not found">{errMsg(g.error)} <Link to={wsPath('/glossary')}>Back to glossaries</Link></Alert></PageBody>;
  const glossary = g.data;
  const writer = canAdmin(current);

  async function onFile(f: File | undefined) {
    if (!f) return;
    setResult(null); setImportError(null);
    try { setResult(await importer.mutateAsync(await f.text())); } catch (e) { setImportError(errMsg(e)); }
    if (file.current) file.current.value = '';
  }
  async function download() {
    setExporting(true);
    try {
      const csv = await exportGlossaryCsv(glossary.id);
      const url = URL.createObjectURL(new Blob([csv], { type: 'text/csv;charset=utf-8' }));
      const a = document.createElement('a');
      a.href = url;
      a.download = `${glossary.name.replace(/[^\w.-]+/g, '-') || 'glossary'}.csv`;
      document.body.append(a);
      a.click();
      a.remove();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (e) { toast({ tone: 'danger', title: 'Export failed', description: errMsg(e) }); } finally { setExporting(false); }
  }
  return (
    <>
      <PageHeader
        title={glossary.name}
        description={<span className="flex flex-wrap items-center gap-2"><Pair g={glossary} />{glossary.description && <span>· {glossary.description}</span>}</span>}
        actions={(
          <>
            <Link to={wsPath('/glossary')} className="text-body">All glossaries</Link>
            <Button onClick={() => void download()} loading={exporting}><DownloadSimple size={16} aria-hidden />Export CSV</Button>
            {writer && (
              <>
                <Button onClick={() => file.current?.click()} loading={importer.isPending}><UploadSimple size={16} aria-hidden />Import CSV</Button>
                <input ref={file} type="file" accept=".csv,text/csv,text/plain" className="sr-only" tabIndex={-1} aria-label="CSV file to import" onChange={(e) => void onFile(e.target.files?.[0])} />
                <Button onClick={() => { update.reset(); setEditing(true); }}><PencilSimple size={16} aria-hidden />Edit</Button>
                <Button variant="danger-soft" onClick={() => { del.reset(); setDeleting(true); }}><Trash size={16} aria-hidden />Delete</Button>
              </>
            )}
          </>
        )}
      />
      <PageBody className="flex flex-col gap-3 py-3 empty:hidden">
      {!writer && <ReadOnlyNote />}
      {result && (
        <Alert tone={result.imported > 0 ? 'success' : 'warning'} title="Import finished">
          <span data-testid="import-result">Imported {result.imported} {result.imported === 1 ? 'term' : 'terms'}, skipped {result.skipped}.</span>{result.skipped > 0 && ' Skipped rows were duplicates or already in the glossary.'}
        </Alert>
      )}
      {importError && <Alert tone="danger" title="Import failed">{importError}</Alert>}
      <p className="text-caption text-ink-subtle">CSV format: one <code>source,target</code> pair per line, optional header row, up to 2000 rows.</p>
      </PageBody>
      <TermsCard g={glossary} writer={writer} />
      {writer && editing && (
        <GlossaryDialog
          open
          onOpenChange={setEditing}
          initial={glossary}
          title="Edit glossary"
          cta="Save changes"
          pending={update.isPending}
          error={update.error}
          onSave={async (v) => { await update.mutateAsync(v); toast({ tone: 'success', title: 'Glossary saved' }); setEditing(false); }}
        />
      )}
      <ConfirmDelete
        open={deleting}
        onOpenChange={setDeleting}
        title={`Delete "${glossary.name}"?`}
        body={`The glossary and its ${glossary.termCount} terms are removed and translations stop using them.`}
        cta="Delete glossary"
        pending={del.isPending}
        error={del.error}
        onConfirm={() => void del.mutateAsync(glossary.id).then(() => { toast({ tone: 'success', title: 'Glossary deleted' }); nav(wsPath('/glossary')); }).catch(() => undefined)}
      />
    </>
  );
}
