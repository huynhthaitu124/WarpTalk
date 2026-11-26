// Workspace suggestion quality (owner/admin): per-category shown/used/dismissed/helpful/wrong counts and rates for a date range.
import { useMemo, useState } from 'react';
import { ChartBar } from '@phosphor-icons/react';
import { Alert, EmptyState, Field, Input, PageBody, PageHeader, Skeleton, Table, TBody, TD, TH, THead, TR } from '@v2/ui';
import { canAdmin, useWorkspace } from '@v2/lib/workspace';
import { useSuggestionQuality, type CategoryQuality } from '@v2/lib/queries';
import { CATEGORY_LABELS, QUALITY } from '@v2/lib/strings';
import { errCode, errMsg } from './errors';

const day = (d: Date) => d.toISOString().slice(0, 10);
const pct = (r: number) => `${(r * 100).toFixed(r > 0 && r < 0.1 ? 1 : 0)}%`;
const label = (c: string) => CATEGORY_LABELS[c] ?? c.replace(/_/g, ' ').replace(/^./, (x) => x.toUpperCase());

function Row({ c, total }: { c: CategoryQuality; total?: boolean }) {
  return (
    <TR data-testid={total ? 'quality-total' : 'quality-row'} data-category={c.category} className={total ? 'bg-surface-2/60 font-semibold' : undefined}>
      <TD className={total ? 'font-semibold' : 'font-medium'}>{total ? QUALITY.total : label(c.category)}</TD>
      <TD className="tabular-nums" data-col="shown">{c.shown}</TD>
      <TD className="tabular-nums" data-col="used">{c.used}</TD>
      <TD className="tabular-nums" data-col="dismissed">{c.dismissed}</TD>
      <TD className="tabular-nums" data-col="helpful">{c.helpful}</TD>
      <TD className="tabular-nums" data-col="notHelpful">{c.notHelpful}</TD>
      <TD className="tabular-nums" data-col="wrong">{c.wrong}</TD>
      <TD className="tabular-nums" data-col="usedRate">{pct(c.usedRate)}</TD>
      <TD className="tabular-nums" data-col="wrongRate">{pct(c.wrongRate)}</TD>
    </TR>
  );
}

export function QualityPage() {
  const { current, loading } = useWorkspace();
  const admin = canAdmin(current);
  const [{ from: dFrom, to: dTo }] = useState(() => { const now = new Date(); return { to: day(now), from: day(new Date(now.getTime() - 29 * 86_400_000)) }; });
  const [from, setFrom] = useState(dFrom);
  const [to, setTo] = useState(dTo);
  const bad = !!from && !!to && from > to;
  // The end date is inclusive: send the end of that day.
  const q = useSuggestionQuality(current?.id, from, to ? `${to}T23:59:59Z` : '', admin && !bad);
  const rows = useMemo(() => q.data?.categories ?? [], [q.data]);
  if (loading) return <PageBody><Skeleton className="h-40" /></PageBody>;
  if (!current || !admin) {
    return (<><PageHeader title={QUALITY.title} /><PageBody><EmptyState icon={<ChartBar size={20} weight="duotone" />} title="Not available" description={QUALITY.restricted} /></PageBody></>);
  }
  return (
    <>
      <PageHeader title={QUALITY.title} description={QUALITY.description} />
      <div role="group" aria-label="Date range" className="flex flex-wrap items-end gap-3 border-b border-row px-4 py-3">
        <Field label={QUALITY.from}><Input type="date" value={from} max={to || undefined} onChange={(e) => setFrom(e.target.value)} className="w-40" /></Field>
        <Field label={QUALITY.to}><Input type="date" value={to} min={from || undefined} onChange={(e) => setTo(e.target.value)} className="w-40" /></Field>
      </div>
      {bad ? <PageBody><Alert tone="warning">{QUALITY.invalidRange}</Alert></PageBody>
        : q.isLoading ? <div aria-hidden><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /></div>
        : q.isError ? <PageBody><Alert tone="danger" title={QUALITY.loadError}><span data-code={errCode(q.error)}>{errMsg(q.error)}</span></Alert></PageBody>
        : q.data && q.data.total.shown === 0 && rows.length === 0 ? <PageBody><EmptyState icon={<ChartBar size={20} weight="duotone" />} title={QUALITY.empty} /></PageBody>
        : q.data && (
          <Table aria-label="Suggestion quality by category" data-testid="quality-table">
            <THead>
              <tr>
                <TH>{QUALITY.columns.category}</TH>
                <TH>{QUALITY.columns.shown}</TH><TH>{QUALITY.columns.used}</TH><TH>{QUALITY.columns.dismissed}</TH><TH>{QUALITY.columns.helpful}</TH>
                <TH>{QUALITY.columns.notHelpful}</TH><TH>{QUALITY.columns.wrong}</TH><TH>{QUALITY.columns.usedRate}</TH><TH>{QUALITY.columns.wrongRate}</TH>
              </tr>
            </THead>
            <TBody>
              {rows.map((c) => <Row key={c.category} c={c} />)}
              <Row c={q.data.total} total />
            </TBody>
          </Table>
        )}
    </>
  );
}
