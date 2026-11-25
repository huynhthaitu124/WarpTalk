import type { ReactNode } from 'react';
import {
  CaretDown, Check, ClosedCaptioning, GearSix, HandPalm, Microphone, MicrophoneSlash, Pause, Play, Record, Screencast, ShieldCheck, SignOut, SmileyWink, Stop, Translate, UsersThree, Users, VideoCamera, VideoCameraSlash,
} from '@phosphor-icons/react';
import { Dropdown, DropdownContent, DropdownItem, DropdownLabel, DropdownSeparator, DropdownTrigger, FlagLabel, Tooltip } from '@/ui';
import { cn } from '@/lib/cn';
import { languageLabel } from '@/lib/languages';
import type { RoomAction } from '@/lib/queries';
import type { RoomStatus } from '@/lib/schemas';
import { REACTIONS } from './protocol';

const BASE = 'relative inline-flex size-10 shrink-0 items-center justify-center rounded-xl transition-colors focus-ring disabled:opacity-50 max-sm:size-9';
const IDLE = 'text-ink-muted hover:bg-surface-2 hover:text-ink';
const ACTIVE = 'bg-surface-2 text-primary';
const OFF = 'bg-red-50 text-red-600 hover:bg-red-100 dark:bg-red-500/15 dark:text-red-400';
const btn = (state: 'idle' | 'active' | 'off' = 'idle') => cn(BASE, state === 'active' ? ACTIVE : state === 'off' ? OFF : IDLE);
const LEAVE = 'inline-flex size-10 shrink-0 items-center justify-center rounded-xl text-red-600 transition-colors hover:bg-red-50 dark:hover:bg-red-500/15 focus-ring max-sm:size-9';
const SPLIT_L = 'rounded-l-xl rounded-r-none';
const CARET = 'inline-flex h-10 w-5 shrink-0 items-center justify-center rounded-l-none rounded-r-xl transition-colors focus-ring max-sm:h-9';

function Divider() {
  return <span aria-hidden className="mx-1 h-6 w-px shrink-0 bg-border max-sm:hidden" />;
}

function Tip({ label, children }: { label: string; children: ReactNode }) {
  return <Tooltip content={label}>{children as React.ReactElement}</Tooltip>;
}

export function hostActions(status: RoomStatus, translationActive: boolean): { action: RoomAction; label: string; icon: ReactNode }[] {
  if (['SCHEDULED', 'WAITING', 'OPEN'].includes(status)) return [{ action: 'start', label: 'Start translation', icon: <Play className="size-4" aria-hidden /> }];
  if (status === 'PAUSED') return [{ action: 'resume', label: 'Resume translation', icon: <Play className="size-4" aria-hidden /> }];
  if (status === 'IN_PROGRESS') {
    const out: { action: RoomAction; label: string; icon: ReactNode }[] = [{ action: 'pause', label: 'Pause translation', icon: <Pause className="size-4" aria-hidden /> }];
    if (translationActive) out.push({ action: 'translation/stop', label: 'Stop translation', icon: <Stop className="size-4" aria-hidden /> });
    return out;
  }
  return [];
}

export interface ControlBarProps {
  micOn: boolean; camOn: boolean; sharing: boolean; canShare: boolean;
  onMic(): void; onCamera(): void; onShare(): void;
  captionsOn: boolean; onCaptions(): void;
  listen: string; onListen(v: string): void; languages: string[];
  /** Own speak language (`none` = listen-only) and its setter; shown in the settings menu. */
  speak: string; onSpeak(v: string): void; languagesBusy: boolean;
  handRaised: boolean; onHand(): void; onReact(emoji: string): void;
  /** Recording state and toggle (host). Without `onRecord` the button is disabled. */
  recording?: boolean; onRecord?(): void;
  host: null | { status: RoomStatus; translationActive: boolean; busy: boolean; lobby: number; onAction(a: RoomAction): void; onAdmitAll(): void };
  onLeave(): void; onEnd(): void;
}

/** The single translation toggle shown as a pill: stop while translating, otherwise start / resume. */
function primaryHostAction(h: NonNullable<ControlBarProps['host']>): { action: RoomAction; label: string; stop: boolean } | null {
  if (h.status === 'IN_PROGRESS' && h.translationActive) return { action: 'translation/stop', label: 'Stop translation', stop: true };
  const first = hostActions(h.status, h.translationActive)[0];
  return first ? { action: first.action, label: first.label, stop: false } : null;
}

/** Icon button plus a caret that opens a small options menu (device menu). */
function Split({ label, off, onClick, icon, caretLabel, children }: { label: string; off: boolean; onClick(): void; icon: ReactNode; caretLabel: string; children: ReactNode }) {
  return (
    <div className="flex shrink-0 items-center">
      <Tip label={label}>
        <button type="button" className={cn(BASE, SPLIT_L, off ? OFF : IDLE)} aria-pressed={!off} aria-label={label} onClick={onClick}>{icon}</button>
      </Tip>
      <Dropdown>
        <DropdownTrigger asChild>
          <button type="button" className={cn(CARET, off ? OFF : IDLE)} aria-label={caretLabel}><CaretDown className="size-3" aria-hidden /></button>
        </DropdownTrigger>
        <DropdownContent align="center" side="top">{children}</DropdownContent>
      </Dropdown>
    </div>
  );
}

export function ControlBar(p: ControlBarProps) {
  const primary = p.host ? primaryHostAction(p.host) : null;
  return (
    <div role="toolbar" aria-label="Meeting controls" data-testid="control-bar" className="flex items-center">
      <div className="flex h-[52px] max-w-full items-center gap-1.5 rounded-full border border-border/50 bg-surface-1/80 px-2.5 shadow-sm backdrop-blur-xl max-sm:gap-0.5 max-sm:overflow-x-auto max-sm:px-1.5">
        {p.host && primary && (
          <>
            <button
              type="button"
              disabled={p.host.busy}
              onClick={() => p.host?.onAction(primary.action)}
              className={cn('inline-flex h-9 shrink-0 items-center gap-1.5 rounded-full px-3.5 text-[13px] font-medium transition-colors focus-ring disabled:opacity-50', primary.stop ? 'bg-red-50 text-red-600 hover:bg-red-100 dark:bg-red-500/15 dark:text-red-400' : 'bg-primary text-white hover:bg-primary-hover')}
            >
              {primary.stop ? <Stop weight="fill" className="size-4" aria-hidden /> : <Play weight="fill" className="size-4" aria-hidden />}
              <span className="max-sm:hidden">{primary.label}</span>
              <span className="sr-only sm:hidden">{primary.label}</span>
            </button>
            <Divider />
          </>
        )}
        <Dropdown>
          <Tip label="Listen language">
            <DropdownTrigger asChild>
              <button type="button" className="inline-flex h-9 shrink-0 items-center gap-1.5 rounded-full bg-surface-2 px-2.5 text-[13px] font-medium text-ink transition-colors hover:bg-surface-3 focus-ring" aria-label="Listen language" data-listen={p.listen}>
                <Translate className="size-4" aria-hidden />
                <span className="max-w-24 truncate max-sm:hidden">{p.listen === 'original' ? 'Original' : languageLabel(p.listen)}</span>
                <CaretDown className="size-3" aria-hidden />
              </button>
            </DropdownTrigger>
          </Tip>
          <DropdownContent align="center" side="top">
            <DropdownLabel>I want to hear</DropdownLabel>
            {['original', ...p.languages].map((l) => (
              <DropdownItem key={l} disabled={p.languagesBusy} icon={<Check className={cn('size-4', l !== p.listen && 'invisible')} aria-hidden />} onSelect={() => p.onListen(l)} data-testid={`listen-${l}`} data-current={l === p.listen}>
                {l === 'original' ? 'Original audio' : <FlagLabel code={l}>Dubbed: {languageLabel(l)}</FlagLabel>}{l === p.listen && <span className="sr-only"> (current)</span>}
              </DropdownItem>
            ))}
          </DropdownContent>
        </Dropdown>
        <Divider />
        {p.host && (
          <Dropdown>
            <Tip label="Host controls">
              <DropdownTrigger asChild>
                <button type="button" className={btn()} aria-label="Host controls" disabled={p.host.busy}>
                  <ShieldCheck className="size-5" aria-hidden />
                  {p.host.lobby > 0 && <span data-testid="panel-badge" className="absolute -right-0.5 -top-0.5 grid h-4 min-w-4 place-items-center rounded-full bg-primary px-1 text-[10px] font-semibold tabular-nums text-white">{p.host.lobby}</span>}
                </button>
              </DropdownTrigger>
            </Tip>
            <DropdownContent align="center" side="top">
              <DropdownLabel>Host controls</DropdownLabel>
              {hostActions(p.host.status, p.host.translationActive).map((a) => (
                <DropdownItem key={a.action} icon={a.icon} onSelect={() => p.host?.onAction(a.action)}>{a.label}</DropdownItem>
              ))}
              {p.host.lobby > 0 && <DropdownItem icon={<UsersThree className="size-4" aria-hidden />} onSelect={p.host.onAdmitAll}>Admit {p.host.lobby} waiting</DropdownItem>}
              {hostActions(p.host.status, p.host.translationActive).length === 0 && p.host.lobby === 0 && <p className="px-2 py-1.5 text-body-sm text-ink-subtle">No actions available.</p>}
            </DropdownContent>
          </Dropdown>
        )}
        {p.host && (
          <Tip label={p.onRecord ? (p.recording ? 'Stop recording' : 'Start recording') : 'Recording is not available'}>
            <button type="button" className={btn(p.recording ? 'off' : 'idle')} aria-pressed={!!p.recording} aria-label={p.recording ? 'Stop recording' : 'Record meeting'} disabled={!p.onRecord} onClick={p.onRecord}>
              <Record weight={p.recording ? 'fill' : 'regular'} className="size-5" aria-hidden />
            </button>
          </Tip>
        )}
        <Split label={p.micOn ? 'Mute microphone' : 'Unmute microphone'} off={!p.micOn} onClick={p.onMic} caretLabel="Microphone options" icon={p.micOn ? <Microphone className="size-5" aria-hidden /> : <MicrophoneSlash className="size-5" aria-hidden />}>
          <DropdownLabel>Microphone</DropdownLabel>
          <DropdownItem icon={p.micOn ? <MicrophoneSlash className="size-4" aria-hidden /> : <Microphone className="size-4" aria-hidden />} onSelect={p.onMic}>{p.micOn ? 'Mute' : 'Unmute'}</DropdownItem>
          <p className="px-2 py-1.5 text-body-sm text-ink-subtle">The input device follows your system setting.</p>
        </Split>
        <Split label={p.camOn ? 'Turn camera off' : 'Turn camera on'} off={!p.camOn} onClick={p.onCamera} caretLabel="Camera options" icon={p.camOn ? <VideoCamera className="size-5" aria-hidden /> : <VideoCameraSlash className="size-5" aria-hidden />}>
          <DropdownLabel>Camera</DropdownLabel>
          <DropdownItem icon={p.camOn ? <VideoCameraSlash className="size-4" aria-hidden /> : <VideoCamera className="size-4" aria-hidden />} onSelect={p.onCamera}>{p.camOn ? 'Turn off' : 'Turn on'}</DropdownItem>
          <p className="px-2 py-1.5 text-body-sm text-ink-subtle">The camera follows your system setting.</p>
        </Split>
        <Tip label={p.captionsOn ? 'Hide captions' : 'Show captions'}>
          <button type="button" className={btn(p.captionsOn ? 'active' : 'idle')} aria-pressed={p.captionsOn} aria-label={p.captionsOn ? 'Hide captions' : 'Show captions'} onClick={p.onCaptions}>
            <ClosedCaptioning weight={p.captionsOn ? 'fill' : 'regular'} className="size-5" aria-hidden />
          </button>
        </Tip>
        {p.canShare && (
          <Tip label={p.sharing ? 'Stop sharing' : 'Share screen'}>
            <button type="button" className={btn(p.sharing ? 'active' : 'idle')} aria-pressed={p.sharing} aria-label={p.sharing ? 'Stop sharing' : 'Share screen'} onClick={p.onShare}>
              <Screencast className="size-5" aria-hidden />
            </button>
          </Tip>
        )}
        <Tip label={p.handRaised ? 'Lower hand' : 'Raise hand'}>
          <button type="button" className={btn(p.handRaised ? 'active' : 'idle')} aria-pressed={p.handRaised} aria-label={p.handRaised ? 'Lower hand' : 'Raise hand'} onClick={p.onHand}>
            <HandPalm weight={p.handRaised ? 'fill' : 'regular'} className="size-5" aria-hidden />
          </button>
        </Tip>
        <Dropdown>
          <Tip label="Reactions">
            <DropdownTrigger asChild><button type="button" className={btn()} aria-label="Reactions"><SmileyWink className="size-5" aria-hidden /></button></DropdownTrigger>
          </Tip>
          <DropdownContent align="center" side="top" className="min-w-0 p-1">
            <div role="group" aria-label="Send a reaction" className="flex gap-1">
              {REACTIONS.map((r) => (
                <DropdownItem key={r.emoji} aria-label={r.label} className="size-9 justify-center px-0 text-title" onSelect={() => p.onReact(r.emoji)}>
                  <span aria-hidden>{r.emoji}</span>
                </DropdownItem>
              ))}
            </div>
          </DropdownContent>
        </Dropdown>
        <Divider />
        <Dropdown>
          <Tip label="Meeting settings">
            <DropdownTrigger asChild>
              <button type="button" className={btn()} aria-label="Meeting settings" data-speak={p.speak}><GearSix className="size-5" aria-hidden /></button>
            </DropdownTrigger>
          </Tip>
          <DropdownContent align="center" side="top">
            <DropdownLabel>I speak</DropdownLabel>
            {[...p.languages, 'none'].map((l) => (
              <DropdownItem key={l} disabled={p.languagesBusy} icon={<Check className={cn('size-4', l !== p.speak && 'invisible')} aria-hidden />} onSelect={() => p.onSpeak(l)} data-testid={`speak-${l}`} data-current={l === p.speak}>
                {l === 'none' ? 'Listen only (I do not speak)' : <FlagLabel code={l}>{languageLabel(l)}</FlagLabel>}{l === p.speak && <span className="sr-only"> (current)</span>}
              </DropdownItem>
            ))}
          </DropdownContent>
        </Dropdown>
        {p.host ? (
          <Dropdown>
            <Tip label="Leave">
              <DropdownTrigger asChild>
                <button type="button" className={LEAVE} aria-label="Leave or end meeting"><SignOut weight="bold" className="size-5" aria-hidden /></button>
              </DropdownTrigger>
            </Tip>
            <DropdownContent align="end" side="top">
              <DropdownItem icon={<SignOut className="size-4" aria-hidden />} onSelect={p.onLeave}>Leave meeting</DropdownItem>
              <DropdownSeparator />
              <DropdownItem danger icon={<Users className="size-4" aria-hidden />} onSelect={p.onEnd}>End meeting for all</DropdownItem>
            </DropdownContent>
          </Dropdown>
        ) : (
          <Tip label="Leave meeting">
            <button type="button" className={LEAVE} aria-label="Leave meeting" onClick={p.onLeave}><SignOut weight="bold" className="size-5" aria-hidden /></button>
          </Tip>
        )}
      </div>
    </div>
  );
}
