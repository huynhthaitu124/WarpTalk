import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { useNavigate } from 'react-router-dom';
import * as RD from '@radix-ui/react-dialog';
import * as DM from '@radix-ui/react-dropdown-menu';
import {
  ArrowsInSimple, ArrowsOutSimple, CalendarBlank, CaretDown, CheckCircle, DotsThree, LinkSimple, Plus, ShieldCheck, Trash, VideoCamera, X, Globe,
} from '@phosphor-icons/react';
import { Tooltip, useToast } from '@/ui';
import { cn } from '@/lib/cn';
import { useProfile } from '@/lib/auth';
import { useWorkspace } from '@/lib/workspace';
import { useCreateRoom } from '@/lib/queries';
import { api } from '@/lib/api';
import { LANGUAGES, languageLabel } from '@/lib/languages';
import { PROVIDER_LABEL, parseMeetingLink } from '@/lib/bridge';
import { errMsg } from './errors';

/**
 * Create meeting dialog, ported from upstream `create-room-dialog.tsx`: borderless title + description,
 * a row of pills (languages, start time, options) and a single submit in the footer.
 *
 * Meeting types: a WarpTalk meeting (LiveKit room) or an external meeting held in Microsoft Teams / Google Meet,
 * where WarpTalk listens to the call app and shows a floating copilot overlay (transcript + suggestions).
 */
export type MeetingKind = 'WARPTALK' | 'EXTERNAL';
const KINDS: { value: MeetingKind; label: string; icon: ReactNode }[] = [
  { value: 'WARPTALK', label: 'WarpTalk meeting', icon: <VideoCamera size={14} weight="duotone" /> },
  { value: 'EXTERNAL', label: 'External meeting', icon: <Globe size={14} weight="duotone" /> },
];

const popover = 'z-[70] rounded-xl bg-popover p-1.5 shadow-pop animate-pop-in';
const pad2 = (n: number) => String(n).padStart(2, '0');
function nextFullHour() { const d = new Date(); d.setMinutes(0, 0, 0); d.setHours(d.getHours() + 1); return d; }
function toLocalInput(d: Date) { return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}T${pad2(d.getHours())}:${pad2(d.getMinutes())}`; }
const fmtStart = (d: Date) => d.toLocaleString('en-US', { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' });

export function CreateRoomDialog({ open, onOpenChange, defaultKind = 'WARPTALK' }: { open: boolean; onOpenChange(v: boolean): void; defaultKind?: MeetingKind }) {
  const { current } = useWorkspace();
  const create = useCreateRoom();
  const toast = useToast();
  const nav = useNavigate();
  const profile = useProfile();

  const [kind, setKind] = useState<MeetingKind>(defaultKind);
  const [expanded, setExpanded] = useState(false);
  const [title, setTitle] = useState('');
  const [description, setDescription] = useState('');
  const [languages, setLanguages] = useState<string[]>(['en', 'vi']);
  const [myLang, setMyLang] = useState('vi');
  const [link, setLink] = useState('');
  const [startAt, setStartAt] = useState<Date | null>(null);
  const [approval, setApproval] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => { if (open) setKind(defaultKind); }, [open, defaultKind]);
  // Reset 400ms after closing (upstream), so the closing animation does not flash an empty form.
  useEffect(() => {
    if (open) return;
    const t = setTimeout(() => {
      setTitle(''); setDescription(''); setLanguages(['en', 'vi']); setMyLang('vi'); setLink(''); setStartAt(null);
      setApproval(false); setError(null); setExpanded(false);
    }, 400);
    return () => clearTimeout(t);
  }, [open]);

  const external = kind === 'EXTERNAL';
  const parsed = useMemo(() => parseMeetingLink(link), [link]);
  const farLangs = languages.filter((l) => l !== myLang);
  const instant = !startAt;
  const valid = external
    ? !!title.trim() || parsed.ok
    : !!title.trim() && languages.length > 0;

  const toggleLang = (code: string) => setLanguages((cur) => {
    if (cur.includes(code)) return cur.length === 1 ? cur : cur.filter((c) => c !== code);
    return [...cur, code];
  });

  async function submit() {
    setError(null);
    if (!current) { setError('Please select a workspace before creating a room.'); return; }
    if (external) {
      if (!parsed.ok) { setError(parsed.error); return; }
      const far = farLangs[0] ?? (myLang === 'en' ? 'vi' : 'en');
      const q = new URLSearchParams({ link: link.trim(), far, listen: myLang, auto: '1' });
      if (title.trim()) q.set('title', title.trim());
      onOpenChange(false);
      nav(`/external?${q.toString()}`);
      return;
    }
    if (!title.trim()) { setError('Give the meeting a title.'); return; }
    try {
      const r = await create.mutateAsync({
        workspaceId: current.id, title: title.trim(), description: description.trim() || undefined,
        sourceLanguage: languages[0]!, targetLanguages: [...new Set(languages)], requiresApproval: approval,
        displayName: profile.data?.fullName,
        scheduledAt: startAt ? startAt.toISOString() : undefined,
      });
      if (instant) {
        try {
          await startRoomNow(r.id);
          onOpenChange(false);
          nav(`/meetings/${r.id}`);
          return;
        } catch {
          toast({ tone: 'danger', title: 'Room created, but it could not be started.' });
        }
      } else {
        toast({ tone: 'success', title: 'Room created successfully.', description: r.title });
      }
      onOpenChange(false);
      nav(`/rooms/${r.id}`);
    } catch (e) {
      const m = errMsg(e) || 'Failed to create room.';
      setError(m);
      toast({ tone: 'danger', title: m });
    }
  }
  // useRoomAction is bound to an id at render time; call the endpoint directly for the room we just created.
  async function startRoomNow(id: string) {
    await api(`/translation-rooms/${id}/start`, { method: 'POST' });
  }

  const pending = create.isPending;
  const submitLabel = external
    ? 'Start copilot'
    : pending ? (instant ? 'Starting...' : 'Saving...') : instant ? 'Start meeting' : 'Create Room';
  const kindInfo = KINDS.find((k) => k.value === kind)!;

  return (
    <RD.Root open={open} onOpenChange={onOpenChange}>
      <RD.Portal>
        <RD.Overlay className="fixed inset-0 z-50 animate-fade-in bg-black/40" />
        <RD.Content
          className={cn(
            'fixed left-1/2 z-50 flex w-full max-w-[calc(100vw-2rem)] -translate-x-1/2 flex-col overflow-hidden rounded-xl bg-elevated shadow-[0_8px_40px_-12px_rgba(0,0,0,0.3)] transition-[top,height] duration-300 focus:outline-none sm:max-w-[750px]',
            expanded ? 'bottom-[5vh] top-[5vh] sm:h-[90vh]' : 'top-[12vh]',
          )}
          onOpenAutoFocus={(e) => { e.preventDefault(); document.getElementById('create-title')?.focus(); }}
        >
          <RD.Title className="sr-only">Create new meeting</RD.Title>
          <RD.Description className="sr-only">Configure and create a new translation room</RD.Description>

          {/* Row 1: context header */}
          <div className="flex items-center justify-between px-5 pb-1 pt-3">
            <div className="flex items-center gap-1.5 text-[12px] font-medium text-ink-muted/80">
              <span className="rounded bg-surface-2 px-1.5 py-0.5 text-ink/80">{current?.name ?? 'Workspace'}</span>
              <span aria-hidden>›</span>
              <DM.Root>
                <DM.Trigger className="flex items-center gap-1 rounded px-1.5 py-0.5 text-ink hover:bg-surface-2 focus-ring" data-testid="meeting-kind">
                  {kindInfo.label}<CaretDown size={12} weight="bold" className="text-ink-muted" />
                </DM.Trigger>
                <DM.Portal>
                  <DM.Content align="start" sideOffset={6} className={cn(popover, 'w-[240px] p-1')}>
                    <DM.Label className="px-2 py-1 text-[11px] text-ink-muted">Meeting Type</DM.Label>
                    {KINDS.map((k) => (
                      <DM.Item key={k.value} onSelect={() => setKind(k.value)}
                        className="flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 text-[13px] outline-none data-[highlighted]:bg-surface-2">
                        <span className="text-ink-muted">{k.icon}</span><span className="font-medium text-ink">{k.label}</span>
                      </DM.Item>
                    ))}
                  </DM.Content>
                </DM.Portal>
              </DM.Root>
            </div>
            <div className="flex items-center">
              <Tooltip content={expanded ? 'Collapse' : 'Expand'}>
                <button type="button" onClick={() => setExpanded((v) => !v)} aria-label={expanded ? 'Collapse' : 'Expand'}
                  className="rounded-md p-1.5 text-ink-muted hover:bg-surface-2 hover:text-ink focus-ring">
                  {expanded ? <ArrowsInSimple size={14} weight="bold" /> : <ArrowsOutSimple size={14} weight="bold" />}
                </button>
              </Tooltip>
              <RD.Close aria-label="Close dialog" className="rounded-md p-1.5 text-ink-muted hover:bg-surface-2 hover:text-ink focus-ring">
                <X size={14} weight="bold" />
              </RD.Close>
            </div>
          </div>

          {/* Row 2: text */}
          <form
            id="create-room-form"
            onSubmit={(e) => { e.preventDefault(); if (valid && !pending) void submit(); }}
            className={cn('flex flex-1 flex-col gap-2 px-5 pb-2 pt-3', expanded && 'overflow-y-auto')}
          >
            <input id="create-title" aria-label="Meeting title" maxLength={200} value={title} onChange={(e) => setTitle(e.target.value)}
              placeholder="Meeting title" className="w-full border-none bg-transparent p-0 text-[18px] font-medium text-ink outline-none placeholder:text-ink-muted/50" />
            {external ? (
              <div className="flex flex-col gap-1">
                <div className="flex items-center gap-2 text-[14px]">
                  <LinkSimple size={14} className="shrink-0 text-ink-muted" aria-hidden />
                  <input aria-label="Meeting link" value={link} onChange={(e) => setLink(e.target.value)}
                    placeholder="Paste the Teams or Google Meet link..." data-testid="external-link"
                    className="w-full border-none bg-transparent p-0 text-ink outline-none placeholder:text-ink-muted/60" />
                  {parsed.ok && <span className="shrink-0 rounded bg-surface-2 px-1.5 py-0.5 text-[11px] font-medium text-ink-muted">{PROVIDER_LABEL[parsed.provider]}</span>}
                </div>
              </div>
            ) : (
              <textarea aria-label="Description" maxLength={5000} value={description} onChange={(e) => setDescription(e.target.value)}
                placeholder="Add description..."
                className={cn('w-full resize-none border-none bg-transparent p-0 text-[14px] text-ink outline-none placeholder:text-ink-muted/60', expanded ? 'min-h-[300px] flex-1' : 'min-h-[60px]')} />
            )}
          </form>

          {/* Row 3: pills */}
          <div className="mt-2 flex flex-wrap items-center gap-2 px-5 pb-[5px] pt-2">
            <LanguagePill languages={languages} onToggle={toggleLang} />
            {external && (
              <DM.Root>
                <DM.Trigger className={pill(true)} aria-label="My language">
                  <span className="text-ink-muted">I read</span>
                  <Code code={myLang} />{languageLabel(myLang)}<CaretDown size={12} weight="bold" className="text-ink-muted" />
                </DM.Trigger>
                <DM.Portal>
                  <DM.Content align="start" sideOffset={6} className={cn(popover, 'w-[210px]')}>
                    <DM.Label className="px-2 py-1 text-[11px] text-ink-muted">My language</DM.Label>
                    {LANGUAGES.map((l) => (
                      <LangRow key={l.code} code={l.code} label={l.label} on={myLang === l.code} onSelect={() => { setMyLang(l.code); if (!languages.includes(l.code)) setLanguages((c) => [...c, l.code]); }} />
                    ))}
                  </DM.Content>
                </DM.Portal>
              </DM.Root>
            )}
            {!external && startAt && (
              <span className={cn(pill(true), 'pr-1')}>
                <CalendarBlank size={14} weight="duotone" />
                <input type="datetime-local" aria-label="Start time" value={toLocalInput(startAt)}
                  onChange={(e) => { const d = new Date(e.target.value); if (!Number.isNaN(d.getTime())) setStartAt(d); }}
                  className="w-[150px] border-none bg-transparent p-0 text-[12px] text-ink outline-none" title={fmtStart(startAt)} />
                <Tooltip content="Remove schedule">
                  <button type="button" aria-label="Remove schedule" onClick={() => setStartAt(null)} className="rounded-full p-1 text-red-500 hover:bg-red-500/10">
                    <Trash size={12} weight="bold" />
                  </button>
                </Tooltip>
              </span>
            )}
            {!external && (
              <DM.Root>
                <DM.Trigger aria-label="Meeting options"
                  className="grid h-[26px] w-[26px] place-items-center rounded-full border border-hairline bg-surface-1 text-ink-muted shadow-[0_1px_2px_rgba(0,0,0,0.04)] hover:bg-surface-2 hover:text-ink focus-ring">
                  <DotsThree size={16} weight="bold" />
                </DM.Trigger>
                <DM.Portal>
                  <DM.Content align="start" sideOffset={6} className={cn(popover, 'w-[262px]')}>
                    {!startAt && (
                      <OptionRow icon={<CalendarBlank size={16} weight="duotone" />} label="Date & Time" onSelect={() => setStartAt(nextFullHour())} />
                    )}
                    <OptionRow icon={<ShieldCheck size={16} weight="duotone" />} label="Require approval" on={approval} onSelect={() => setApproval((v) => !v)} keepOpen />
                  </DM.Content>
                </DM.Portal>
              </DM.Root>
            )}
          </div>

          {/* Footer */}
          <div className="flex items-center justify-between gap-4 bg-surface-1/50 px-5 py-3">
            {error ? <p role="alert" className="min-w-0 text-[12px] leading-snug text-danger-text">{error}</p> : <span />}
            <button type="submit" form="create-room-form" disabled={!valid || pending}
              className="h-[30px] shrink-0 rounded-md bg-ink px-3.5 text-[13px] font-medium text-canvas shadow-sm hover:opacity-90 disabled:opacity-40 focus-ring">
              {submitLabel}
            </button>
          </div>
        </RD.Content>
      </RD.Portal>
    </RD.Root>
  );
}

const pill = (active: boolean) => cn(
  'flex items-center gap-1.5 rounded-full border px-2.5 py-1 text-[12px] font-medium shadow-[0_1px_2px_rgba(0,0,0,0.04)] focus-ring',
  active ? 'border-hairline-strong bg-surface-1 text-ink hover:bg-surface-2' : 'border-hairline bg-surface-1 text-ink-muted hover:bg-surface-2 hover:text-ink',
);

function Code({ code }: { code: string }) {
  return <span className="rounded bg-surface-3 px-1 py-px text-[10px] font-medium uppercase leading-none tracking-wide text-ink-muted">{code}</span>;
}

function LanguagePill({ languages, onToggle }: { languages: string[]; onToggle(code: string): void }) {
  const single = languages.length === 1;
  return (
    <DM.Root>
      <DM.Trigger aria-label="Meeting languages" data-testid="language-pill"
        className="flex items-center gap-1 rounded-full border border-hairline bg-transparent px-1 py-1 text-[13px] focus-ring">
        {languages.map((c, i) => (
          <span key={c} className="flex items-center">
            {i > 0 && <span aria-hidden className="px-0.5 text-ink-muted/40">·</span>}
            <span title={languageLabel(c)} className="flex items-center gap-1.5 rounded-full px-2.5 py-[3px] hover:bg-surface-2">
              <Code code={c} />{single && <span className="text-ink">{languageLabel(c)}</span>}
            </span>
          </span>
        ))}
        <span className="rounded-full px-2 py-[5px] hover:bg-surface-2"><Plus size={12} weight="bold" className="text-ink-muted" /></span>
      </DM.Trigger>
      <DM.Portal>
        <DM.Content align="start" sideOffset={6} className={cn(popover, 'w-[210px]')}>
          <DM.Label className="px-2 py-1 text-[11px] text-ink-muted">Meeting languages</DM.Label>
          {LANGUAGES.map((l) => (
            <LangRow key={l.code} code={l.code} label={l.label} on={languages.includes(l.code)} onSelect={(e) => { e.preventDefault(); onToggle(l.code); }} />
          ))}
        </DM.Content>
      </DM.Portal>
    </DM.Root>
  );
}

function LangRow({ code, label, on, onSelect }: { code: string; label: string; on: boolean; onSelect(e: Event): void }) {
  return (
    <DM.Item onSelect={onSelect} className="mb-0.5 flex cursor-default items-center justify-between gap-2 rounded-md px-2 py-1.5 text-[13px] outline-none data-[highlighted]:bg-surface-2">
      <span className="flex min-w-0 items-center gap-2"><Code code={code} /><span className="truncate font-medium text-ink">{label}</span></span>
      {on && <CheckCircle size={14} weight="fill" className="text-[#3b82f6]" />}
    </DM.Item>
  );
}

function OptionRow({ icon, label, on, onSelect, keepOpen }: { icon: ReactNode; label: string; on?: boolean; onSelect(): void; keepOpen?: boolean }) {
  return (
    <DM.Item aria-pressed={on} onSelect={(e) => { if (keepOpen) e.preventDefault(); onSelect(); }}
      className="flex cursor-default items-center gap-2 rounded-md px-2 py-1.5 outline-none data-[highlighted]:bg-surface-2">
      <span className="text-ink-muted">{icon}</span>
      <span className="flex-1 whitespace-nowrap text-[13px] font-medium text-ink">{label}</span>
      {on !== undefined && (on
        ? <CheckCircle size={16} weight="fill" className="text-[#3b82f6]" />
        : <span className="h-4 w-4 rounded-full border border-hairline" />)}
    </DM.Item>
  );
}
