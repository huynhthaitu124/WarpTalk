import { useEffect, useState, type ReactNode } from 'react';
import { AppWindow, DownloadSimple as Download, Desktop as MonitorSmartphone, ShieldCheck } from '@phosphor-icons/react';
import { Badge, Button, Card, CardBody, CardHeader } from '@v2/ui';
import { cn } from '@v2/lib/cn';
import type { NativeError, PermissionKind, PermissionState, Permissions } from '@v2/lib/native';

/** Re-render every `ms` so relative times ("3s ago") stay current. */
export function useNow(ms = 1000) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

export function ago(iso: string | null | undefined, now: number): string {
  if (!iso) return 'never';
  const s = Math.max(0, Math.round((now - Date.parse(iso)) / 1000));
  if (Number.isNaN(s)) return 'never';
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  return `${Math.floor(s / 3600)}h ago`;
}

export function BrowserFallback() {
  return (
    <Card data-testid="external-browser-fallback">
      <CardHeader
        title="Capture needs the WarpTalk desktop app"
        description="A browser cannot listen to another application's audio. You can still follow captions here."
        actions={<Badge tone="info">Browser mode</Badge>}
      />
      <CardBody className="grid gap-6 md:grid-cols-2">
        <div className="flex gap-3">
          <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-ink-subtle"><Download className="size-5" aria-hidden /></span>
          <div>
            <h3 className="text-body font-semibold text-ink">Get the macOS app</h3>
            <ol className="mt-2 list-decimal space-y-1 pl-5 text-body-sm text-ink-subtle">
              <li>Install WarpTalk for macOS 14.2 or later (the desktop app is macOS-only for now).</li>
              <li>Open the app and sign in with this account.</li>
              <li>Open External meeting inside the app and grant the audio permission when macOS asks.</li>
            </ol>
          </div>
        </div>
        <div className="flex gap-3">
          <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-ink-subtle"><MonitorSmartphone className="size-5" aria-hidden /></span>
          <div>
            <h3 className="text-body font-semibold text-ink">Viewing only</h3>
            <p className="mt-2 text-body-sm text-ink-subtle">
              Paste the same meeting link below to join the room as a viewer. When a teammate captures the meeting from the
              desktop app, translated captions appear in this tab.
            </p>
          </div>
        </div>
      </CardBody>
    </Card>
  );
}

const DB_FLOOR = -60;
export function LevelMeter({ dbfs }: { dbfs: number }) {
  const db = Math.max(DB_FLOOR, Math.min(0, Math.round(dbfs)));
  const pct = ((db - DB_FLOOR) / -DB_FLOOR) * 100;
  const silent = dbfs <= DB_FLOOR;
  return (
    <div className="flex items-center gap-3">
      <div
        role="meter"
        aria-label="Audio level"
        aria-valuemin={DB_FLOOR}
        aria-valuemax={0}
        aria-valuenow={db}
        aria-valuetext={silent ? 'Silent' : `${db} dBFS`}
        data-testid="level-meter"
        data-db={db}
        className="h-2 flex-1 overflow-hidden rounded-pill bg-surface-3"
      >
        <div className="h-full rounded-pill bg-primary transition-[width] duration-100" style={{ width: `${pct}%` }} />
      </div>
      <span data-testid="level-text" className="w-20 text-right font-mono text-caption text-ink-subtle">{silent ? 'Silent' : `${db} dBFS`}</span>
    </div>
  );
}

const PERM_LABEL: Record<PermissionKind, { label: string; hint: string }> = {
  microphone: { label: 'Microphone', hint: 'Your own voice, for voice mode in the meeting app.' },
  screenRecording: { label: 'Screen recording', hint: 'Only needed for the ScreenCaptureKit fallback.' },
  audioCapture: { label: 'Audio capture', hint: 'Hear the meeting app. macOS asks the first time capture starts.' },
};
const PERM_TONE: Record<PermissionState, 'success' | 'danger' | 'warning' | 'neutral'> = {
  granted: 'success', denied: 'danger', notDetermined: 'warning', unknown: 'neutral',
};
const PERM_TEXT: Record<PermissionState, string> = {
  granted: 'Granted', denied: 'Denied', notDetermined: 'Not requested', unknown: 'Unknown',
};

export function PermissionsPanel({
  perms, busy, onRefresh, onRequest, onOpen,
}: {
  perms: Permissions | null;
  busy: boolean;
  onRefresh(): void;
  onRequest(k: PermissionKind): void;
  onOpen(k: PermissionKind): void;
}) {
  return (
    <Card aria-label="Permissions">
      <CardHeader
        title="Permissions"
        description="macOS privacy settings the capture needs"
        actions={<Button size="sm" loading={busy} onClick={onRefresh}>Refresh</Button>}
      />
      {!perms ? (
        <CardBody><p className="text-body-sm text-ink-subtle">Checking permissions…</p></CardBody>
      ) : (
        <ul aria-label="macOS permissions" className="divide-y divide-hairline">
          {(Object.keys(PERM_LABEL) as PermissionKind[]).map((k) => {
            const st = perms[k] ?? 'unknown';
            const requestable = k !== 'audioCapture';
            return (
              <li key={k} data-testid={`perm-${k}`} data-state={st} className="flex flex-wrap items-center gap-3 px-5 py-3">
                <div className="min-w-0 flex-1">
                  <p className="text-body font-medium text-ink">{PERM_LABEL[k].label}</p>
                  <p className="text-caption text-ink-subtle">{PERM_LABEL[k].hint}</p>
                </div>
                <Badge tone={PERM_TONE[st]}>{PERM_TEXT[st]}</Badge>
                {st !== 'granted' && (
                  <div className="flex gap-2">
                    {requestable && <Button size="sm" aria-label={`Request ${PERM_LABEL[k].label.toLowerCase()} access`} onClick={() => onRequest(k)}>Request</Button>}
                    <Button size="sm" variant="ghost" aria-label={`Open settings for ${PERM_LABEL[k].label.toLowerCase()}`} onClick={() => onOpen(k)}>Open settings</Button>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}

export interface Recovery { hint: string; actions: { id: string; label: string; run(): void }[] }

/** One native/API error with the way out. */
export function ErrorRow({ error, recovery, onDismiss }: { error: NativeError; recovery: Recovery; onDismiss(): void }) {
  return (
    <div role="alert" data-testid="capture-error" data-code={error.code} className="rounded-lg bg-danger-soft px-4 py-3 text-body-sm text-danger-text">
      <p className="font-semibold">{error.fatal ? 'Capture stopped' : 'Capture problem'}: {error.message}</p>
      <p className="mt-0.5">{recovery.hint}</p>
      <div className="mt-3 flex flex-wrap gap-2">
        {recovery.actions.map((a) => <Button key={a.id} size="sm" onClick={a.run}>{a.label}</Button>)}
        <Button size="sm" variant="ghost" onClick={onDismiss}>Dismiss</Button>
      </div>
      <p className="mt-2 font-mono text-caption">{error.code}</p>
    </div>
  );
}

export function StepCard({ title, description, actions, children, testId }: { title: string; description?: ReactNode; actions?: ReactNode; children: ReactNode; testId?: string }) {
  return (
    <Card data-testid={testId}>
      <CardHeader title={title} description={description} actions={actions} />
      <CardBody className="flex flex-col gap-4">{children}</CardBody>
    </Card>
  );
}

export function KeyValue({ label, children, testId }: { label: string; children: ReactNode; testId?: string }) {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="text-eyebrow uppercase text-ink-subtle">{label}</dt>
      <dd data-testid={testId} className="text-body text-ink">{children}</dd>
    </div>
  );
}

export function AppRow({ name, producing, className }: { name: string; producing: boolean; className?: string }) {
  return (
    <span className={cn('flex items-center gap-2', className)}>
      <AppWindow className="size-4 text-ink-subtle" aria-hidden />
      {name}
      <Badge tone={producing ? 'success' : 'neutral'}>{producing ? 'Playing audio' : 'Silent'}</Badge>
    </span>
  );
}

export const NativeBadge = ({ text }: { text: string }) => (
  <Badge tone="success" data-testid="native-connected"><ShieldCheck className="size-3.5" aria-hidden />{text}</Badge>
);
