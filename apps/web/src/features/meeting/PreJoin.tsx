import { useEffect, useRef } from 'react';
import { Camera, CameraSlash, Microphone, VideoCamera, WarningCircle } from '@phosphor-icons/react';
import { Alert, Button, Card, CardBody, Checkbox, Field, Input, Select, StatusPill } from '@/ui';
import { languageLabel } from '@/lib/languages';
import type { Room } from '@/lib/schemas';
import type { MediaPreview } from './useMediaPreview';

export interface PreJoinValues { name: string; speak: string; listen: string; joinMuted: boolean }

/** Languages a room can use: the source language plus every target. */
export function roomLanguages(r: Pick<Room, 'sourceLanguage' | 'targetLanguages'>): string[] {
  return [r.sourceLanguage, ...r.targetLanguages.filter((l) => l !== r.sourceLanguage)];
}

function LevelMeter({ level, active }: { level: number; active: boolean }) {
  const pct = Math.round(level * 100);
  return (
    <div className="flex items-center gap-3">
      <Microphone className="size-4 shrink-0 text-ink-subtle" aria-hidden />
      <div role="meter" aria-label="Microphone level" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} data-testid="mic-meter" data-active={active} className="h-2 flex-1 overflow-hidden rounded-pill bg-surface-3">
        <div className="h-full rounded-pill bg-success transition-[width] duration-75" style={{ width: `${pct}%` }} />
      </div>
      <span className="w-24 text-right text-caption text-ink-subtle">{!active ? 'No microphone' : pct > 4 ? 'Hearing you' : 'Say something'}</span>
    </div>
  );
}

function Preview({ preview, name }: { preview: MediaPreview; name: string }) {
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.srcObject = preview.hasVideo && preview.camOn ? preview.stream : null;
    if (el.srcObject) void el.play().catch(() => undefined);
  }, [preview.stream, preview.hasVideo, preview.camOn]);
  const showVideo = preview.hasVideo && preview.camOn;
  return (
    <div className="relative flex aspect-video w-full items-center justify-center overflow-hidden rounded-lg border border-hairline bg-surface-2" data-testid="camera-preview" data-camera={showVideo ? 'on' : 'off'}>
      <video ref={ref} muted playsInline aria-label="Camera preview" className={showVideo ? 'size-full -scale-x-100 object-cover' : 'hidden'} />
      {!showVideo && (
        <div className="flex flex-col items-center gap-2 px-4 text-center text-ink-subtle">
          <CameraSlash className="size-8" aria-hidden />
          <p className="text-body-sm">{preview.status === 'requesting' ? 'Starting camera…' : preview.camOn && !preview.hasVideo ? 'No camera available' : 'Camera is off'}</p>
        </div>
      )}
      <span className="absolute bottom-2 left-2 max-w-[70%] truncate rounded-md bg-black/75 px-2 py-0.5 text-caption text-white">{name || 'You'}</span>
      <Button
        size="sm"
        variant="secondary"
        className="absolute bottom-2 right-2"
        aria-pressed={preview.camOn}
        aria-label={preview.camOn ? 'Turn camera off' : 'Turn camera on'}
        onClick={() => preview.setCamOn(!preview.camOn)}
      >
        {preview.camOn ? <Camera className="size-4" aria-hidden /> : <CameraSlash className="size-4" aria-hidden />}
        {preview.camOn ? 'Camera on' : 'Camera off'}
      </Button>
    </div>
  );
}

export function PreJoin({
  room, values, onChange, preview, onJoin, joining, notice,
}: {
  room: Room;
  values: PreJoinValues;
  onChange(v: Partial<PreJoinValues>): void;
  preview: MediaPreview;
  onJoin(): void;
  joining: boolean;
  notice?: { tone: 'warning' | 'danger'; title: string; body: string } | null;
}) {
  const langs = roomLanguages(room).map((c) => ({ value: c, label: languageLabel(c) }));
  const micSel = preview.micId || preview.stream?.getAudioTracks()[0]?.getSettings().deviceId || '';
  const camSel = preview.camId || preview.stream?.getVideoTracks()[0]?.getSettings().deviceId || '';
  const sameLang = values.speak === values.listen;
  const blocked = preview.status === 'denied' || preview.status === 'error';
  return (
    <Card className="mx-auto w-full max-w-4xl" data-testid="meeting-prejoin">
      <CardBody className="grid gap-x-6 gap-y-4 p-4 sm:p-6 lg:grid-cols-[1.15fr_1fr] lg:grid-rows-[auto_1fr]">
        <div className="flex min-w-0 flex-col gap-3 lg:col-start-1 lg:row-start-1">
          <Preview preview={preview} name={values.name} />
          <LevelMeter level={preview.level} active={preview.hasAudio} />
          {blocked && (
            <Alert tone="warning">
              <span className="flex items-start gap-2"><WarningCircle className="mt-0.5 size-4 shrink-0" aria-hidden /><span data-testid="media-blocked">{preview.message}</span></span>
              <Button size="sm" className="mt-2" onClick={preview.retry}>Try again</Button>
            </Alert>
          )}
        </div>
        <div className="order-3 grid min-w-0 content-start gap-3 sm:grid-cols-2 lg:order-none lg:col-start-1 lg:row-start-2">
          <Field label="Microphone">
            <Select
              value={micSel}
              onValueChange={preview.setMicId}
              options={preview.mics.map((d) => ({ value: d.id, label: d.label }))}
              placeholder={preview.status === 'requesting' ? 'Looking for microphones…' : 'No microphone found'}
              disabled={preview.mics.length === 0}
            />
          </Field>
          <Field label="Camera">
            <Select
              value={camSel}
              onValueChange={preview.setCamId}
              options={preview.cams.map((d) => ({ value: d.id, label: d.label }))}
              placeholder={preview.status === 'requesting' ? 'Looking for cameras…' : 'No camera found'}
              disabled={preview.cams.length === 0}
            />
          </Field>
        </div>
        <div className="order-2 flex min-w-0 flex-col gap-4 lg:order-none lg:col-start-2 lg:row-span-2 lg:row-start-1">
          <div className="flex items-center gap-3">
            <span className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-surface-2 text-ink-subtle"><VideoCamera className="size-5" aria-hidden /></span>
            <div className="min-w-0">
              <h2 className="truncate text-title font-semibold">Ready to join?</h2>
              <p className="flex items-center gap-2 text-body-sm text-ink-subtle"><StatusPill status={room.status} /><span className="truncate">{room.title}</span></p>
            </div>
          </div>
          {notice && <Alert tone={notice.tone} title={notice.title}>{notice.body}</Alert>}
          <Field label="Display name (optional)"><Input value={values.name} onChange={(e) => onChange({ name: e.target.value })} maxLength={100} autoComplete="name" /></Field>
          <div className="grid grid-cols-2 gap-3">
            <Field label="I speak"><Select value={values.speak} onValueChange={(v) => onChange({ speak: v })} options={langs} /></Field>
            <Field label="I listen in" hint={sameLang ? 'Same as the language you speak: no dubbing for you.' : undefined}><Select value={values.listen} onValueChange={(v) => onChange({ listen: v })} options={langs} /></Field>
          </div>
          <Checkbox checked={values.joinMuted} onCheckedChange={(v) => onChange({ joinMuted: v })} label="Join with my microphone muted" />
          <Button variant="primary" size="lg" loading={joining} onClick={onJoin}>Join meeting</Button>
        </div>
      </CardBody>
    </Card>
  );
}
