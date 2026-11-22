// Glossary REST hooks (workspace glossaries + the system-admin global glossary).
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import { api } from './api';
import { globalAudit, globalTerm, glossary, glossaryTerm, importResult, type GlobalStatus } from './schemas';
import { session } from './session';
import { apiUrl } from '@/lib/env';

// ---- workspace glossaries
export function useGlossaries(wsId: string | undefined) {
  return useQuery({
    queryKey: ['glossaries', wsId],
    enabled: !!wsId,
    queryFn: () => api(`/glossaries/workspace/${wsId}`, { schema: z.array(glossary) }),
  });
}
export function useGlossary(id: string | undefined) {
  return useQuery({ queryKey: ['glossary', id], enabled: !!id, retry: false, queryFn: () => api(`/glossaries/${id}`, { schema: glossary }) });
}
export interface GlossaryInput { name: string; description?: string; sourceLanguage: string; targetLanguage: string }
export function useCreateGlossary(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: GlossaryInput) => api('/glossaries', { method: 'POST', body: { ...b, workspaceId: wsId }, schema: glossary }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['glossaries', wsId] }),
  });
}
export function useUpdateGlossary(id: string, wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: Partial<GlossaryInput>) => api(`/glossaries/${id}`, { method: 'PUT', body: b, schema: glossary }),
    onSuccess: (g) => {
      qc.setQueryData(['glossary', id], g);
      void qc.invalidateQueries({ queryKey: ['glossaries', wsId] });
    },
  });
}
export function useDeleteGlossary(wsId: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (id: string) => api(`/glossaries/${id}`, { method: 'DELETE' }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['glossaries', wsId] }),
  });
}

// ---- terms
export interface TermInput { sourceTerm: string; targetTerm: string; caseSensitive: boolean; notes?: string | null }
export function useTerms(glossaryId: string | undefined) {
  return useQuery({
    queryKey: ['glossary-terms', glossaryId],
    enabled: !!glossaryId,
    retry: false,
    queryFn: () => api(`/glossaries/${glossaryId}/terms`, { schema: z.array(glossaryTerm) }),
  });
}
/** Term writes change `termCount` on the glossary, so both caches are refreshed. */
function useTermInvalidate(glossaryId: string) {
  const qc = useQueryClient();
  return () => {
    void qc.invalidateQueries({ queryKey: ['glossary-terms', glossaryId] });
    void qc.invalidateQueries({ queryKey: ['glossary', glossaryId] });
    void qc.invalidateQueries({ queryKey: ['glossaries'] });
  };
}
export function useAddTerm(glossaryId: string) {
  const done = useTermInvalidate(glossaryId);
  return useMutation({
    mutationFn: (b: TermInput) => api(`/glossaries/${glossaryId}/terms`, { method: 'POST', body: b, schema: glossaryTerm }),
    onSuccess: done,
  });
}
export function useUpdateTerm(glossaryId: string) {
  const done = useTermInvalidate(glossaryId);
  return useMutation({
    mutationFn: (v: { id: string } & TermInput) => api(`/glossaries/${glossaryId}/terms/${v.id}`, { method: 'PUT', body: { sourceTerm: v.sourceTerm, targetTerm: v.targetTerm, caseSensitive: v.caseSensitive, notes: v.notes }, schema: glossaryTerm }),
    onSuccess: done,
  });
}
export function useDeleteTerm(glossaryId: string) {
  const done = useTermInvalidate(glossaryId);
  return useMutation({
    mutationFn: (id: string) => api(`/glossaries/${glossaryId}/terms/${id}`, { method: 'DELETE' }),
    onSuccess: done,
  });
}
/** CSV text (`source,target` per line, optional header) posted as `text/csv`. */
export function useImportTerms(glossaryId: string) {
  const done = useTermInvalidate(glossaryId);
  return useMutation({
    mutationFn: async (csv: string) => {
      // The shared `api()` helper always sends JSON; the endpoint also accepts `{ csv }`.
      return api(`/glossaries/${glossaryId}/terms/bulk`, { method: 'POST', body: { csv }, schema: importResult });
    },
    onSuccess: done,
  });
}
/** Authenticated download (a plain link cannot carry the bearer token). Returns the CSV text. */
export async function exportGlossaryCsv(glossaryId: string): Promise<string> {
  const get = (token: string | null) => fetch(apiUrl(`/glossaries/${glossaryId}/export`), { headers: { authorization: `Bearer ${token ?? ''}` } });
  let res = await get(session.accessToken);
  if (res.status === 401 && session.refreshToken) res = await get((await session.refresh())?.accessToken ?? null);
  if (!res.ok) throw new Error(`Export failed (${res.status})`);
  return res.text();
}

// ---- global glossary (system admins)
export function useGlobalTerms(status: GlobalStatus | 'all', enabled: boolean) {
  return useQuery({
    queryKey: ['global-glossary', status],
    enabled,
    retry: false,
    queryFn: () => api('/admin/global-glossary', {
      query: { status: status === 'all' ? undefined : status, pageSize: 200 },
      schema: z.object({ items: z.array(globalTerm), total: z.number(), page: z.number(), pageSize: z.number() }),
    }),
  });
}
export interface GlobalInput {
  term: string; preferredTranslation: string; sourceLanguage?: string | null; targetLanguage?: string | null; businessDomain?: string | null;
  definition?: string | null; usageNote?: string | null; priority?: number;
}
export function useGlobalSave(id?: string) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (b: GlobalInput) => id
      ? api(`/admin/global-glossary/${id}`, { method: 'PUT', body: b, schema: globalTerm })
      : api('/admin/global-glossary', { method: 'POST', body: b, schema: globalTerm }),
    onSuccess: () => qc.invalidateQueries({ queryKey: ['global-glossary'] }),
  });
}
export function useGlobalAction() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: async (v: { id: string; action: 'publish' | 'archive' | 'delete' }) => {
      if (v.action === 'delete') await api(`/admin/global-glossary/${v.id}`, { method: 'DELETE' });
      else await api(`/admin/global-glossary/${v.id}/${v.action}`, { method: 'POST', schema: globalTerm });
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ['global-glossary'] }),
  });
}
export function useGlobalAudits(id: string | undefined) {
  return useQuery({
    queryKey: ['global-glossary-audits', id],
    enabled: !!id,
    retry: false,
    queryFn: () => api(`/admin/global-glossary/${id}/audits`, { schema: z.array(globalAudit) }),
  });
}
