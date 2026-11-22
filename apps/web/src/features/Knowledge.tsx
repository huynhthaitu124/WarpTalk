import { useState, type FormEvent } from 'react';
import { useSearchParams } from 'react-router-dom';
import { Brain, FileText, MagnifyingGlass, Microphone } from '@phosphor-icons/react';
import { Alert, Badge, Button, EmptyState, GroupHeader, Input, PageBody, PageHeader, Select, Skeleton } from '@/ui';
import { useWorkspace } from '@/lib/workspace';
import { useKnowledgeSearch } from '@/lib/queries';
import type { Citation, KnowledgeHit } from '@/lib/schemas';
import { errMsg } from './errors';

const SOURCES = [
  { value: 'all', label: 'All sources' },
  { value: 'document', label: 'Documents' },
  { value: 'transcript', label: 'Transcripts' },
  { value: 'meeting_summary', label: 'Meeting summaries' },
  { value: 'glossary_term', label: 'Glossary terms' },
];

function clock(ms: number) {
  const s = Math.floor(ms / 1000);
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const mm = String(m).padStart(2, '0');
  const ss = String(s % 60).padStart(2, '0');
  return h > 0 ? `${h}:${mm}:${ss}` : `${m}:${ss}`;
}

/** "Quarterly report, page 4" or "Board review, 12:30": what the answer cites. */
export function citationLabel(c: Citation): string {
  if (c.page != null) return `${c.title}, page ${c.page}`;
  if (c.startMs != null) return `${c.title}, ${clock(c.startMs)}`;
  return c.title;
}

function Hit({ hit }: { hit: KnowledgeHit }) {
  const c = hit.citation;
  const doc = c.kind === 'document' || c.kind === 'knowledge';
  return (
    <li className="flex flex-col gap-2 px-4 py-3" data-testid="knowledge-hit">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="text-ink-subtle" aria-hidden>{doc ? <FileText size={16} weight="duotone" /> : <Microphone size={16} weight="duotone" />}</span>
        <span className="min-w-0 max-w-full truncate text-body font-medium text-ink" title={c.title}>{c.title}</span>
        <Badge data-testid="citation">{c.page != null ? `Page ${c.page}` : c.startMs != null ? `Meeting · ${clock(c.startMs)}` : c.kind}</Badge>
      </div>
      {c.headingPath && c.headingPath.length > 0 && <p className="truncate text-caption text-ink-subtle">{c.headingPath.join(' › ')}</p>}
      <p className="line-clamp-4 break-words text-body text-ink-muted">{hit.text}</p>
      <p className="text-caption text-ink-subtle">Source: {citationLabel(c)}</p>
    </li>
  );
}

export function KnowledgePage() {
  const { current, loading } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const q = params.get('q') ?? '';
  const source = params.get('source') ?? 'all';
  const [draft, setDraft] = useState(q);
  const res = useKnowledgeSearch(current?.id, q, source === 'all' ? undefined : source);

  const patch = (next: Record<string, string | null>) => {
    const p = new URLSearchParams(params);
    for (const [k, v] of Object.entries(next)) { if (v === null || v === '') p.delete(k); else p.set(k, v); }
    setParams(p, { replace: true });
  };
  function submit(e: FormEvent) { e.preventDefault(); patch({ q: draft.trim() || null }); }

  if (loading) return <PageBody><Skeleton className="h-40" /></PageBody>;
  if (!current) return (<><PageHeader title="Knowledge" /><PageBody><EmptyState icon={<Brain size={20} weight="duotone" />} title="No workspace" description="Create or join a workspace to search its knowledge." /></PageBody></>);
  const hits = res.data?.chunks ?? [];
  return (
    <>
      <PageHeader title="Knowledge" description={`Search documents and past meetings in ${current.name}. Answers cite their source.`} />
      <form onSubmit={submit} role="search" aria-label="Knowledge search" className="flex flex-wrap items-center gap-2 border-b border-row px-4 py-2">
        <div className="relative min-w-0 flex-1 basis-64">
          <MagnifyingGlass size={14} aria-hidden className="pointer-events-none absolute left-2.5 top-1/2 -translate-y-1/2 text-ink-subtle" />
          <Input aria-label="Search knowledge" placeholder="Ask about a decision, a term, a document…" className="h-8 rounded-pill pl-8 text-body-sm" value={draft} onChange={(e) => setDraft(e.target.value)} />
        </div>
        <Select aria-label="Source type" className="h-8 w-48 rounded-pill max-sm:w-full" value={source} onValueChange={(v) => patch({ source: v === 'all' ? null : v })} options={SOURCES} />
        <Button type="submit" variant="primary" disabled={!draft.trim()}>Search</Button>
      </form>
      {!q.trim() ? (
        <PageBody><EmptyState icon={<Brain size={20} weight="duotone" />} title="Search your workspace knowledge" description="Find passages from uploaded documents and meeting transcripts. Each result shows where it came from." /></PageBody>
      ) : res.isLoading ? (
        <div className="flex flex-col" aria-hidden><Skeleton className="h-20 rounded-none" /><Skeleton className="h-20 rounded-none" /></div>
      ) : res.isError ? (
        <PageBody><Alert tone="danger" title="Search failed">{errMsg(res.error)}</Alert></PageBody>
      ) : hits.length === 0 ? (
        <PageBody><EmptyState icon={<MagnifyingGlass size={20} weight="duotone" />} title={`No results for “${q.trim()}”`} description="Nothing indexed matches yet. Documents and meeting summaries become searchable after they are processed." action={<Button onClick={() => { setDraft(''); patch({ q: null, source: null }); }}>Clear search</Button>} /></PageBody>
      ) : (
        <>
          <GroupHeader><p role="status">{hits.length} result{hits.length === 1 ? '' : 's'} for “{q.trim()}”</p></GroupHeader>
          <ul className="divide-y divide-[color:var(--row-line)] border-b border-row" aria-label="Search results">{hits.map((h) => <Hit key={h.chunkId} hit={h} />)}</ul>
        </>
      )}
    </>
  );
}
