import type { z } from 'zod';
import { errorBody } from './schemas';
import { session } from './session';

export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
    this.name = 'ApiError';
  }
}

type Query = Record<string, string | number | boolean | undefined | null>;

interface Opts<S extends z.ZodType | undefined> {
  method?: 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';
  body?: unknown;
  query?: Query;
  schema?: S;
  auth?: boolean;
}

function url(path: string, query?: Query) {
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(query ?? {})) if (v !== undefined && v !== null && v !== '') q.set(k, String(v));
  const s = q.toString();
  return `/api/v1${path}${s ? `?${s}` : ''}`;
}

async function send(path: string, o: Omit<Opts<undefined>, 'schema'>, token: string | null) {
  const headers: Record<string, string> = {};
  if (o.body !== undefined) headers['content-type'] = 'application/json';
  if (token) headers.authorization = `Bearer ${token}`;
  return fetch(url(path, o.query), {
    method: o.method ?? 'GET',
    headers,
    body: o.body === undefined ? undefined : JSON.stringify(o.body),
  });
}

async function toError(res: Response): Promise<ApiError> {
  let code = 'HTTP_ERROR';
  let message = `Request failed (${res.status})`;
  try {
    const parsed = errorBody.safeParse(await res.json());
    if (parsed.success) ({ code, message } = parsed.data);
  } catch { /* non-JSON body */ }
  return new ApiError(res.status, code, message);
}

export async function api<S extends z.ZodType | undefined = undefined>(
  path: string,
  o: Opts<S> = {},
): Promise<S extends z.ZodType ? z.infer<S> : void> {
  const useAuth = o.auth !== false;
  const sentGeneration = session.generation;
  let res = await send(path, o, useAuth ? session.accessToken : null);
  if (res.status === 401 && useAuth && session.refreshToken) {
    // If the session already rotated while this request was in flight, just retry with the
    // current token (compared by generation, not by string: tokens minted in the same second can be
    // identical); otherwise refresh (single-flight, so concurrent 401s share one).
    const token = session.generation !== sentGeneration ? session.accessToken : (await session.refresh())?.accessToken;
    if (token) res = await send(path, o, token);
  }
  if (!res.ok) throw await toError(res);
  if (!o.schema || res.status === 204) return undefined as never;
  const json: unknown = await res.json();
  return o.schema.parse(json) as never;
}

/**
 * Authenticated request returning the raw `Response` (file downloads). Same 401 -> refresh -> retry
 * behaviour as `api`; throws `ApiError` for non-2xx answers.
 */
export async function apiResponse(path: string, o: Omit<Opts<undefined>, 'schema' | 'auth'> = {}): Promise<Response> {
  const sentGeneration = session.generation;
  let res = await send(path, o, session.accessToken);
  if (res.status === 401 && session.refreshToken) {
    const token = session.generation !== sentGeneration ? session.accessToken : (await session.refresh())?.accessToken;
    if (token) res = await send(path, o, token);
  }
  if (!res.ok) throw await toError(res);
  return res;
}

/**
 * Multipart POST with upload progress (fetch cannot report it). Retries once after a token refresh.
 * The response body is validated by `schema`.
 */
export function apiUpload<S extends z.ZodType>(
  path: string,
  form: FormData,
  schema: S,
  onProgress?: (fraction: number) => void,
): Promise<z.infer<S>> {
  const attempt = (token: string | null) =>
    new Promise<{ status: number; text: string }>((resolve, reject) => {
      const xhr = new XMLHttpRequest();
      xhr.open('POST', url(path));
      if (token) xhr.setRequestHeader('authorization', `Bearer ${token}`);
      xhr.upload.onprogress = (e) => { if (e.lengthComputable && onProgress) onProgress(e.loaded / e.total); };
      xhr.onload = () => resolve({ status: xhr.status, text: xhr.responseText });
      xhr.onerror = () => reject(new ApiError(0, 'NETWORK_ERROR', 'Network error: the upload did not finish.'));
      xhr.onabort = () => reject(new ApiError(0, 'ABORTED', 'Upload cancelled.'));
      xhr.send(form);
    });
  return (async () => {
    const sentGeneration = session.generation;
    let r = await attempt(session.accessToken);
    if (r.status === 401 && session.refreshToken) {
      const token = session.generation !== sentGeneration ? session.accessToken : (await session.refresh())?.accessToken;
      if (token) r = await attempt(token);
    }
    if (r.status < 200 || r.status >= 300) {
      let code = 'HTTP_ERROR';
      let message = `Request failed (${r.status})`;
      try {
        const parsed = errorBody.safeParse(JSON.parse(r.text));
        if (parsed.success) ({ code, message } = parsed.data);
      } catch { /* non-JSON body (e.g. a proxy 413) */ }
      if (r.status === 413 && code === 'HTTP_ERROR') { code = 'FILE_TOO_LARGE'; message = 'File exceeds the size limit.'; }
      throw new ApiError(r.status, code, message);
    }
    return schema.parse(JSON.parse(r.text)) as z.infer<S>;
  })();
}
