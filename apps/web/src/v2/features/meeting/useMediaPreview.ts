import { useCallback, useEffect, useRef, useState } from 'react';

export type PreviewStatus = 'idle' | 'requesting' | 'ready' | 'denied' | 'unsupported' | 'error';
export interface Device { id: string; label: string }

export interface MediaPreview {
  status: PreviewStatus;
  message: string | null;
  stream: MediaStream | null;
  hasVideo: boolean;
  hasAudio: boolean;
  mics: Device[];
  cams: Device[];
  micId: string;
  camId: string;
  setMicId(id: string): void;
  setCamId(id: string): void;
  camOn: boolean;
  setCamOn(on: boolean): void;
  /** 0..1 smoothed input level. */
  level: number;
  retry(): void;
  stop(): void;
}

function toDevices(list: MediaDeviceInfo[], kind: MediaDeviceKind, fallback: string): Device[] {
  return list
    .filter((d) => d.kind === kind && d.deviceId)
    .map((d, i) => ({ id: d.deviceId, label: d.label || `${fallback} ${i + 1}` }));
}

/**
 * Camera + microphone preview for the pre-join card: permission state, device lists (kept fresh on
 * `devicechange`), a live input-level meter and an on/off camera toggle that really stops the track.
 */
export function useMediaPreview(): MediaPreview {
  const supported = typeof navigator !== 'undefined' && !!navigator.mediaDevices?.getUserMedia;
  const [status, setStatus] = useState<PreviewStatus>(supported ? 'idle' : 'unsupported');
  const [message, setMessage] = useState<string | null>(null);
  const [stream, setStream] = useState<MediaStream | null>(null);
  const [mics, setMics] = useState<Device[]>([]);
  const [cams, setCams] = useState<Device[]>([]);
  const [micId, setMicId] = useState('');
  const [camId, setCamId] = useState('');
  const [camOn, setCamOn] = useState(true);
  const [level, setLevel] = useState(0);
  const [attempt, setAttempt] = useState(0);
  const cur = useRef<MediaStream | null>(null);

  const stop = useCallback(() => {
    cur.current?.getTracks().forEach((t) => t.stop());
    cur.current = null;
    setStream(null);
    setLevel(0);
  }, []);

  const refreshDevices = useCallback(async () => {
    if (!navigator.mediaDevices?.enumerateDevices) return;
    const all = await navigator.mediaDevices.enumerateDevices();
    setMics(toDevices(all, 'audioinput', 'Microphone'));
    setCams(toDevices(all, 'videoinput', 'Camera'));
  }, []);

  // (Re)acquire the stream when the chosen devices or the camera toggle change.
  useEffect(() => {
    if (!supported) return undefined;
    let alive = true;
    void (async () => {
      setStatus('requesting');
      setMessage(null);
      cur.current?.getTracks().forEach((t) => t.stop());
      cur.current = null;
      const audio: MediaTrackConstraints | boolean = micId ? { deviceId: { exact: micId } } : true;
      const video: MediaTrackConstraints | boolean = camOn ? (camId ? { deviceId: { exact: camId } } : true) : false;
      let s: MediaStream | null = null;
      try {
        s = await navigator.mediaDevices.getUserMedia({ audio, video });
      } catch (e1) {
        let e: unknown = e1;
        // Camera missing or blocked: keep the microphone check working.
        if (video) {
          try { s = await navigator.mediaDevices.getUserMedia({ audio, video: false }); } catch (e2) { e = e2; }
        }
        if (!s) {
          if (!alive) return;
          const name = e instanceof DOMException ? e.name : '';
          setStream(null);
          setStatus(name === 'NotAllowedError' || name === 'SecurityError' ? 'denied' : 'error');
          setMessage(name === 'NotAllowedError' || name === 'SecurityError'
            ? 'Camera and microphone access is blocked. Allow it in your browser settings, then try again.'
            : 'No camera or microphone could be opened. You can still join and listen.');
          void refreshDevices().catch(() => undefined);
          return;
        }
      }
      if (!alive) { s.getTracks().forEach((t) => t.stop()); return; }
      cur.current = s;
      setStream(s);
      setStatus('ready');
      void refreshDevices().catch(() => undefined);
    })();
    return () => { alive = false; };
  }, [supported, micId, camId, camOn, attempt, refreshDevices]);

  useEffect(() => {
    if (!supported) return undefined;
    const md = navigator.mediaDevices;
    const on = () => { void refreshDevices().catch(() => undefined); };
    md.addEventListener?.('devicechange', on);
    return () => md.removeEventListener?.('devicechange', on);
  }, [supported, refreshDevices]);

  useEffect(() => () => { cur.current?.getTracks().forEach((t) => t.stop()); }, []);

  // Level meter.
  const hasAudio = !!stream && stream.getAudioTracks().length > 0;
  useEffect(() => {
    if (!stream || !hasAudio) { setLevel(0); return undefined; }
    const Ctx = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Ctx) return undefined;
    const ctx = new Ctx();
    const src = ctx.createMediaStreamSource(new MediaStream(stream.getAudioTracks()));
    const an = ctx.createAnalyser();
    an.fftSize = 512;
    src.connect(an);
    const buf = new Uint8Array(an.fftSize);
    let raf = 0;
    let smooth = 0;
    const tick = () => {
      an.getByteTimeDomainData(buf);
      let peak = 0;
      for (const v of buf) peak = Math.max(peak, Math.abs(v - 128) / 128);
      smooth = Math.max(peak, smooth * 0.85);
      setLevel(Math.min(1, smooth * 1.6));
      raf = requestAnimationFrame(tick);
    };
    void ctx.resume().catch(() => undefined);
    tick();
    return () => { cancelAnimationFrame(raf); src.disconnect(); void ctx.close().catch(() => undefined); };
  }, [stream, hasAudio]);

  const retry = useCallback(() => setAttempt((n) => n + 1), []);
  return {
    status, message, stream, hasVideo: !!stream && stream.getVideoTracks().length > 0, hasAudio, mics, cams, micId, camId, setMicId, setCamId, camOn, setCamOn, level, retry, stop,
  };
}
