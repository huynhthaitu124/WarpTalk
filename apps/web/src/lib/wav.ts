// Client-side audio helpers for voice enrollment: decode a recording to mono PCM16 WAV.

export const VOICE_SAMPLE_RATE = 24_000;

/** Mono 16-bit little-endian PCM WAV container. */
export function encodeWav(pcm: Float32Array, rate: number): Uint8Array {
  const dataLen = pcm.length * 2;
  const buf = new ArrayBuffer(44 + dataLen);
  const v = new DataView(buf);
  const ascii = (o: number, s: string) => { for (let i = 0; i < s.length; i++) v.setUint8(o + i, s.charCodeAt(i)); };
  ascii(0, 'RIFF');
  v.setUint32(4, 36 + dataLen, true);
  ascii(8, 'WAVE');
  ascii(12, 'fmt ');
  v.setUint32(16, 16, true);
  v.setUint16(20, 1, true); // PCM
  v.setUint16(22, 1, true); // mono
  v.setUint32(24, rate, true);
  v.setUint32(28, rate * 2, true);
  v.setUint16(32, 2, true);
  v.setUint16(34, 16, true);
  ascii(36, 'data');
  v.setUint32(40, dataLen, true);
  for (let i = 0; i < pcm.length; i++) {
    const s = Math.max(-1, Math.min(1, pcm[i]!));
    v.setInt16(44 + i * 2, Math.round(s < 0 ? s * 32768 : s * 32767), true);
  }
  return new Uint8Array(buf);
}

/**
 * Decodes any browser-playable audio (MediaRecorder output) and resamples it to mono at
 * `VOICE_SAMPLE_RATE` using an OfflineAudioContext, then wraps it as PCM16 WAV.
 */
export async function recordingToWav(blob: Blob): Promise<{ wav: Uint8Array; seconds: number }> {
  const data = await blob.arrayBuffer();
  const Ctx = window.AudioContext ?? (window as unknown as { webkitAudioContext: typeof AudioContext }).webkitAudioContext;
  const ctx = new Ctx();
  try {
    const decoded = await ctx.decodeAudioData(data);
    const frames = Math.max(1, Math.ceil(decoded.duration * VOICE_SAMPLE_RATE));
    const off = new OfflineAudioContext(1, frames, VOICE_SAMPLE_RATE);
    const src = off.createBufferSource();
    src.buffer = decoded;
    src.connect(off.destination);
    src.start();
    const rendered = await off.startRendering();
    const pcm = rendered.getChannelData(0);
    return { wav: encodeWav(pcm, VOICE_SAMPLE_RATE), seconds: pcm.length / VOICE_SAMPLE_RATE };
  } finally {
    void ctx.close();
  }
}

export function toBase64(bytes: Uint8Array): string {
  let s = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) s += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return btoa(s);
}

/** Reads the header of a WAV file: seconds, or null when it is not a RIFF/WAVE PCM file. */
export function wavInfo(bytes: Uint8Array): { seconds: number; rate: number } | null {
  if (bytes.length < 44) return null;
  const v = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const tag = (o: number) => String.fromCharCode(v.getUint8(o), v.getUint8(o + 1), v.getUint8(o + 2), v.getUint8(o + 3));
  if (tag(0) !== 'RIFF' || tag(8) !== 'WAVE') return null;
  let pos = 12;
  let rate = 0;
  let block = 0;
  while (pos + 8 <= bytes.length) {
    const id = tag(pos);
    const len = v.getUint32(pos + 4, true);
    if (id === 'fmt ' && pos + 8 + 16 <= bytes.length) {
      rate = v.getUint32(pos + 12, true);
      block = v.getUint16(pos + 20, true);
    } else if (id === 'data') {
      if (!rate || !block) return null;
      const size = Math.min(len, bytes.length - pos - 8);
      return { seconds: size / block / rate, rate };
    }
    pos += 8 + len + (len & 1);
  }
  return null;
}

export async function sha256Hex(text: string): Promise<string> {
  const d = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text));
  return Array.from(new Uint8Array(d), (b) => b.toString(16).padStart(2, '0')).join('');
}
