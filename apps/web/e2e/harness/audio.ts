// Drives the real subscription logic (features/meeting/audio.ts) against fake LiveKit participants.
import { applyAudioSubscriptions, type PubLike } from '@/features/meeting/audio';

export interface FakePub { name: string; kind?: 'audio' | 'video'; source?: string }
export interface FakeRemote { identity: string; pubs: FakePub[] }

/** Returns the subscribed track names per identity after applying the rules. */
export function run(listen: string, remotes: FakeRemote[], roster: Record<string, string>): Record<string, string[]> {
  const state = new Map<string, Map<string, boolean>>();
  const fake = remotes.map((r) => {
    const m = new Map<string, boolean>();
    state.set(r.identity, m);
    const pubs: PubLike[] = r.pubs.map((p) => ({
      kind: p.kind ?? 'audio', trackName: p.name, source: p.source,
      setSubscribed: (v: boolean) => { m.set(p.name, v); },
    }));
    return { identity: r.identity, trackPublications: { values: () => pubs } };
  });
  applyAudioSubscriptions(fake, listen, new Map(Object.entries(roster)));
  const out: Record<string, string[]> = {};
  for (const [id, m] of state) out[id] = [...m].filter(([, v]) => v).map(([k]) => k);
  return out;
}
