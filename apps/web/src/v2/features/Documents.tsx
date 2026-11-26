import { useEffect, useRef, useState, type DragEvent, type FormEvent, type ReactNode } from 'react';
import { DownloadSimple, FileText, MagnifyingGlass, UploadSimple, X } from '@phosphor-icons/react';
import {
  Alert, Badge, Button, Dialog, DialogContent, Drawer, EmptyState, Field, Input, PageBody, PageHeader, Select, Skeleton, Switch, Table, TBody, TD, TH, THead, Tooltip, TR, useToast,
} from '@v2/ui';
import { cn } from '@v2/lib/cn';
import { canAdmin, useWorkspace } from '@v2/lib/workspace';
import {
  DOCUMENT_PAGE_SIZE, downloadDocument, useArchiveDocument, useDocuments, usePatchDocument, useUploadDocument,
} from '@v2/lib/queries';
import { DOC_ACCESS_LEVELS, DOC_CATEGORIES, type WorkspaceDocument, type Workspace } from '@v2/lib/schemas';
import { formatWhen } from './common';
import { errCode, errMsg } from './errors';

// Client-side checks mirror crates/modules/workspace/src/domain/documents.rs (types, 20 MB).
export const DOC_EXTENSIONS = ['pdf', 'docx', 'txt', 'md', 'markdown', 'csv'] as const;
export const DOC_MAX_MB = 20;
const DOC_MAX_BYTES = DOC_MAX_MB * 1024 * 1024;

export function checkDocumentFile(file: File): string | null {
  const ext = file.name.includes('.') ? file.name.split('.').pop()!.toLowerCase() : '';
  if (!(DOC_EXTENSIONS as readonly string[]).includes(ext)) return 'Unsupported file type: allowed types are pdf, docx, txt, md and csv.';
  if (file.size === 0) return 'The file is empty.';
  if (file.size > DOC_MAX_BYTES) return `File exceeds the ${DOC_MAX_MB} MB limit.`;
  return null;
}

export function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

const CATEGORY_LABEL: Record<string, string> = { GENERAL: 'General', TECHNICAL: 'Technical', FINANCIAL: 'Financial', LEGAL: 'Legal', MEETING_NOTES: 'Meeting notes' };
const ACCESS_LABEL: Record<string, string> = { PUBLIC: 'Public', MEMBERS_ONLY: 'Members', ADMIN_ONLY: 'Admins only' };
const ACCESS_HINT: Record<string, string> = {
  PUBLIC: 'Visible to everyone in the workspace.',
  MEMBERS_ONLY: 'Visible to every workspace member.',
  ADMIN_ONLY: 'Visible to owners and admins only. Members do not see it at all.',
};
const categoryOptions = DOC_CATEGORIES.map((c) => ({ value: c, label: CATEGORY_LABEL[c]! }));
const accessOptions = DOC_ACCESS_LEVELS.map((a) => ({ value: a, label: ACCESS_LABEL[a]! }));

type Tone = 'info' | 'success' | 'neutral' | 'danger';
const ING: Record<string, { label: string; tone: Tone }> = {
  pending: { label: 'Pending', tone: 'info' },
  completed: { label: 'Completed', tone: 'success' },
  skipped: { label: 'Skipped', tone: 'neutral' },
  failed: { label: 'Failed', tone: 'danger' },
};

/** Plain-language explanation of an ingestion status and its machine reason. */
export function ingestionExplanation(doc: Pick<WorkspaceDocument, 'ingestionStatus' | 'ingestionFailureReason' | 'fileExtension'>): string {
  const r = doc.ingestionFailureReason ?? '';
  if (doc.ingestionStatus === 'pending') return 'Waiting to be indexed for the AI assistant.';
  if (doc.ingestionStatus === 'completed') return 'Indexed: the AI assistant can search this document.';
  if (r === 'no_indexable_content') {
    return ['pdf', 'docx'].includes(doc.fileExtension)
      ? 'No text was indexed. Text extraction for PDF and DOCX files is not available yet, so the AI assistant cannot search this file. You can still download it.'
      : 'No text could be extracted from this file, so the AI assistant cannot search it.';
  }
  if (r === 'ai_retrieval_not_allowed') return 'AI retrieval is switched off for this document.';
  return r ? `${doc.ingestionStatus === 'failed' ? 'Indexing failed' : 'Skipped'}: ${r.replace(/_/g, ' ')}.` : doc.ingestionStatus === 'failed' ? 'Indexing failed.' : 'Not indexed.';
}

function IngestionPill({ doc }: { doc: WorkspaceDocument }) {
  const m = ING[doc.ingestionStatus] ?? { label: doc.ingestionStatus, tone: 'neutral' as Tone };
  const why = ingestionExplanation(doc);
  return (
    <Tooltip content={why}>
      <span tabIndex={0} className="inline-flex rounded-pill focus-ring" data-testid="ingestion-pill" data-status={doc.ingestionStatus}>
        <Badge tone={m.tone}>
          <span aria-hidden className="size-1.5 rounded-pill bg-current" />
          {m.label}
        </Badge>
        <span className="sr-only">. {why}</span>
      </span>
    </Tooltip>
  );
}

// ---- upload
function UploadDialog({ ws, open, onOpenChange }: { ws: Workspace; open: boolean; onOpenChange(v: boolean): void }) {
  const upload = useUploadDocument(ws.id);
  const toast = useToast();
  const inputRef = useRef<HTMLInputElement>(null);
  const [file, setFile] = useState<File | null>(null);
  const [fileError, setFileError] = useState<string | null>(null);
  const [title, setTitle] = useState('');
  const [category, setCategory] = useState('GENERAL');
  const [access, setAccess] = useState('MEMBERS_ONLY');
  const [ai, setAi] = useState(true);
  const [progress, setProgress] = useState<number | null>(null);
  const [over, setOver] = useState(false);

  const reset = () => { setFile(null); setFileError(null); setTitle(''); setCategory('GENERAL'); setAccess('MEMBERS_ONLY'); setAi(true); setProgress(null); upload.reset(); };
  function pick(f: File | undefined) {
    upload.reset();
    if (!f) return;
    const err = checkDocumentFile(f);
    setFileError(err);
    setFile(err ? null : f);
  }
  function onDrop(e: DragEvent) {
    e.preventDefault();
    setOver(false);
    pick(e.dataTransfer.files[0]);
  }
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!file) { setFileError('Choose a file to upload.'); return; }
    setProgress(0);
    try {
      const d = await upload.mutateAsync({ file, title, category, accessLevel: access, aiRetrievalAllowed: ai, onProgress: setProgress });
      toast({ tone: 'success', title: 'Document uploaded', description: d.title });
      reset();
      onOpenChange(false);
    } catch { setProgress(null); }
  }
  return (
    <Dialog open={open} onOpenChange={(v) => { if (!v) reset(); onOpenChange(v); }}>
      <DialogContent
        title="Upload document"
        description={`PDF, DOCX, TXT, MD or CSV, up to ${DOC_MAX_MB} MB.`}
        footer={<><Button onClick={() => onOpenChange(false)}>Cancel</Button><Button variant="primary" type="submit" form="doc-upload-form" loading={upload.isPending}>Upload</Button></>}
      >
        <form id="doc-upload-form" onSubmit={submit} className="flex flex-col gap-4" noValidate>
          <div
            data-testid="doc-dropzone"
            data-over={over || undefined}
            onDragOver={(e) => { e.preventDefault(); setOver(true); }}
            onDragLeave={() => setOver(false)}
            onDrop={onDrop}
            className={cn('flex flex-col items-center gap-2 rounded-lg border border-dashed border-hairline-strong px-4 py-6 text-center transition-colors', over && 'border-primary bg-surface-3')}
          >
            <UploadSimple size={20} weight="duotone" className="text-ink-subtle" aria-hidden />
            {file ? (
              <p className="flex max-w-full items-center gap-2 text-body">
                <FileText size={16} aria-hidden className="shrink-0 text-ink-subtle" />
                <span className="truncate" title={file.name} data-testid="doc-selected-file">{file.name}</span>
                <span className="shrink-0 text-ink-subtle">{formatSize(file.size)}</span>
                <Button size="icon" variant="ghost" aria-label="Remove selected file" onClick={() => { setFile(null); setFileError(null); if (inputRef.current) inputRef.current.value = ''; }}><X size={14} aria-hidden /></Button>
              </p>
            ) : <p className="text-body text-ink-muted">Drag a file here, or</p>}
            <Button onClick={() => inputRef.current?.click()}>{file ? 'Choose another file' : 'Choose file'}</Button>
            <input
              ref={inputRef}
              type="file"
              hidden
              data-testid="doc-file-input"
              accept={DOC_EXTENSIONS.map((e) => `.${e}`).join(',')}
              onChange={(e) => { pick(e.target.files?.[0]); e.target.value = ''; }}
            />
          </div>
          {fileError && <Alert tone="danger">{fileError}</Alert>}
          <Field label="Title" hint="Optional. Defaults to the file name."><Input value={title} maxLength={255} onChange={(e) => setTitle(e.target.value)} /></Field>
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label="Category"><Select value={category} onValueChange={setCategory} options={categoryOptions} /></Field>
            <Field label="Access level" hint={ACCESS_HINT[access]}><Select value={access} onValueChange={setAccess} options={accessOptions} /></Field>
          </div>
          <Switch label="Allow AI retrieval" description="Let the AI assistant search this document and cite it in answers." checked={ai} onCheckedChange={setAi} />
          {progress !== null && (
            <div className="flex items-center gap-3">
              <div role="progressbar" aria-label="Upload progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(progress * 100)} className="h-2 flex-1 overflow-hidden rounded-pill bg-surface-3">
                <div className="h-full rounded-pill bg-primary transition-[width] duration-75" style={{ width: `${Math.round(progress * 100)}%` }} />
              </div>
              <span className="w-10 text-right text-caption text-ink-subtle">{Math.round(progress * 100)}%</span>
            </div>
          )}
          {upload.isError && <Alert tone="danger"><span data-code={errCode(upload.error)}>{errMsg(upload.error)}</span></Alert>}
        </form>
      </DialogContent>
    </Dialog>
  );
}

// ---- detail drawer
function Meta({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">{label}</dt>
      <dd className="min-w-0 break-words text-body text-ink">{children}</dd>
    </div>
  );
}

function EditForm({ ws, doc, onSaved }: { ws: Workspace; doc: WorkspaceDocument; onSaved(d: WorkspaceDocument): void }) {
  const patch = usePatchDocument(ws.id);
  const toast = useToast();
  const [title, setTitle] = useState(doc.title);
  const [category, setCategory] = useState(doc.category);
  const [access, setAccess] = useState(doc.accessLevel);
  const [ai, setAi] = useState(doc.aiRetrievalAllowed);
  const dirty = title.trim() !== doc.title || category !== doc.category || access !== doc.accessLevel || ai !== doc.aiRetrievalAllowed;
  async function submit(e: FormEvent) {
    e.preventDefault();
    if (!title.trim()) return;
    const body: Parameters<typeof patch.mutateAsync>[0] = { id: doc.id };
    if (title.trim() !== doc.title) body.title = title.trim();
    if (category !== doc.category) body.category = category;
    if (access !== doc.accessLevel) body.accessLevel = access;
    if (ai !== doc.aiRetrievalAllowed) body.aiRetrievalAllowed = ai;
    try {
      const d = await patch.mutateAsync(body);
      toast({ tone: 'success', title: 'Document updated', description: d.title });
      onSaved(d);
    } catch { /* inline */ }
  }
  return (
    <form onSubmit={submit} noValidate className="flex flex-col gap-4" aria-label="Edit document">
      <Field label="Title" error={!title.trim() ? 'A title is required.' : undefined}><Input value={title} maxLength={255} onChange={(e) => setTitle(e.target.value)} /></Field>
      <Field label="Category"><Select value={category} onValueChange={setCategory} options={categoryOptions} /></Field>
      <Field label="Access level" hint={ACCESS_HINT[access]}><Select value={access} onValueChange={setAccess} options={accessOptions} /></Field>
      <Switch label="Allow AI retrieval" description="Turning this off removes the document from AI answers." checked={ai} onCheckedChange={setAi} />
      {patch.isError && <Alert tone="danger">{errMsg(patch.error)}</Alert>}
      <div><Button type="submit" variant="primary" loading={patch.isPending} disabled={!dirty || !title.trim()}>Save changes</Button></div>
    </form>
  );
}

function DocDrawer({ ws, doc, onClose, onChanged }: { ws: Workspace; doc: WorkspaceDocument | null; onClose(): void; onChanged(d: WorkspaceDocument): void }) {
  const toast = useToast();
  const archive = useArchiveDocument(ws.id);
  const [confirm, setConfirm] = useState(false);
  const [dl, setDl] = useState(false);
  const manage = canAdmin(ws);
  if (!doc) return null;
  async function download() {
    if (!doc) return;
    setDl(true);
    try { await downloadDocument(ws.id, doc); } catch (e) { toast({ tone: 'danger', title: 'Download failed', description: errMsg(e) }); } finally { setDl(false); }
  }
  async function doArchive() {
    if (!doc) return;
    try {
      await archive.mutateAsync(doc.id);
      toast({ tone: 'success', title: 'Document archived', description: doc.title });
      setConfirm(false);
      onClose();
    } catch { /* inline */ }
  }
  return (
    <>
      <Drawer open onOpenChange={(v) => { if (!v) onClose(); }} title={doc.title} description={doc.fileName}>
        <div className="flex flex-col gap-5" data-testid="doc-drawer">
          <div className="flex flex-wrap items-center gap-2">
            <IngestionPill doc={doc} />
            <Button size="sm" loading={dl} onClick={() => void download()}><DownloadSimple size={14} aria-hidden />Download</Button>
          </div>
          <p className="text-body-sm text-ink-subtle" data-testid="doc-ingestion-note">{ingestionExplanation(doc)}</p>
          <dl className="grid gap-3 sm:grid-cols-2">
            <Meta label="Category">{CATEGORY_LABEL[doc.category] ?? doc.category}</Meta>
            <Meta label="Access">{ACCESS_LABEL[doc.accessLevel] ?? doc.accessLevel}</Meta>
            <Meta label="Size">{formatSize(doc.sizeBytes)}</Meta>
            <Meta label="Type">{doc.fileExtension.toUpperCase()}</Meta>
            <Meta label="AI retrieval">{doc.aiRetrievalAllowed ? 'Allowed' : 'Off'}</Meta>
            <Meta label="Uploaded">{formatWhen(doc.createdAt)}</Meta>
            {doc.lastIndexedAt && <Meta label="Last indexed">{formatWhen(doc.lastIndexedAt)}</Meta>}
          </dl>
          {manage ? (
            <>
              <hr className="border-hairline" />
              <EditForm key={`${doc.id}-${doc.updatedAt ?? doc.createdAt}`} ws={ws} doc={doc} onSaved={onChanged} />
              <hr className="border-hairline" />
              <div><Button variant="danger-soft" onClick={() => setConfirm(true)}>Archive document</Button></div>
            </>
          ) : <Alert tone="info">You can read and download this document. Only owners and admins can change or archive it.</Alert>}
        </div>
      </Drawer>
      <Dialog open={confirm} onOpenChange={(v) => { if (!v) { setConfirm(false); archive.reset(); } }}>
        <DialogContent
          title={`Archive “${doc.title}”?`}
          description="It disappears from this list and the AI assistant stops using it. The file is kept in storage."
          footer={<><Button onClick={() => { setConfirm(false); archive.reset(); }}>Cancel</Button><Button variant="danger" loading={archive.isPending} onClick={() => void doArchive()}>Archive document</Button></>}
        >
          {archive.isError && <Alert tone="danger">{errMsg(archive.error)}</Alert>}
        </DialogContent>
      </Dialog>
    </>
  );
}

// ---- list
export function DocumentsPage() {
  const { current, loading } = useWorkspace();
  const [search, setSearch] = useState('');
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState('all');
  const [pageNo, setPageNo] = useState(1);
  const [uploading, setUploading] = useState(false);
  const [selected, setSelected] = useState<WorkspaceDocument | null>(null);

  useEffect(() => {
    const t = setTimeout(() => { setQuery(search); setPageNo(1); }, 300);
    return () => clearTimeout(t);
  }, [search]);

  const q = useDocuments(current?.id, { page: pageNo, search: query, category });
  if (loading) return <PageBody><Skeleton className="h-40" /></PageBody>;
  if (!current) return (<><PageHeader title="Documents" /><PageBody><EmptyState icon={<FileText size={20} weight="duotone" />} title="No workspace" description="Create or join a workspace to see its documents." /></PageBody></>);
  const admin = canAdmin(current);
  const items = q.data?.items ?? [];
  const total = q.data?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / DOCUMENT_PAGE_SIZE));
  const filtered = query.trim() !== '' || category !== 'all';
  const live = selected ? items.find((d) => d.id === selected.id) ?? selected : null;

  return (
    <>
      <PageHeader
        title="Documents"
        description={`Files shared in ${current.name}. Documents with AI retrieval on can be searched in Knowledge.`}
        actions={admin ? <Button variant="primary" onClick={() => setUploading(true)}><UploadSimple size={16} aria-hidden />Upload document</Button> : undefined}
      />
      <PageBody className="flex flex-col gap-3 border-b border-row py-3 empty:hidden">
        <Alert tone="info" title="PDF and DOCX text extraction is not available yet">
          Only TXT, MD and CSV files are indexed for the AI assistant today. PDF and DOCX files are stored and can be downloaded, but they finish as “Skipped” (no indexable content) until extraction ships.
        </Alert>
        {!admin && <Alert tone="info">You can read and download documents. Only workspace owners and admins can upload or change them, and admin-only documents are hidden from you.</Alert>}
      </PageBody>
      <div role="search" aria-label="Filter documents" className="flex flex-wrap items-center gap-2 border-b border-row px-4 py-2">
        <div className="relative min-w-0 flex-1 basis-64">
          <MagnifyingGlass size={14} aria-hidden className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-subtle" />
          <Input aria-label="Search documents" placeholder="Search by title or file name" className="h-8 rounded-pill pl-8 text-body-sm" value={search} onChange={(e) => setSearch(e.target.value)} />
        </div>
        <Select aria-label="Category filter" className="h-8 w-48 rounded-pill max-sm:w-full" value={category} onValueChange={(v) => { setCategory(v); setPageNo(1); }} options={[{ value: 'all', label: 'All categories' }, ...categoryOptions]} />
      </div>
      {q.isLoading ? <div className="flex flex-col" aria-hidden><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /></div>
        : q.isError ? (
          errCode(q.error) === 'STORAGE_NOT_CONFIGURED'
            ? <PageBody><Alert tone="warning" title="Document storage is not configured">This server has no object store, so documents cannot be uploaded or listed. Ask an administrator to configure storage.</Alert></PageBody>
            : <PageBody><Alert tone="danger" title="Could not load documents">{errMsg(q.error)}</Alert></PageBody>
        ) : items.length === 0 ? (
          <PageBody><EmptyState
            icon={<FileText size={20} weight="duotone" />}
            title={filtered ? 'No matching documents' : 'No documents yet'}
            description={filtered ? 'Try another search or category.' : admin ? 'Upload handbooks, glossaries or meeting notes so the AI assistant can cite them.' : 'Workspace admins can upload documents here.'}
            action={filtered ? <Button onClick={() => { setSearch(''); setQuery(''); setCategory('all'); }}>Clear filters</Button> : admin ? <Button variant="primary" onClick={() => setUploading(true)}><UploadSimple size={16} aria-hidden />Upload document</Button> : undefined}
          /></PageBody>
        ) : (
          <div>
            <div className="max-sm:hidden">
              <Table bare aria-label="Documents">
                <THead><tr><TH>Title</TH><TH>Category</TH><TH>Access</TH><TH>Size</TH><TH>Status</TH><TH>Uploaded</TH></tr></THead>
                <TBody>
                  {items.map((d) => (
                    <TR key={d.id} data-testid="document-row" data-title={d.title}>
                      <TD className="max-w-72">
                        <button type="button" onClick={() => setSelected(d)} title={d.title} aria-label={`Open ${d.title}`} className="block max-w-full truncate rounded-xs text-left font-medium text-ink hover:text-link focus-ring">{d.title}</button>
                        <span className="block truncate text-caption text-ink-subtle" title={d.fileName}>{d.fileName}</span>
                      </TD>
                      <TD className="text-ink-muted">{CATEGORY_LABEL[d.category] ?? d.category}</TD>
                      <TD className="whitespace-nowrap text-ink-muted">{ACCESS_LABEL[d.accessLevel] ?? d.accessLevel}</TD>
                      <TD className="whitespace-nowrap text-ink-subtle">{formatSize(d.sizeBytes)}</TD>
                      <TD><IngestionPill doc={d} /></TD>
                      <TD className="whitespace-nowrap text-ink-subtle">{formatWhen(d.createdAt)}</TD>
                    </TR>
                  ))}
                </TBody>
              </Table>
            </div>
            <ul className="sm:hidden" aria-label="Documents (list)">
              {items.map((d) => (
                <li key={d.id} className="flex flex-col gap-2 border-b border-row px-4 py-3" data-testid="document-row" data-title={d.title}>
                  <button type="button" onClick={() => setSelected(d)} title={d.title} aria-label={`Open ${d.title}`} className="block max-w-full truncate rounded-xs text-left text-body font-medium text-ink hover:text-link focus-ring">{d.title}</button>
                  <p className="truncate text-caption text-ink-subtle">{d.fileName} · {formatSize(d.sizeBytes)} · {CATEGORY_LABEL[d.category] ?? d.category}</p>
                  <div className="flex flex-wrap items-center gap-2"><IngestionPill doc={d} /><Badge>{ACCESS_LABEL[d.accessLevel] ?? d.accessLevel}</Badge></div>
                </li>
              ))}
            </ul>
            <div className="flex items-center justify-between gap-3 px-4 py-3">
              <p className="text-body-sm text-ink-subtle" role="status">{total} document{total === 1 ? '' : 's'}</p>
              {pages > 1 && (
                <nav aria-label="Pagination" className="flex items-center gap-2">
                  <Button size="sm" disabled={pageNo <= 1} onClick={() => setPageNo(pageNo - 1)}>Previous</Button>
                  <span className="text-body-sm text-ink-subtle">Page {pageNo} of {pages}</span>
                  <Button size="sm" disabled={pageNo >= pages} onClick={() => setPageNo(pageNo + 1)}>Next</Button>
                </nav>
              )}
            </div>
          </div>
        )}
      {admin && <UploadDialog ws={current} open={uploading} onOpenChange={setUploading} />}
      <DocDrawer ws={current} doc={live} onClose={() => setSelected(null)} onChanged={setSelected} />
    </>
  );
}
