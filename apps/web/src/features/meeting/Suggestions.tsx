import { BellSlash, Check, Copy, X } from '@phosphor-icons/react';
import { Badge, Button, useToast } from '@/ui';
import type { FollowUp, Suggestion } from '@/lib/suggestions';

export interface SuggestionsView {
  items: Suggestion[];
  followUps: FollowUp[];
  onDismiss(id: string): void;
  onSnooze(id: string): void;
}

async function copyText(text: string): Promise<boolean> {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}

function tone(s?: string): 'danger' | 'warning' | 'neutral' {
  return s === 'high' || s === 'critical' ? 'danger' : s === 'medium' || s === 'warning' ? 'warning' : 'neutral';
}

export function SuggestionsTab({ view }: { view: SuggestionsView }) {
  const toast = useToast();
  const copy = async (t: string) => toast(await copyText(t) ? { tone: 'success', title: 'Copied' } : { tone: 'warning', title: 'Could not copy', description: 'Clipboard access was denied.' });
  return (
    <div className="flex flex-col gap-4 p-3" data-testid="suggestions-tab">
      <section aria-label="Live suggestions">
        {view.items.length === 0 ? (
          <p className="px-1 py-4 text-body-sm text-ink-subtle" data-testid="suggestions-empty">Suggestions will appear here while people talk.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {view.items.map((s) => (
              <li key={s.suggestionId} data-testid="suggestion" data-category={s.category} className="rounded-lg border border-hairline bg-surface-2 p-3">
                <div className="mb-1 flex flex-wrap items-center gap-1.5">
                  <Badge tone={tone(s.severity)}>{s.category.replace(/_/g, ' ')}</Badge>
                  {s.severity && <span className="text-caption text-ink-subtle">{s.severity}</span>}
                </div>
                <p className="text-body font-medium text-ink" data-testid="suggestion-content">{s.content}</p>
                {s.detail && <p className="mt-1 text-body-sm text-ink-subtle">{s.detail}</p>}
                {s.followUp && <p className="mt-1 text-body-sm text-ink-subtle">Follow-up: {s.followUp}</p>}
                {s.sources && s.sources.length > 0 && (
                  <p className="mt-1 text-caption text-ink-subtle">Sources: {s.sources.map((x) => x.title ?? x.url ?? 'source').join(', ')}</p>
                )}
                <div className="mt-2 flex flex-wrap gap-1.5">
                  {(s.actions ?? []).map((a) => <Button key={a.label} size="sm" onClick={() => void copy(s.content)}>{a.label}</Button>)}
                  <Button size="sm" variant="ghost" onClick={() => void copy(s.content)} aria-label={`Copy suggestion: ${s.content}`}><Copy className="size-4" aria-hidden />Copy</Button>
                  <Button size="sm" variant="ghost" onClick={() => view.onSnooze(s.suggestionId)} aria-label={`Snooze suggestion: ${s.content}`}><BellSlash className="size-4" aria-hidden />Snooze</Button>
                  <Button size="sm" variant="ghost" onClick={() => view.onDismiss(s.suggestionId)} aria-label={`Dismiss suggestion: ${s.content}`}><X className="size-4" aria-hidden />Dismiss</Button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section aria-label="Follow-ups">
        <h3 className="mb-1 px-1 text-body-sm font-semibold text-ink">Follow-ups</h3>
        {view.followUps.length === 0 ? (
          <p className="px-1 text-body-sm text-ink-subtle" data-testid="followups-empty">No follow-ups yet.</p>
        ) : (
          <ul className="flex flex-col gap-1.5" data-testid="followups">
            {view.followUps.map((f) => (
              <li key={f.id} data-testid="followup" className="flex items-start gap-2 rounded-md px-1 py-1.5">
                <Check className="mt-0.5 size-4 shrink-0 text-ink-subtle" aria-hidden />
                <span className="min-w-0 flex-1 text-body-sm">{f.text}{(f.owner || f.due) && <span className="text-ink-subtle"> ({[f.owner, f.due].filter(Boolean).join(', ')})</span>}</span>
                <Button size="sm" variant="ghost" onClick={() => void copy(f.text)} aria-label={`Copy follow-up: ${f.text}`}><Copy className="size-4" aria-hidden /></Button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
