// Feature flags (ADR 0001 §1): `GET /features?workspaceId=` -> camelCase booleans. Translation meetings, dubbing,
// voice cloning and the external-meeting bridge are hidden by default; the copilot is on. The code behind a hidden
// feature is kept; only entry points are gated.
import type { ReactNode } from 'react';
import { Navigate } from 'react-router-dom';
import { useQuery } from '@tanstack/react-query';
import { z } from 'zod';
import { api } from './api';
import { useWorkspaceOptional } from './workspace';
import { Skeleton } from '@/ui';

export const FEATURE_NAMES = ['translationMeetings', 'dubbing', 'voiceClone', 'externalBridge', 'copilot'] as const;
export type FeatureName = (typeof FEATURE_NAMES)[number];
export type Features = Record<FeatureName, boolean>;

/** Platform defaults; also used while loading and when the endpoint is missing (404) or fails. */
export const DEFAULT_FEATURES: Features = {
  translationMeetings: false, dubbing: false, voiceClone: false, externalBridge: false, copilot: true,
};

const featuresSchema = z.object({
  translationMeetings: z.boolean().optional(),
  dubbing: z.boolean().optional(),
  voiceClone: z.boolean().optional(),
  externalBridge: z.boolean().optional(),
  copilot: z.boolean().optional(),
});

/** Cached per workspace (`['features', workspaceId | 'platform']`). Never throws: errors fall back to the defaults. */
export function useFeatures(): { features: Features; ready: boolean } {
  const wsId = useWorkspaceOptional()?.current?.id;
  const q = useQuery({
    queryKey: ['features', wsId ?? 'platform'],
    staleTime: 5 * 60_000,
    retry: false,
    queryFn: async (): Promise<Features> => {
      try {
        const r = await api('/features', { query: { workspaceId: wsId }, schema: featuresSchema });
        return { ...DEFAULT_FEATURES, ...Object.fromEntries(Object.entries(r).filter(([, v]) => typeof v === 'boolean')) };
      } catch {
        return DEFAULT_FEATURES;
      }
    },
  });
  return { features: q.data ?? DEFAULT_FEATURES, ready: !q.isPending };
}

export function useFeature(name: FeatureName): boolean {
  return useFeatures().features[name];
}

/** Renders `children` only when the feature is on (`fallback` otherwise). */
export function Feature({ name, children, fallback = null }: { name: FeatureName; children: ReactNode; fallback?: ReactNode }) {
  const on = useFeature(name);
  return <>{on ? children : fallback}</>;
}

/** Route guard: waits for the flags, then redirects to Home (no error) when the feature is off. */
export function FeatureRoute({ name, children }: { name: FeatureName; children: ReactNode }) {
  const { features, ready } = useFeatures();
  const ws = useWorkspaceOptional();
  if (!ready || ws?.loading) return <div className="p-8"><Skeleton className="h-8 w-48" /></div>;
  if (!features[name]) return <Navigate to={ws?.current ? ws.wsPath('/home') : '/workspaces'} replace />;
  return <>{children}</>;
}
