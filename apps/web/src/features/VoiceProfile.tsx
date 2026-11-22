import { useEffect, useRef, useState } from 'react';
import { Microphone, Record, Stop, UploadSimple, Waveform } from '@phosphor-icons/react';
import { Alert, Badge, Button, Card, CardBody, CardHeader, Checkbox, Dialog, DialogContent, Skeleton, useToast } from '@/ui';
import { useAuth } from '@/lib/auth';
import { useEnrollVoice, useGrantVoiceConsent, useRevokeVoiceConsent, useVoiceStatus } from '@/lib/queries';
import { recordingToWav, sha256Hex, toBase64, wavInfo } from '@/lib/wav';
import { formatWhen } from './common';
import { errCode, errMsg } from './errors';

export const CONSENT_VERSION = '2026-11';
/** The exact text shown to the user; its SHA-256 is sent as the acknowledgement hash. */
export const CONSENT_PARAGRAPHS = [
  'I agree that WarpTalk may create a synthetic copy of my voice (a voice clone) from a recording that I provide.',
  'What is stored: the voice sample I submit, a voice profile with a quality score, and a voice identifier at the voice provider. A record of this consent (time, text version, IP address and browser) is kept as proof.',
  'How it is used: only to speak my translated words in my own voice in meetings I take part in. Consent is checked again for every sentence.',
  'Revoking: I can revoke at any time. Revoking stops cloned speech, deletes my stored samples and profile, and queues the provider voice for deletion.',
];
export const CONSENT_TEXT = CONSENT_PARAGRAPHS.join('\n\n');

export const MIN_RECORD_SECONDS = 10;
export const MAX_RECORD_SECONDS = 20;
const MAX_UPLOAD_BYTES = 6 * 1024 * 1024;

const VOICE_ERRORS: Record<string, string> = {
  VOICE_SAMPLE_TOO_QUIET: 'The sample is too quiet. Move closer to the microphone, speak at a normal volume and try again.',
  VOICE_SAMPLE_TOO_SHORT: 'The sample is too short. Record at least 10 seconds of continuous speech.',
  VOICE_SAMPLE_TOO_LONG: 'The sample is too long. Keep it under 30 seconds (10 to 20 seconds is ideal).',
  VOICE_SAMPLE_TOO_LITTLE_SPEECH: 'There is too little speech in the sample. Speak throughout the recording and avoid long pauses.',
  VOICE_SAMPLE_CLIPPED: 'The sample is distorted (too loud). Move back from the microphone or lower the input volume.',
  VOICE_SAMPLE_RATE_UNSUPPORTED: 'The audio sample rate is not supported. Use a recording between 16 and 48 kHz.',
  VOICE_SAMPLE_INVALID: 'That audio could not be used. Record again, or upload a 16-bit PCM WAV file (mono or stereo).',
  VOICE_CONSENT_REQUIRED: 'Voice cloning consent is required first. Grant consent above, then submit the sample.',
  VOICE_CLONING_UNAVAILABLE: 'Voice cloning is not available on this server: no voice provider is configured. Ask your administrator to set one up.',
};

/** Friendly text for an enrollment/consent failure; falls back to the API message. */
export function voiceErrorMessage(e: unknown): string {
  const code = errCode(e);
  return (code && VOICE_ERRORS[code]) || errMsg(e);
}

const clock = (s: number) => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`;

function LevelMeter({ level, active }: { level: number; active: boolean }) {
  const pct = Math.round(level * 100);
  return (
    <div className="flex items-center gap-3">
      <Microphone className="size-4 shrink-0 text-ink-subtle" aria-hidden />
      <div role="meter" aria-label="Voice level" aria-valuemin={0} aria-valuemax={100} aria-valuenow={pct} data-testid="voice-meter" data-active={active} className="h-2 flex-1 overflow-hidden rounded-pill bg-surface-3">
        <div className="h-full rounded-pill bg-success transition-[width] duration-75" style={{ width: `${pct}%` }} />
      </div>
      <span className="w-24 text-right text-caption text-ink-subtle">{!active ? 'Not recording' : pct > 4 ? 'Hearing you' : 'Say something'}</span>
    </div>
  );
}

interface Sample { wav: Uint8Array; seconds: number; label: string; url: string }

function Enroll({ userId }: { userId: string | null }) {
  const enroll = useEnrollVoice(userId);
  const toast = useToast();
  const fileRef = useRef<HTMLInputElement>(null);
  const [phase, setPhase] = useState<'idle' | 'recording' | 'processing' | 'ready'>('idle');
  const [sample, setSample] = useState<Sample | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [elapsed, setElapsed] = useState(0);
  const [level, setLevel] = useState(0);
  const [result, setResult] = useState<{ score: number; seconds: number } | null>(null);
  const rt = useRef<{ rec?: MediaRecorder; stream?: MediaStream; ac?: AudioContext; raf?: number; timer?: ReturnType<typeof setInterval>; ctl?: { discard: boolean } }>({});

  function cleanup() {
    const r = rt.current;
    if (r.raf) cancelAnimationFrame(r.raf);
    if (r.timer) clearInterval(r.timer);
    r.stream?.getTracks().forEach((t) => t.stop());
    void r.ac?.close();
    rt.current = {};
    setLevel(0);
  }
  useEffect(() => () => { if (rt.current.ctl) rt.current.ctl.discard = true; cleanup(); }, []);
  useEffect(() => () => { if (sample) URL.revokeObjectURL(sample.url); }, [sample]);

  const clear = () => { setSample(null); setPhase('idle'); setProblem(null); setElapsed(0); enroll.reset(); };

  async function start() {
    clear();
    setResult(null);
    if (!navigator.mediaDevices?.getUserMedia || typeof MediaRecorder === 'undefined') {
      setProblem('Recording is not supported in this browser. Upload a WAV file instead.');
      return;
    }
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: { echoCancellation: true, noiseSuppression: true } });
    } catch (e) {
      const name = e instanceof DOMException ? e.name : '';
      setProblem(name === 'NotFoundError' ? 'No microphone was found. Connect one, or upload a WAV file instead.' : 'Microphone access was blocked. Allow it in your browser settings, or upload a WAV file instead.');
      return;
    }
    const rec = new MediaRecorder(stream);
    const chunks: Blob[] = [];
    const ctl = { discard: false };
    rec.ondataavailable = (e) => { if (e.data.size > 0) chunks.push(e.data); };
    rec.onstop = () => {
      const discard = ctl.discard;
      const type = rec.mimeType;
      cleanup();
      if (discard) return;
      setPhase('processing');
      recordingToWav(new Blob(chunks, { type }))
        .then(({ wav, seconds }) => {
          setSample({ wav, seconds, label: 'Recording', url: URL.createObjectURL(new Blob([wav as BlobPart], { type: 'audio/wav' })) });
          setPhase('ready');
        })
        .catch(() => { setProblem('The recording could not be processed. Try again, or upload a WAV file instead.'); setPhase('idle'); });
    };
    const ac = new AudioContext();
    const an = ac.createAnalyser();
    an.fftSize = 1024;
    ac.createMediaStreamSource(stream).connect(an);
    const buf = new Float32Array(an.fftSize);
    let smooth = 0;
    const tick = () => {
      an.getFloatTimeDomainData(buf);
      let sum = 0;
      for (const v of buf) sum += v * v;
      smooth = smooth * 0.7 + Math.min(1, Math.sqrt(sum / buf.length) * 4) * 0.3;
      setLevel(smooth);
      rt.current.raf = requestAnimationFrame(tick);
    };
    const t0 = Date.now();
    const timer = setInterval(() => {
      const s = (Date.now() - t0) / 1000;
      setElapsed(s);
      if (s >= MAX_RECORD_SECONDS && rec.state === 'recording') rec.stop();
    }, 100);
    rt.current = { rec, stream, ac, timer, ctl };
    rec.start(250);
    setPhase('recording');
    tick();
  }
  const stop = () => { if (rt.current.rec?.state === 'recording') rt.current.rec.stop(); };
  const cancel = () => { if (rt.current.ctl) rt.current.ctl.discard = true; stop(); cleanup(); clear(); };

  async function pickFile(f: File | undefined) {
    if (!f) return;
    clear();
    setResult(null);
    if (f.size > MAX_UPLOAD_BYTES) { setProblem('That file is too large. Use a WAV file under 6 MB (about 30 seconds of speech).'); return; }
    const wav = new Uint8Array(await f.arrayBuffer());
    const info = wavInfo(wav);
    if (!info) { setProblem('That is not a WAV recording. Export it as 16-bit PCM WAV (mono or stereo) and try again.'); return; }
    setSample({ wav, seconds: info.seconds, label: f.name, url: URL.createObjectURL(new Blob([wav as BlobPart], { type: 'audio/wav' })) });
    setPhase('ready');
  }

  async function submit() {
    if (!sample) return;
    setProblem(null);
    try {
      const r = await enroll.mutateAsync(toBase64(sample.wav));
      setResult({ score: r.qualityScore, seconds: r.durationSeconds });
      toast({ tone: 'success', title: 'Voice sample accepted', description: `Quality score ${r.qualityScore} / 100` });
      setSample(null);
      setPhase('idle');
    } catch { /* shown below */ }
  }

  const canStop = elapsed >= MIN_RECORD_SECONDS;
  return (
    <Card>
      <CardHeader title="Voice sample" description={`Record ${MIN_RECORD_SECONDS} to ${MAX_RECORD_SECONDS} seconds of natural speech in a quiet room, or upload a WAV file. A new sample replaces the previous one.`} />
      <CardBody className="flex flex-col gap-4">
        {phase === 'recording' && (
          <div className="flex flex-col gap-3" data-testid="voice-recording">
            <LevelMeter level={level} active />
            <p className="flex items-center gap-2 text-body" role="timer" aria-label="Recording time">
              <Record size={14} weight="fill" className="text-danger-text" aria-hidden />
              <span data-testid="voice-timer">{clock(elapsed)} / {clock(MAX_RECORD_SECONDS)}</span>
            </p>
            {!canStop && <p className="text-caption text-ink-subtle">Keep talking for at least {MIN_RECORD_SECONDS} seconds, then stop.</p>}
            <div className="flex flex-wrap gap-2">
              <Button variant="primary" disabled={!canStop} onClick={stop}><Stop size={14} weight="fill" aria-hidden />Stop recording</Button>
              <Button onClick={cancel}>Cancel</Button>
            </div>
          </div>
        )}
        {phase === 'processing' && <p className="text-body text-ink-subtle" role="status">Preparing your recording…</p>}
        {phase === 'ready' && sample && (
          <div className="flex flex-col gap-3" data-testid="voice-ready">
            <p className="text-body"><span className="font-medium">{sample.label}</span> <span className="text-ink-subtle">· {sample.seconds.toFixed(1)} s</span></p>
            <audio controls src={sample.url} aria-label="Preview of your voice sample" className="w-full" />
            <div className="flex flex-wrap gap-2">
              <Button variant="primary" loading={enroll.isPending} onClick={() => void submit()}>Submit sample</Button>
              <Button disabled={enroll.isPending} onClick={clear}>Discard</Button>
            </div>
          </div>
        )}
        {(phase === 'idle') && (
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" onClick={() => void start()}><Microphone size={16} aria-hidden />Record sample</Button>
            <Button onClick={() => fileRef.current?.click()}><UploadSimple size={16} aria-hidden />Upload WAV</Button>
          </div>
        )}
        <input ref={fileRef} type="file" hidden data-testid="voice-file-input" accept=".wav,audio/wav,audio/x-wav" onChange={(e) => { void pickFile(e.target.files?.[0]); e.target.value = ''; }} />
        {problem && <Alert tone="danger">{problem}</Alert>}
        {enroll.isError && <Alert tone="danger"><span data-testid="voice-error" data-code={errCode(enroll.error)}>{voiceErrorMessage(enroll.error)}</span></Alert>}
        {result && <Alert tone="success" title="Voice sample accepted"><span data-testid="voice-result">Quality score {result.score} / 100 · {result.seconds.toFixed(1)} s of audio.</span></Alert>}
      </CardBody>
    </Card>
  );
}

export function VoiceSection() {
  const { userId } = useAuth();
  const q = useVoiceStatus(userId);
  const grant = useGrantVoiceConsent(userId);
  const revoke = useRevokeVoiceConsent(userId);
  const toast = useToast();
  const [agree, setAgree] = useState(false);
  const [confirm, setConfirm] = useState(false);

  async function consent() {
    try {
      const acknowledgementHash = await sha256Hex(CONSENT_TEXT);
      await grant.mutateAsync({ consentVersion: CONSENT_VERSION, acknowledgementHash });
      setAgree(false);
      toast({ tone: 'success', title: 'Consent recorded', description: 'You can now record a voice sample.' });
    } catch { /* inline */ }
  }
  async function doRevoke() {
    try {
      await revoke.mutateAsync();
      setConfirm(false);
      toast({ tone: 'success', title: 'Consent revoked', description: 'Your voice sample and profile are being deleted.' });
    } catch { /* inline */ }
  }

  const st = q.data;
  const granted = !!st?.consent.granted;
  return (
    <section aria-label="Voice cloning" className="flex flex-col gap-4" data-testid="voice-section">
      <Card>
        <CardHeader
          title="Voice cloning"
          description="Hear your translated speech in a copy of your own voice. Off unless you consent."
          actions={st ? <Badge tone={granted ? 'success' : 'neutral'} data-testid="voice-consent-state" data-granted={granted}><span aria-hidden className="size-1.5 rounded-pill bg-current" />{granted ? 'Consent granted' : 'No consent'}</Badge> : undefined}
        />
        <CardBody className="flex flex-col gap-4">
          {q.isLoading ? <Skeleton className="h-32" /> : q.isError ? <Alert tone="danger">{errMsg(q.error)}</Alert> : !granted ? (
            <>
              <div className="flex flex-col gap-2 rounded-lg bg-surface-2 p-4" data-testid="consent-text">
                <p className="text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">Consent text · version {CONSENT_VERSION}</p>
                {CONSENT_PARAGRAPHS.map((p) => <p key={p} className="text-body-sm text-ink-muted">{p}</p>)}
              </div>
              <Checkbox checked={agree} onCheckedChange={setAgree} label="I have read the text above and agree to it." description="Your agreement is stored together with a fingerprint (SHA-256) of this exact text." />
              {grant.isError && <Alert tone="danger">{voiceErrorMessage(grant.error)}</Alert>}
              <div><Button variant="primary" disabled={!agree} loading={grant.isPending} onClick={() => void consent()}>I consent</Button></div>
            </>
          ) : (
            <>
              <dl className="grid gap-3 sm:grid-cols-2" data-testid="voice-status">
                <div className="flex flex-col gap-0.5">
                  <dt className="text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">Consent granted</dt>
                  <dd className="text-body" data-testid="voice-granted-at">{formatWhen(st?.consent.grantedAt)} · version {st?.consent.consentVersion}</dd>
                </div>
                <div className="flex flex-col gap-0.5">
                  <dt className="text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">Profile quality</dt>
                  <dd className="text-body" data-testid="voice-quality">{st?.profile?.qualityScore != null ? `${st.profile.qualityScore} / 100` : 'No sample yet'}</dd>
                </div>
                <div className="flex flex-col gap-0.5 sm:col-span-2">
                  <dt className="text-eyebrow font-medium uppercase tracking-[0.4px] text-ink-subtle">Cloned voice</dt>
                  <dd className="flex flex-wrap items-center gap-2 text-body" data-testid="voice-clone-state" data-enabled={st?.cloneEnabled}>
                    <Badge tone={st?.cloneEnabled ? 'success' : 'neutral'}><Waveform size={12} aria-hidden />{st?.cloneEnabled ? 'Enabled' : 'Not enabled'}</Badge>
                    <span className="text-body-sm text-ink-subtle">
                      {st?.cloneEnabled ? 'Your translated speech can use your cloned voice.' : st?.profile ? `A sample is on file but no cloned voice exists at the provider (status: ${st.profile.status.replace(/_/g, ' ')}).` : 'Record a sample to create your cloned voice.'}
                    </span>
                  </dd>
                </div>
              </dl>
              {revoke.isError && <Alert tone="danger">{errMsg(revoke.error)}</Alert>}
              <div><Button variant="danger-soft" onClick={() => setConfirm(true)}>Revoke consent</Button></div>
            </>
          )}
        </CardBody>
      </Card>
      {granted && <Enroll userId={userId} />}
      <Dialog open={confirm} onOpenChange={(v) => { if (!v) { setConfirm(false); revoke.reset(); } }}>
        <DialogContent
          title="Revoke voice cloning consent?"
          description="Cloned speech stops immediately. Your stored voice sample and profile are deleted and the voice at the provider is queued for deletion. You can consent and record again later."
          footer={<><Button onClick={() => setConfirm(false)}>Cancel</Button><Button variant="danger" loading={revoke.isPending} onClick={() => void doRevoke()}>Revoke and delete</Button></>}
        >
          {revoke.isError && <Alert tone="danger">{errMsg(revoke.error)}</Alert>}
        </DialogContent>
      </Dialog>
    </section>
  );
}
