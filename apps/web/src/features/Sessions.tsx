// Copilot sessions: list, "Start session" dialog and the session detail (transcript, suggestions, follow-ups,
// summary, action items). Backend shapes: src/lib/copilot.ts. Audio capture needs the desktop app (ADR 0001).
import { useEffect, useRef, useState, type FormEvent } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router-dom';
import { ArrowsClockwise, Check, CheckCircle, DesktopTower, Link as LinkIcon, Sparkle, Stop, Play } from '@phosphor-icons/react';
import {
  Alert, Badge, Button, Capsule, DateCapsule, Dialog, DialogContent, EmptyState, Field, GroupHeader, Input, PageBody, PageHeader, Select, Skeleton, StatusPill,
  Tabs, TabsContent, TabsList, TabsTrigger, useToast,
} from '@/ui';
import { LANGUAGES, languageLabel } from '@/lib/languages';
import { apiBaseUrl } from '@/lib/env';
import { session } from '@/lib/session';
import { useNative } from '@/lib/native';
import { useWorkspace } from '@/lib/workspace';
import { useFollowUps, useSuggestionStream, useSuggestionTriage } from '@/lib/suggestions';
import {
  PLATFORMS, platformLabel, sessionIsLive, useActionItems, useCreateSession, useMyActionItems, useRegenerateSummary, useSession, useSessions, useSetActionItemDone,
  useSetMyActionItemDone, useSummary, summaryBusy, useEndSession, useTranscript, type ActionItem, type CopilotSession,
} from '@/lib/copilot';
import { RoomDot, formatShortDate, formatWhen } from './common';
import { SuggestionsTab } from './meeting/Suggestions';
import { errMsg } from './errors';

const langOptions = LANGUAGES.map((l) => ({ value: l.code, label: l.label }));
const mmss = (ms?: number) => (ms === undefined ? '' : `${Math.floor(ms / 60000)}:${String(Math.floor(ms / 1000) % 60).padStart(2, '0')}`);

/** Desktop-app notice shown wherever audio capture is mentioned and the host cannot capture (a plain browser). */
export function CaptureNotice({ className }: { className?: string }) {
  return (
    <Alert tone="info" title="Audio capture needs the WarpTalk desktop app" className={className}>
      <span className="flex items-start gap-2">
        <DesktopTower size={16} weight="duotone" aria-hidden className="mt-0.5 shrink-0" />
        <span>A browser cannot listen to your meeting app. You can create the session here and review transcripts, suggestions and minutes; open the session in the desktop app for Mac to capture the microphone and the meeting audio.</span>
      </span>
    </Alert>
  );
}

export function StartSessionDialog({ open, onOpenChange }: { open: boolean; onOpenChange(v: boolean): void }) {
  const { current } = useWorkspace();
  const create = useCreateSession();
  const native = useNative();
  const toast = useToast();
  const nav = useNavigate();
  const canCapture = typeof native?.startCopilotSession === 'function';
  const [title, setTitle] = useState('');
  const [platform, setPlatform] = useState<string>('google_meet');
  const [url, setUrl] = useState('');
  const [me, setMe] = useState('en');
  const [others, setOthers] = useState('en');
  const [tried, setTried] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setTried(true);
    if (!current || !title.trim()) return;
    try {
      const s = await create.mutateAsync({ workspaceId: current.id, title: title.trim(), platform, meetingUrl: url.trim() || undefined, languages: { me, others } });
      if (canCapture) {
        try {
          await native!.startCopilotSession!({ sessionId: s.id, accessToken: session.accessToken ?? '', apiBaseUrl: apiBaseUrl() || window.location.origin });
        } catch (err) { toast({ tone: 'danger', title: 'Could not start capture', description: errMsg(err) }); }
      }
      toast({ tone: 'success', title: canCapture ? 'Session started' : 'Session created', description: s.title });
      setTitle(''); setUrl(''); setTried(false);
      onOpenChange(false);
      nav(`/sessions/${s.id}`);
    } catch { /* inline */ }
  }
  return (
    <Dialog open={open} onOpenChange={(v) => { if (!v) setTried(false); onOpenChange(v); }}>
      <DialogContent
        title="Start copilot session"
        description="Get a live transcript, suggestions and follow-ups for any meeting, then minutes when it ends."
        footer={
          <>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button variant="primary" type="submit" form="start-session-form" loading={create.isPending}>{canCapture ? 'Start session' : 'Create session'}</Button>
          </>
        }
      >
        <form id="start-session-form" onSubmit={submit} className="flex flex-col gap-4" noValidate>
          {!canCapture && <CaptureNotice />}
          <Field label="Title" required error={tried && !title.trim() ? 'Give the session a title.' : undefined}>
            <Input maxLength={200} value={title} onChange={(e) => setTitle(e.target.value)} autoFocus aria-required="true" placeholder="Weekly product sync" />
          </Field>
          <Field label="Meeting app"><Select value={platform} onValueChange={setPlatform} options={PLATFORMS.map((p) => ({ value: p.value, label: p.label }))} /></Field>
          <Field label="Meeting link (optional)"><Input type="url" inputMode="url" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://meet.google.com/..." /></Field>
          <div className="grid grid-cols-2 gap-3 max-[420px]:grid-cols-1">
            <Field label="I speak"><Select value={me} onValueChange={setMe} options={langOptions} /></Field>
            <Field label="Others speak"><Select value={others} onValueChange={setOthers} options={langOptions} /></Field>
          </div>
          {create.isError && <Alert tone="danger">{errMsg(create.error)}</Alert>}
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function SessionRow({ s }: { s: CopilotSession }) {
  const moment = s.startedAt ?? s.createdAt;
  return (
    <div role="listitem" data-testid="session-row" className="flex min-h-11 flex-wrap items-center gap-x-3 gap-y-1.5 border-b border-row px-4 py-1.5 hover:bg-surface-2/50 sm:flex-nowrap">
      <RoomDot status={s.status} />
      <Link to={`/sessions/${s.id}`} title={s.title} className="min-w-0 flex-1 basis-40 truncate text-body font-medium text-ink hover:text-link">{s.title}</Link>
      <div className="flex flex-wrap items-center gap-1.5 sm:shrink-0 sm:flex-nowrap">
        <Badge>{platformLabel(s.platform)}</Badge>
        <StatusPill status={s.status} />
        {moment && <span title={formatWhen(moment)}><DateCapsule text={formatShortDate(moment)} /></span>}
      </div>
    </div>
  );
}

export function ActionItemRow({ item, onToggle, busy, to }: { item: ActionItem; onToggle(done: boolean): void; busy?: boolean; to?: string }) {
  return (
    <li data-testid="action-item" data-done={item.done} className="flex min-h-11 flex-wrap items-center gap-x-3 gap-y-1 border-b border-row px-4 py-1.5">
      <span className="grid size-4 shrink-0 place-items-center text-ink-subtle" aria-hidden>{item.done ? <CheckCircle size={16} weight="fill" /> : <span className="size-3 rounded-pill border border-control" />}</span>
      <span className={`min-w-0 flex-1 basis-48 break-words text-body ${item.done ? 'text-ink-subtle line-through' : 'text-ink'}`}>
        {to ? <Link to={to} className="text-inherit hover:text-link">{item.text}</Link> : item.text}
        {(item.owner || item.deadline) && <span className="text-body-sm text-ink-subtle no-underline"> ({[item.owner, item.deadline].filter(Boolean).join(', ')})</span>}
      </span>
      {item.done && <Badge tone="success" size="sm">Done</Badge>}
      <Button size="sm" variant={item.done ? 'ghost' : 'secondary'} loading={busy} aria-label={`${item.done ? 'Reopen' : 'Mark done'}: ${item.text}`} onClick={() => onToggle(!item.done)}>
        {item.done ? 'Reopen' : <><Check size={14} weight="bold" aria-hidden />Mark done</>}
      </Button>
    </li>
  );
}

function ActionItemsTab() {
  const { current, wsPath } = useWorkspace();
  const q = useMyActionItems(current?.id);
  const set = useSetMyActionItemDone(current?.id);
  const toast = useToast();
  const items = q.data ?? [];
  if (q.isLoading) return <div aria-hidden><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /></div>;
  if (q.isError) return <PageBody><Alert tone="danger" title="Could not load action items">{errMsg(q.error)}</Alert></PageBody>;
  if (items.length === 0) {
    return <PageBody><EmptyState icon={<CheckCircle size={20} weight="duotone" />} title="No open action items" description="Action items are extracted from your sessions when they end. Owners and deadlines appear only when they were said out loud." action={<Link to={wsPath('/sessions')} className="text-body">View sessions</Link>} /></PageBody>;
  }
  return (
    <ul aria-label="Action items">
      {items.map((a) => (
        <ActionItemRow
          key={a.id} item={a} to={a.roomId ? `/sessions/${a.roomId}` : undefined} busy={set.isPending && set.variables?.id === a.id}
          onToggle={(done) => set.mutate({ id: a.id, roomId: a.roomId, done }, { onError: (e) => toast({ tone: 'danger', title: 'Could not update the action item', description: errMsg(e) }) })}
        />
      ))}
    </ul>
  );
}

/** Copilot sessions of the workspace (`GET /copilot/sessions?workspaceId=`), plus "My action items". */
export function SessionsPage() {
  const { current, loading } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const sessions = useSessions(current?.id);
  const tab = params.get('tab') === 'actions' ? 'actions' : 'sessions';
  const starting = params.get('new') === '1';
  const patch = (next: Record<string, string | null>) => {
    const p = new URLSearchParams(params);
    for (const [k, v] of Object.entries(next)) { if (v === null) p.delete(k); else p.set(k, v); }
    setParams(p, { replace: true });
  };
  if (loading) return <Skeleton className="h-40" />;
  const items = sessions.data ?? [];
  return (
    <>
      <PageHeader
        title="Sessions"
        description="Copilot sessions: transcripts, suggestions, follow-ups and minutes."
        actions={<Button variant="primary" onClick={() => patch({ new: '1' })}><Sparkle size={16} weight="bold" aria-hidden />Start session</Button>}
      />
      <Tabs value={tab} onValueChange={(v) => patch({ tab: v === 'actions' ? 'actions' : null })}>
        <TabsList className="px-4 py-3" aria-label="Sessions views">
          <TabsTrigger value="sessions">Sessions</TabsTrigger>
          <TabsTrigger value="actions">Action items</TabsTrigger>
        </TabsList>
        <TabsContent value="sessions" className="border-t border-row">
          {sessions.isError ? (
            <PageBody><Alert tone="danger" title="Could not load sessions">{errMsg(sessions.error)}</Alert></PageBody>
          ) : sessions.isLoading ? (
            <div aria-hidden><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /><Skeleton className="h-11 rounded-none" /></div>
          ) : items.length === 0 ? (
            <PageBody>
              <EmptyState
                icon={<Sparkle size={20} weight="duotone" />} title="No sessions yet"
                description="Start a copilot session for a Google Meet, Zoom or Teams call, or an in-person meeting, and get a live transcript, suggestions and minutes."
                action={<Button variant="primary" onClick={() => patch({ new: '1' })}>Start session</Button>}
              />
            </PageBody>
          ) : (
            <>
              <GroupHeader><span className="font-medium text-ink">All sessions</span><span aria-hidden>·</span><span className="tabular-nums" data-testid="sessions-count">{items.length}</span></GroupHeader>
              <div role="list" aria-label="Sessions">{items.map((s) => <SessionRow key={s.id} s={s} />)}</div>
            </>
          )}
        </TabsContent>
        <TabsContent value="actions" className="border-t border-row"><ActionItemsTab /></TabsContent>
      </Tabs>
      <StartSessionDialog open={starting} onOpenChange={(v) => patch({ new: v ? '1' : null })} />
    </>
  );
}

function TranscriptBand({ roomId, live }: { roomId: string; live: boolean }) {
  const t = useTranscript(roomId, live);
  const lines = t.data ?? [];
  const end = useRef<HTMLDivElement>(null);
  useEffect(() => { if (live) end.current?.scrollIntoView?.({ block: 'nearest' }); }, [live, lines.length]);
  return (
    <section aria-label="Transcript" className="min-w-0">
      <GroupHeader>
        <h2 className="font-medium text-ink">Transcript</h2><span aria-hidden>·</span><span className="tabular-nums" data-testid="transcript-count">{lines.length}</span>
        {live && <Badge tone="success" size="sm" className="ml-auto" data-testid="transcript-live">Updating live</Badge>}
      </GroupHeader>
      {t.isError ? (
        <PageBody><Alert tone="danger" title="Could not load the transcript">{errMsg(t.error)}</Alert></PageBody>
      ) : t.isLoading ? (
        <PageBody><Skeleton className="h-24" /></PageBody>
      ) : lines.length === 0 ? (
        <p className="px-4 py-4 text-body-sm text-ink-muted" data-testid="transcript-empty">{live ? 'Waiting for the first words. The transcript appears here as people talk.' : 'No transcript was captured for this session.'}</p>
      ) : (
        <div tabIndex={0} aria-label="Transcript lines" className="max-h-[480px] overflow-y-auto focus-ring">
          <ol>
            {lines.map((l) => (
              <li key={l.id} data-testid="transcript-line" data-mine={l.mine} className="flex gap-3 border-b border-row px-4 py-2">
                <span className="w-12 shrink-0 pt-0.5 text-caption tabular-nums text-ink-subtle">{mmss(l.atMs)}</span>
                <span className="min-w-0 flex-1">
                  <span className="block text-body-sm font-medium text-ink-muted">{l.speaker}</span>
                  <span className={`block break-words text-body ${l.final ? 'text-ink' : 'text-ink-subtle'}`}>{l.text}</span>
                </span>
              </li>
            ))}
          </ol>
          <div ref={end} />
        </div>
      )}
    </section>
  );
}

function failureText(code?: string) {
  if (code === 'LLM_NOT_CONFIGURED') return 'No AI provider is configured for this workspace. Add one in Settings, then regenerate.';
  if (code === 'NO_TRANSCRIPT') return 'There was no transcript to summarise.';
  return code ? `Error: ${code}. Try regenerating.` : 'Try regenerating.';
}

function SummaryBand({ roomId, ended, waiting, onRequested }: { roomId: string; ended: boolean; waiting: boolean; onRequested(): void }) {
  const summary = useSummary(roomId, true, waiting);
  const regen = useRegenerateSummary(roomId);
  const toast = useToast();
  const s = summary.data;
  return (
    <section aria-label="Summary" className="min-w-0">
      <GroupHeader className="pr-2">
        <h2 className="font-medium text-ink">Summary and minutes</h2>
        {ended && (
          <Button
            size="sm" className="ml-auto" loading={regen.isPending}
            onClick={() => regen.mutate(undefined, {
              onSuccess: () => { onRequested(); toast({ tone: 'success', title: 'Regenerating the summary', description: 'It appears here when it is ready.' }); },
              onError: (e) => toast({ tone: 'danger', title: 'Could not regenerate the summary', description: errMsg(e) }),
            })}
          >
            <ArrowsClockwise size={14} aria-hidden />{s && (s.short || s.detailed) ? 'Regenerate summary' : 'Generate summary'}
          </Button>
        )}
      </GroupHeader>
      <div className="px-4 py-3">
        {summary.isLoading ? <Skeleton className="h-16" /> : summary.isError ? <Alert tone="danger" title="Could not load the summary">{errMsg(summary.error)}</Alert> : s && (s.short || s.detailed) ? (
          <div className="flex flex-col gap-3" data-testid="summary">
            {s.short && <p className="whitespace-pre-wrap break-words text-body font-medium text-ink">{s.short}</p>}
            {s.detailed && <p className="whitespace-pre-wrap break-words text-body text-ink">{s.detailed}</p>}
            {s.decisions.length > 0 && (
              <div>
                <h3 className="text-body-sm font-semibold text-ink">Decisions</h3>
                <ul className="mt-1 list-disc pl-5 text-body text-ink">{s.decisions.map((d, i) => <li key={i}>{d}</li>)}</ul>
              </div>
            )}
            {s.openQuestions.length > 0 && (
              <div>
                <h3 className="text-body-sm font-semibold text-ink">Open questions</h3>
                <ul className="mt-1 list-disc pl-5 text-body text-ink">{s.openQuestions.map((d, i) => <li key={i}>{d}</li>)}</ul>
              </div>
            )}
            {summaryBusy(s) && <p className="text-caption text-ink-subtle">Updating the summary…</p>}
            {s.generatedAt && <p className="text-caption text-ink-subtle">Generated {formatWhen(s.generatedAt)}</p>}
          </div>
        ) : s?.status === 'failed' ? (
          <Alert tone="warning" title="The summary could not be generated">{failureText(s.errorCode)}</Alert>
        ) : (
          <p className="text-body-sm text-ink-muted" data-testid="summary-empty">
            {!ended ? 'The summary and minutes are generated when the session ends.' : waiting || summaryBusy(s) ? 'Generating the summary. This page updates when it is ready.' : 'No summary yet.'}
          </p>
        )}
      </div>
    </section>
  );
}

function ActionItemsBand({ roomId }: { roomId: string }) {
  const q = useActionItems(roomId);
  const set = useSetActionItemDone(roomId);
  const toast = useToast();
  const items = q.data ?? [];
  const open = items.filter((i) => !i.done).length;
  return (
    <section aria-label="Action items" className="min-w-0">
      <GroupHeader>
        <h2 className="font-medium text-ink">Action items</h2><span aria-hidden>·</span><span className="tabular-nums" data-testid="action-items-open">{open} open</span>
      </GroupHeader>
      {q.isError ? <PageBody><Alert tone="danger" title="Could not load action items">{errMsg(q.error)}</Alert></PageBody>
        : q.isLoading ? <PageBody><Skeleton className="h-11" /></PageBody>
        : items.length === 0 ? <p className="px-4 py-3 text-body-sm text-ink-muted" data-testid="action-items-empty">No action items. They are extracted when the session ends; owners and deadlines only appear when they were said in the meeting.</p>
        : (
          <ul aria-label="Action items list">
            {items.map((a) => (
              <ActionItemRow
                key={a.id} item={a} busy={set.isPending && set.variables?.id === a.id}
                onToggle={(done) => set.mutate({ id: a.id, done }, { onError: (e) => toast({ tone: 'danger', title: 'Could not update the action item', description: errMsg(e) }) })}
              />
            ))}
          </ul>
        )}
    </section>
  );
}

/** `/sessions/:sessionId`: live transcript (polled while IN_PROGRESS), suggestions, follow-ups, summary, action items. */
export function SessionDetailPage() {
  const { sessionId = '' } = useParams();
  const { wsPath } = useWorkspace();
  const sess = useSession(sessionId);
  const s = sess.data;
  const live = sessionIsLive(s);
  const ended = !!s && ['ENDED', 'EXPIRED', 'CANCELLED', 'FAILED'].includes(s.status);
  const native = useNative();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const end = useEndSession(sessionId);
  const [waiting, setWaiting] = useState(false);
  const wasLive = useRef(false);
  useEffect(() => { if (wasLive.current && !live) setWaiting(true); wasLive.current = live; }, [live]);

  const stream = useSuggestionStream(sessionId, undefined, live);
  const triage = useSuggestionTriage(stream.items);
  const followUps = useFollowUps(sessionId, !!s);

  if (sess.isLoading) return <PageBody className="flex flex-col gap-3"><Skeleton className="h-10 w-64" /><Skeleton className="h-40" /></PageBody>;
  if (sess.isError || !s) {
    return (
      <>
        <PageHeader size="detail" title="Session not found" description="This session does not exist, or you do not have access to it." />
        <PageBody><EmptyState title="Looking for your sessions?" description={sess.isError ? errMsg(sess.error) : undefined} action={<Link to={wsPath('/sessions')} className="text-body">All sessions</Link>} /></PageBody>
      </>
    );
  }
  const canCapture = typeof native?.startCopilotSession === 'function';
  async function capture(start: boolean) {
    setBusy(true);
    try {
      if (start) await native!.startCopilotSession!({ sessionId, accessToken: session.accessToken ?? '', apiBaseUrl: apiBaseUrl() || window.location.origin });
      else await native!.stopCopilotSession?.();
    } catch (e) { toast({ tone: 'danger', title: start ? 'Could not start capture' : 'Could not stop capture', description: errMsg(e) }); }
    setBusy(false);
  }
  return (
    <div className="flex min-h-full flex-col">
      <header className="flex flex-col gap-3 px-4 pb-4 pt-5">
        <h1 className="break-words text-headline font-semibold text-ink max-sm:text-title" title={s.title}>{s.title}</h1>
        <div className="flex flex-wrap items-center gap-2">
          <StatusPill status={s.status} />
          <Capsule size="lg">{platformLabel(s.platform)}</Capsule>
          {s.languages?.me && s.languages.others && <Capsule size="lg" title="Languages: me / others">Me {languageLabel(s.languages.me)} · Others {languageLabel(s.languages.others)}</Capsule>}
          {(s.startedAt ?? s.createdAt) && <DateCapsule size="lg" text={formatShortDate(s.startedAt ?? s.createdAt)} />}
          {s.meetingUrl && /^https?:\/\//.test(s.meetingUrl) && (
            <a href={s.meetingUrl} target="_blank" rel="noreferrer noopener" className="inline-flex items-center gap-1 text-body-sm"><LinkIcon size={14} aria-hidden />Meeting link</a>
          )}
        </div>
        {!ended && (
          <div className="flex flex-wrap gap-2">
            <Button
              variant="danger-soft" loading={end.isPending}
              onClick={() => end.mutate(undefined, { onSuccess: () => setWaiting(true), onError: (e) => toast({ tone: 'danger', title: 'Could not end the session', description: errMsg(e) }) })}
            >End session</Button>
          </div>
        )}
        {!ended && (canCapture ? (
          <div className="flex flex-wrap gap-2">
            {live
              ? <Button variant="danger-soft" loading={busy} onClick={() => void capture(false)}><Stop size={14} weight="fill" aria-hidden />Stop capture</Button>
              : <Button variant="primary" loading={busy} onClick={() => void capture(true)}><Play size={14} weight="fill" aria-hidden />Start capture</Button>}
          </div>
        ) : <CaptureNotice />)}
      </header>
      <div className="grid min-w-0 border-t border-row lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <TranscriptBand roomId={sessionId} live={live} />
        <section aria-label="Suggestions and follow-ups" className="min-w-0 border-row lg:border-l">
          <GroupHeader><h2 className="font-medium text-ink">Suggestions and follow-ups</h2></GroupHeader>
          <SuggestionsTab view={{ items: triage.visible, followUps: followUps.data ?? [], onDismiss: triage.dismiss, onSnooze: triage.snooze }} />
        </section>
      </div>
      <div className="flex min-w-0 flex-col border-t border-row">
        <SummaryBand roomId={sessionId} ended={ended} waiting={waiting} onRequested={() => setWaiting(true)} />
        <ActionItemsBand roomId={sessionId} />
      </div>
    </div>
  );
}
