import { ApiError } from '@v2/lib/api';

export function errMsg(e: unknown): string {
  if (e instanceof ApiError) return e.message;
  if (e instanceof Error) return e.message;
  return 'Something went wrong';
}
export function errCode(e: unknown): string | undefined {
  return e instanceof ApiError ? e.code : undefined;
}
