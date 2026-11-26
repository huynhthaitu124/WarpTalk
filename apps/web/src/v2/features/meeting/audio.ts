// Which remote audio tracks to subscribe to (contract B-02).
//  - listen "original": every plain microphone track, no dub.
//  - listen "{lang}": the dubbed track `dub-{lang}`, plus the ORIGINAL microphone of every speaker whose speak
//    language is already {lang} (the pipeline makes no dub for them); the original of everyone else stays muted.
import { baseIdentity } from '@v2/lib/captions';

export interface PubLike { kind: string; trackName: string; source?: string; setSubscribed(v: boolean): void }
export interface RemoteLike { identity: string; trackPublications: { values(): Iterable<PubLike> } }

export const isDub = (trackName: string) => trackName.startsWith('dub-');
/** Identities of secondary connections (`{userId}:listener`) never publish media the user should hear. */
export const isSecondaryIdentity = (identity: string) => /:(listener|bridge-listener)$/.test(identity);

export function wantAudio(pub: Pick<PubLike, 'trackName' | 'source'>, identity: string, listen: string, speakLang: ReadonlyMap<string, string>): boolean {
  if (pub.source === 'screen_share_audio') return true;
  const dub = isDub(pub.trackName);
  if (listen === 'original') return !dub;
  if (dub) return pub.trackName === `dub-${listen}`;
  return speakLang.get(baseIdentity(identity)) === listen;
}

/** `speakLang`: LiveKit identity (userId) -> that participant's speakLanguage, from the roster. */
export function applyAudioSubscriptions(remotes: Iterable<RemoteLike>, listen: string, speakLang: ReadonlyMap<string, string>): void {
  for (const p of remotes) {
    for (const pub of p.trackPublications.values()) {
      if (pub.kind === 'audio') pub.setSubscribed(!isSecondaryIdentity(p.identity) && wantAudio(pub, p.identity, listen, speakLang));
      else pub.setSubscribed(true);
    }
  }
}
