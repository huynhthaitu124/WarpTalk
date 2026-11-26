import { useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router-dom';
import { Alert, Button, Card, CardBody, Dialog, DialogContent, Field, Input, PageHeader, Select, StatusPill } from '@v2/ui';
import { useJoinByCode } from '@v2/lib/queries';
import { languageOptions } from '@v2/lib/languageOptions';
import { useWorkspace } from '@v2/lib/workspace';
import { errMsg } from './errors';

/** Join-by-code form shared by the `/join` page and the dialog opened from Meetings. */
export function JoinByCodeForm({ onJoined }: { onJoined?: () => void }) {
  const join = useJoinByCode();
  const nav = useNavigate();
  const [code, setCode] = useState('');
  const [name, setName] = useState('');
  const [lang, setLang] = useState('en');
  const opts = languageOptions();
  const valid = code.trim().length === 12;

  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      const p = await join.mutateAsync({ roomCode: code.trim(), displayName: name.trim() || undefined, speakLanguage: lang, listenLanguage: lang });
      if (p.roomId) { onJoined?.(); nav(`/rooms/${p.roomId}`); }
    } catch { /* inline */ }
  }
  const joined = join.data;
  if (joined) {
    return (
      <div className="flex flex-col gap-4" data-testid="join-result">
        <Alert tone={joined.status === 'WAITING' ? 'warning' : 'success'} title={joined.status === 'WAITING' ? 'Waiting for the host to admit you' : 'You joined the meeting'}>
          Joined as <strong>{joined.displayName}</strong>. Status: <StatusPill status={joined.status} />
        </Alert>
        {!joined.roomId && <p className="text-body-sm text-ink-subtle">Open the meeting link shared by the host to follow its progress.</p>}
        <Button onClick={() => join.reset()}>Join another meeting</Button>
      </div>
    );
  }
  return (
    <form onSubmit={submit} className="flex flex-col gap-4" noValidate>
      <Field label="Room code" hint="12 characters, letters and digits." error={code && !valid ? 'Room code must be exactly 12 characters.' : undefined}>
        <Input value={code} onChange={(e) => setCode(e.target.value)} autoComplete="off" spellCheck={false} className="font-mono" placeholder="e.g. abcdef123456" autoFocus />
      </Field>
      <Field label="Display name (optional)"><Input value={name} onChange={(e) => setName(e.target.value)} maxLength={100} /></Field>
      <Field label="My language" hint="You speak and hear this language. Everyone else hears you in theirs."><Select value={lang} onValueChange={setLang} options={opts} /></Field>
      {join.isError && <Alert tone="danger">{errMsg(join.error)}</Alert>}
      <Button type="submit" variant="primary" disabled={!valid} loading={join.isPending}>Join meeting</Button>
    </form>
  );
}

export function JoinDialog({ open, onOpenChange }: { open: boolean; onOpenChange(v: boolean): void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent title="Join a meeting" description="Enter the 12-character room code you received from the host.">
        <JoinByCodeForm onJoined={() => onOpenChange(false)} />
      </DialogContent>
    </Dialog>
  );
}

export function JoinByCodePage() {
  const { wsPath, current } = useWorkspace();
  return (
    <>
      <PageHeader title="Join by code" description="Enter the 12-character room code you received from the host." />
      <Card className="max-w-md">
        <CardBody>
          <JoinByCodeForm />
          {current && <p className="mt-4 text-body-sm text-ink-subtle">Have a link instead? Open it directly, or go to <Link to={wsPath('/meetings')}>your meetings</Link>.</p>}
        </CardBody>
      </Card>
    </>
  );
}
