// Generated file fixtures for the document and voice specs (nothing is committed as a binary).
import { call, API, ok } from './api';

/** Mono 16-bit PCM WAV. `kind: 'silent'` is digital silence, `'speech'` a synthetic voiced signal with syllable-like pauses. */
export function makeWav(o: { seconds: number; kind: 'silent' | 'speech'; rate?: number }): Buffer {
  const rate = o.rate ?? 24_000;
  const n = Math.round(o.seconds * rate);
  const data = Buffer.alloc(n * 2);
  for (let i = 0; i < n; i++) {
    let v = 0;
    if (o.kind === 'speech') {
      const t = i / rate;
      // ~3.5 syllables per second, 75% voiced; vowel-ish formants on a 130 Hz pitch with slow vibrato.
      const phase = (t * 3.5) % 1;
      const env = phase < 0.75 ? Math.sin((Math.PI * phase) / 0.75) ** 0.6 : 0;
      const f0 = 130 + 8 * Math.sin(2 * Math.PI * 5 * t);
      let s = 0;
      for (let h = 1; h <= 12; h++) s += Math.sin(2 * Math.PI * f0 * h * t) / h;
      v = 0.22 * env * s + 0.004 * Math.sin(2 * Math.PI * 3100 * t);
    }
    data.writeInt16LE(Math.max(-32767, Math.min(32767, Math.round(v * 32767))), i * 2);
  }
  const head = Buffer.alloc(44);
  head.write('RIFF', 0);
  head.writeUInt32LE(36 + data.length, 4);
  head.write('WAVEfmt ', 8);
  head.writeUInt32LE(16, 16);
  head.writeUInt16LE(1, 20);
  head.writeUInt16LE(1, 22);
  head.writeUInt32LE(rate, 24);
  head.writeUInt32LE(rate * 2, 28);
  head.writeUInt16LE(2, 32);
  head.writeUInt16LE(16, 34);
  head.write('data', 36);
  head.writeUInt32LE(data.length, 40);
  return Buffer.concat([head, data]);
}

export interface DocDto {
  id: string; title: string; fileName: string; fileExtension: string; category: string; accessLevel: string;
  aiRetrievalAllowed: boolean; ingestionStatus: string; ingestionFailureReason?: string | null; sizeBytes: number;
}

export interface DocUpload {
  name: string;
  content: Buffer | string;
  title?: string;
  category?: string;
  accessLevel?: string;
  ai?: boolean;
}

/** Raw multipart upload (returns status + body so rejections can be asserted). */
export async function postDocument(token: string, wsId: string, d: DocUpload) {
  const form = new FormData();
  if (d.title) form.set('title', d.title);
  if (d.category) form.set('category', d.category);
  if (d.accessLevel) form.set('accessLevel', d.accessLevel);
  if (d.ai !== undefined) form.set('aiRetrievalAllowed', String(d.ai));
  form.set('file', new Blob([typeof d.content === 'string' ? d.content : new Uint8Array(d.content)]), d.name);
  const res = await fetch(`${API}/workspaces/${wsId}/documents`, { method: 'POST', headers: { authorization: `Bearer ${token}` }, body: form });
  const text = await res.text();
  return { status: res.status, body: (text ? JSON.parse(text) : undefined) as DocDto & { code?: string; message?: string } };
}

export async function uploadDocument(token: string, wsId: string, d: DocUpload): Promise<DocDto> {
  const r = await postDocument(token, wsId, d);
  if (r.status >= 300) throw new Error(`upload ${r.status}: ${JSON.stringify(r.body)}`);
  return r.body;
}

export const listDocuments = (token: string, wsId: string, query = '') =>
  ok(call<{ items: DocDto[]; total: number }>(`/workspaces/${wsId}/documents?pageSize=100${query}`, { token }));
