import type { ReactNode } from 'react';
import {
  Camera, CameraSlash, CaretUp, ChatCircleText, Check, ClosedCaptioning, GearSix, Hand, Microphone, MicrophoneSlash, MonitorArrowUp, Pause, Play, ShieldCheck, SignOut, Smiley, Stop, Translate, UsersThree, Users,
} from '@phosphor-icons/react';
import { Badge, Button, Dropdown, DropdownContent, DropdownItem, DropdownLabel, DropdownSeparator, DropdownTrigger, Tooltip } from '@/ui';
import { cn } from '@/lib/cn';
import { languageLabel } from '@/lib/languages';
import type { RoomAction } from '@/lib/queries';
import type { RoomStatus } from '@/lib/schemas';
import { REACTIONS } from './protocol';

const iconBtn = 'size-10 shrink-0 rounded-lg p-0 max-sm:size-9';

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
  panelOpen: boolean; onPanel(): void; panelBadge: number;
  host: null | { status: RoomStatus; translationActive: boolean; busy: boolean; lobby: number; onAction(a: RoomAction): void; onAdmitAll(): void };
  onLeave(): void; onEnd(): void;
}

export function ControlBar(p: ControlBarProps) {
  return (
    <div role="toolbar" aria-label="Meeting controls" data-testid="control-bar" className="flex shrink-0 flex-wrap items-center justify-center gap-2 border-t border-hairline bg-canvas px-3 py-3 max-sm:gap-1.5 max-sm:px-2">
      <Tip label={p.micOn ? 'Mute microphone' : 'Unmute microphone'}>
        <Button variant={p.micOn ? 'secondary' : 'danger'} className={iconBtn} aria-pressed={p.micOn} aria-label={p.micOn ? 'Mute microphone' : 'Unmute microphone'} onClick={p.onMic}>
          {p.micOn ? <Microphone weight="fill" className="size-5" aria-hidden /> : <MicrophoneSlash className="size-5" aria-hidden />}
        </Button>
      </Tip>
      <Tip label={p.camOn ? 'Turn camera off' : 'Turn camera on'}>
        <Button variant={p.camOn ? 'secondary' : 'danger'} className={iconBtn} aria-pressed={p.camOn} aria-label={p.camOn ? 'Turn camera off' : 'Turn camera on'} onClick={p.onCamera}>
          {p.camOn ? <Camera weight="fill" className="size-5" aria-hidden /> : <CameraSlash className="size-5" aria-hidden />}
        </Button>
      </Tip>
      {p.canShare && (
        <Tip label={p.sharing ? 'Stop sharing' : 'Share screen'}>
          <Button variant={p.sharing ? 'primary' : 'secondary'} className={iconBtn} aria-pressed={p.sharing} aria-label={p.sharing ? 'Stop sharing' : 'Share screen'} onClick={p.onShare}>
            <MonitorArrowUp className="size-5" aria-hidden />
          </Button>
        </Tip>
      )}
      <Tip label={p.captionsOn ? 'Hide captions' : 'Show captions'}>
        <Button variant={p.captionsOn ? 'primary' : 'secondary'} className={iconBtn} aria-pressed={p.captionsOn} aria-label={p.captionsOn ? 'Hide captions' : 'Show captions'} onClick={p.onCaptions}>
          <ClosedCaptioning weight={p.captionsOn ? 'fill' : 'regular'} className="size-5" aria-hidden />
        </Button>
      </Tip>
      <Dropdown>
        <Tip label="Listen language">
          <DropdownTrigger asChild>
            <Button className={iconBtn} aria-label="Listen language" data-listen={p.listen}><Translate className="size-5" aria-hidden /></Button>
          </DropdownTrigger>
        </Tip>
        <DropdownContent align="center" side="top">
          <DropdownLabel>I want to hear</DropdownLabel>
          {['original', ...p.languages].map((l) => (
            <DropdownItem key={l} disabled={p.languagesBusy} icon={<Check className={cn('size-4', l !== p.listen && 'invisible')} aria-hidden />} onSelect={() => p.onListen(l)} data-testid={`listen-${l}`} data-current={l === p.listen}>
              {l === 'original' ? 'Original audio' : `Dubbed: ${languageLabel(l)}`}{l === p.listen && <span className="sr-only"> (current)</span>}
            </DropdownItem>
          ))}
        </DropdownContent>
      </Dropdown>
      <Dropdown>
        <Tip label="Meeting settings">
          <DropdownTrigger asChild>
            <Button className={iconBtn} aria-label="Meeting settings" data-speak={p.speak}><GearSix className="size-5" aria-hidden /></Button>
          </DropdownTrigger>
        </Tip>
        <DropdownContent align="center" side="top">
          <DropdownLabel>I speak</DropdownLabel>
          {[...p.languages, 'none'].map((l) => (
            <DropdownItem key={l} disabled={p.languagesBusy} icon={<Check className={cn('size-4', l !== p.speak && 'invisible')} aria-hidden />} onSelect={() => p.onSpeak(l)} data-testid={`speak-${l}`} data-current={l === p.speak}>
              {l === 'none' ? 'Listen only (I do not speak)' : languageLabel(l)}{l === p.speak && <span className="sr-only"> (current)</span>}
            </DropdownItem>
          ))}
        </DropdownContent>
      </Dropdown>
      <Tip label={p.handRaised ? 'Lower hand' : 'Raise hand'}>
        <Button variant={p.handRaised ? 'primary' : 'secondary'} className={iconBtn} aria-pressed={p.handRaised} aria-label={p.handRaised ? 'Lower hand' : 'Raise hand'} onClick={p.onHand}>
          <Hand weight={p.handRaised ? 'fill' : 'regular'} className="size-5" aria-hidden />
        </Button>
      </Tip>
      <Dropdown>
        <Tip label="Reactions">
          <DropdownTrigger asChild><Button className={iconBtn} aria-label="Reactions"><Smiley className="size-5" aria-hidden /></Button></DropdownTrigger>
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
      <Tip label={p.panelOpen ? 'Close side panel' : 'Open transcript, people and chat'}>
        <Button variant={p.panelOpen ? 'primary' : 'secondary'} className={cn(iconBtn, 'relative')} aria-pressed={p.panelOpen} aria-label={p.panelOpen ? 'Close side panel' : 'Open side panel'} onClick={p.onPanel}>
          <ChatCircleText className="size-5" aria-hidden />
          {p.panelBadge > 0 && <Badge tone="warning" data-testid="panel-badge" className="absolute -right-1 -top-1 px-1.5">{p.panelBadge}</Badge>}
        </Button>
      </Tip>
      {p.host && (
        <Dropdown>
          <Tip label="Host controls">
            <DropdownTrigger asChild>
              <Button className={iconBtn} aria-label="Host controls" disabled={p.host.busy}><ShieldCheck className="size-5" aria-hidden /></Button>
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
      {p.host ? (
        <Dropdown>
          <DropdownTrigger asChild>
            <Button variant="danger" className="rounded-lg max-sm:px-3" aria-label="Leave or end meeting"><SignOut className="size-4" aria-hidden />Leave<CaretUp className="size-3" aria-hidden /></Button>
          </DropdownTrigger>
          <DropdownContent align="end" side="top">
            <DropdownItem icon={<SignOut className="size-4" aria-hidden />} onSelect={p.onLeave}>Leave meeting</DropdownItem>
            <DropdownSeparator />
            <DropdownItem danger icon={<Users className="size-4" aria-hidden />} onSelect={p.onEnd}>End meeting for all</DropdownItem>
          </DropdownContent>
        </Dropdown>
      ) : (
        <Button variant="danger" className="rounded-lg max-sm:px-3" aria-label="Leave meeting" onClick={p.onLeave}><SignOut className="size-4" aria-hidden />Leave</Button>
      )}
    </div>
  );
}
