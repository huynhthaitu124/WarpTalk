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
  method?: 'GET' | 'POST' | 'PUT' | 'DELETE';
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
