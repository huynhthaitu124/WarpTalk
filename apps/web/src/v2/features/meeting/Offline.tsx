import { useMemo, useState } from 'react';
import { Plugs } from '@phosphor-icons/react';
import { Alert, Button, Card, EmptyState } from '@v2/ui';
import type { Participant, Room } from '@v2/lib/schemas';
import { chatUnavailable, SidePanel, type PanelTab, type PersonRow } from './SidePanel';

const CHAT_OFFLINE = chatUnavailable('Chat is available once you are connected to the meeting.');

/** Room info + (empty) transcript and live roster shown when no media session exists (LiveKit not configured). */
export function OfflineRoom({ room, roster, selfUserId, onRetry }: { room: Room; roster: Participant[]; selfUserId: string | null; onRetry(): void }) {
  const [tab, setTab] = useState<PanelTab>('people');
  const rows = useMemo<PersonRow[]>(() => roster.map((p) => ({ p, isSelf: !!p.userId && p.userId === selfUserId, host: !!p.userId && p.userId === room.effectiveHostId })), [roster, selfUserId, room.effectiveHostId]);
  return (
    <div className="mx-auto flex w-full max-w-4xl flex-col gap-4" data-testid="meeting-offline">
      <Alert tone="warning" title="LiveKit is not configured">
        <span className="flex items-start gap-2"><Plugs className="mt-0.5 size-4 shrink-0" aria-hidden /><span data-testid="livekit-not-configured">The server has no LiveKit credentials (LIVEKIT_URL, LIVEKIT_API_KEY, LIVEKIT_API_SECRET), so live audio and video are unavailable. You can still see who is in the room; captions and dubbed audio need a media server.</span></span>
        <Button size="sm" className="mt-3" onClick={onRetry}>Try again</Button>
      </Alert>
      <Card className="flex h-[30rem] max-h-[70dvh] flex-col overflow-hidden">
        <SidePanel
          className="min-h-0 flex-1"
          tab={tab}
          onTab={setTab}
          lines={[]}
          translationLang="original"
          transcriptPlaceholder="Captions are unavailable until you are connected to the meeting."
          rows={rows}
          canModerate={false}
          onAdmit={() => undefined}
          onDeny={() => undefined}
          onAdmitAll={() => undefined}
          moderating={false}
          chat={CHAT_OFFLINE}
          unread={0}
        />
      </Card>
    </div>
  );
}

export function EndedView({ title, body, onRejoin, onBack }: { title: string; body: string; onRejoin?: () => void; onBack(): void }) {
  return (
    <div className="mx-auto w-full max-w-md" data-testid="meeting-ended">
      <EmptyState
        title={title}
        description={body}
        action={(
          <div className="flex gap-2">
            {onRejoin && <Button variant="primary" onClick={onRejoin}>Rejoin</Button>}
            <Button onClick={onBack}>Back to room details</Button>
          </div>
        )}
      />
    </div>
  );
}
