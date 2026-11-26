import { Alert, Button, Dialog, DialogClose, DialogContent, useToast } from '@v2/ui';
import { useEndMeeting } from '@v2/lib/queries';
import { errMsg } from '../errors';

/** Legacy copy (meetingLive.exitControl). */
export function EndMeetingDialog({ roomId, open, onOpenChange, onEnded }: { roomId: string; open: boolean; onOpenChange(v: boolean): void; onEnded(): void }) {
  const endM = useEndMeeting(roomId);
  const toast = useToast();
  async function end() {
    try {
      await endM.mutateAsync();
      onOpenChange(false);
      toast({ tone: 'success', title: 'Meeting ended' });
      onEnded();
    } catch { /* shown inside the dialog */ }
  }
  return (
    <Dialog open={open} onOpenChange={(v) => { if (!endM.isPending) { onOpenChange(v); if (!v) endM.reset(); } }}>
      <DialogContent
        title="End meeting for everyone?"
        description="Everyone is disconnected and the room is closed. This cannot be undone."
        footer={(
          <>
            <DialogClose asChild><Button>Cancel</Button></DialogClose>
            <Button variant="danger" loading={endM.isPending} onClick={() => void end()}>End meeting</Button>
          </>
        )}
      >
        {endM.isError && <Alert tone="danger" title="Could not end the meeting">{errMsg(endM.error)}</Alert>}
      </DialogContent>
    </Dialog>
  );
}

export function LeaveDialog({ open, onOpenChange, onLeave, isHost }: { open: boolean; onOpenChange(v: boolean): void; onLeave(): void; isHost: boolean }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        title="Leave meeting?"
        description={isHost ? 'You are the host. The meeting will continue without an active host unless you end it for everyone.' : 'The meeting continues without you. You can rejoin while it is live.'}
        footer={(
          <>
            <DialogClose asChild><Button>Cancel</Button></DialogClose>
            <Button variant="danger" onClick={onLeave}>Leave meeting</Button>
          </>
        )}
      />
    </Dialog>
  );
}
