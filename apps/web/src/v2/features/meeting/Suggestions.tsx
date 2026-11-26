import { useState } from 'react';
import { BellSlash, Check, Copy, ThumbsDown, ThumbsUp, X, Warning, CheckFat } from '@phosphor-icons/react';
import { Badge, Button, Textarea, useToast } from '@v2/ui';
import type { FeedbackAction, FeedbackState, FollowUp, Suggestion } from '@v2/lib/suggestions';
import { CATEGORY_LABELS, FEEDBACK } from '@v2/lib/strings';

export interface SuggestionsView {
  items: Suggestion[];
  followUps: FollowUp[];
  onDismiss(id: string): void;
  onSnooze(id: string): void;
  /** Rating buttons are shown when present; dismissing also sends `dismissed`. */
  feedback?: FeedbackState;
}

async function copyText(text: string): Promise<boolean> {
  try { await navigator.clipboard.writeText(text); return true; } catch { return false; }
}

function tone(s?: string, category?: string): 'danger' | 'warning' | 'neutral' | 'primary' {
  if (s === 'high' || s === 'critical') return 'danger';
  if (s === 'medium' || s === 'warning') return 'warning';
  return category === 'answer' ? 'primary' : 'neutral';
}

const RATINGS: { action: FeedbackAction; label: string; icon: typeof Check }[] = [
  { action: 'used', label: FEEDBACK.used, icon: CheckFat },
  { action: 'helpful', label: FEEDBACK.helpful, icon: ThumbsUp },
  { action: 'not_helpful', label: FEEDBACK.notHelpful, icon: ThumbsDown },
  { action: 'wrong', label: FEEDBACK.wrong, icon: Warning },
];
const WITH_COMMENT: FeedbackAction[] = ['not_helpful', 'wrong'];

/** Rating row of one suggestion. Not helpful / Wrong send at once and then offer an optional comment. */
function FeedbackRow({ s, fb }: { s: Suggestion; fb: FeedbackState }) {
  const marks = fb.marks[s.suggestionId] ?? [];
  const [commenting, setCommenting] = useState<FeedbackAction | null>(null);
  const [comment, setComment] = useState('');
  const [thanks, setThanks] = useState(false);
  const failed = fb.failed[s.suggestionId];
  function pick(a: FeedbackAction) {
    void fb.send(s.suggestionId, a);
    setThanks(false);
    if (WITH_COMMENT.includes(a)) { setCommenting(a); setComment(''); } else setCommenting(null);
  }
  async function sendComment() {
    if (commenting && comment.trim()) {
      const ok = await fb.send(s.suggestionId, commenting, comment);
      if (ok) setThanks(true);
    }
    setCommenting(null);
  }
  return (
    <div className="mt-2 flex flex-col gap-1.5 border-t border-hairline pt-2" data-testid="suggestion-feedback">
      <div role="group" aria-label={FEEDBACK.group} className="flex flex-wrap items-center gap-1.5">
        {RATINGS.map(({ action, label, icon: Icon }) => {
          const on = marks.includes(action);
          return (
            <Button
              key={action}
              size="sm"
              variant={on ? 'accent' : 'ghost'}
              aria-pressed={on}
              data-action={action}
              disabled={on}
              onClick={() => pick(action)}
              aria-label={`${label}: ${s.content}`}
            >
              <Icon size={14} weight={on ? 'fill' : 'regular'} aria-hidden />{label}
            </Button>
          );
        })}
      </div>
      {commenting && (
        <div className="flex flex-col gap-1.5" data-testid="feedback-comment">
          <label className="text-caption text-ink-subtle" htmlFor={`fb-${s.suggestionId}`}>{FEEDBACK.commentLabel}</label>
          <Textarea id={`fb-${s.suggestionId}`} rows={2} maxLength={1000} className="min-h-14" placeholder={FEEDBACK.commentPlaceholder} value={comment} onChange={(e) => setComment(e.target.value)} />
          <div className="flex gap-1.5">
            <Button size="sm" variant="primary" disabled={!comment.trim()} onClick={() => void sendComment()}>{FEEDBACK.send}</Button>
            <Button size="sm" variant="ghost" onClick={() => setCommenting(null)}>{FEEDBACK.skip}</Button>
          </div>
        </div>
      )}
      {thanks && !commenting && <p role="status" className="text-caption text-ink-subtle">{FEEDBACK.thanks}</p>}
      {failed && <p role="alert" className="text-caption text-danger-text">{FEEDBACK.failed}</p>}
    </div>
  );
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
                  <Badge tone={tone(s.severity, s.category)}>{CATEGORY_LABELS[s.category] ?? s.category.replace(/_/g, ' ')}</Badge>
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
                  <Button size="sm" variant="ghost" onClick={() => { void view.feedback?.send(s.suggestionId, 'dismissed'); view.onDismiss(s.suggestionId); }} aria-label={`Dismiss suggestion: ${s.content}`}><X className="size-4" aria-hidden />Dismiss</Button>
                </div>
                {view.feedback && <FeedbackRow s={s} fb={view.feedback} />}
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
