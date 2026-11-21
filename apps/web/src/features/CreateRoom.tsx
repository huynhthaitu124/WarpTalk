import { useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router-dom';
import { Alert, Button, Checkbox, Dialog, DialogContent, Field, Input, Select, useToast } from '@/ui';
import { useProfile } from '@/lib/auth';
import { useWorkspace } from '@/lib/workspace';
import { useCreateRoom } from '@/lib/queries';
import { LANGUAGES } from '@/lib/languages';
import { errMsg } from './errors';

export function CreateRoomDialog({ open, onOpenChange }: { open: boolean; onOpenChange(v: boolean): void }) {
  const { current } = useWorkspace();
  const create = useCreateRoom();
  const toast = useToast();
  const nav = useNavigate();
  const profile = useProfile();
  const [title, setTitle] = useState('');
  const [source, setSource] = useState('vi');
  const [targets, setTargets] = useState<string[]>(['en']);
  const [approval, setApproval] = useState(false);
  const [tried, setTried] = useState(false);

  const toggle = (code: string, on: boolean) => setTargets((t) => (on ? [...new Set([...t, code])] : t.filter((c) => c !== code)));
  const titleError = tried && !title.trim() ? 'Give the meeting a title.' : undefined;
  const targetError = tried && targets.length === 0 ? 'Add at least one target language.' : undefined;

  async function submit(e: FormEvent) {
    e.preventDefault();
    setTried(true);
    if (!current || !title.trim() || targets.length === 0) return;
    try {
      const r = await create.mutateAsync({
        workspaceId: current.id, title: title.trim(), sourceLanguage: source, targetLanguages: targets, requiresApproval: approval,
        displayName: profile.data?.fullName,
      });
      toast({ tone: 'success', title: 'Meeting created', description: r.title });
      setTitle('');
      setTried(false);
      onOpenChange(false);
      nav(`/rooms/${r.id}`);
    } catch { /* inline */ }
  }
  return (
    <Dialog open={open} onOpenChange={(v) => { if (!v) setTried(false); onOpenChange(v); }}>
      <DialogContent
        title="Create meeting"
        description="A translation meeting with live captions and dubbed audio."
        footer={
          <>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button variant="primary" type="submit" form="create-room-form" loading={create.isPending}>Create meeting</Button>
          </>
        }
      >
        <form id="create-room-form" onSubmit={submit} className="flex flex-col gap-4" noValidate>
          <Field label="Title" required error={titleError}><Input maxLength={200} value={title} onChange={(e) => setTitle(e.target.value)} autoFocus aria-required="true" /></Field>
          <Field label="Source language">
            <Select value={source} onValueChange={(v) => { setSource(v); setTargets((t) => t.filter((c) => c !== v)); }} options={LANGUAGES.map((l) => ({ value: l.code, label: l.label }))} />
          </Field>
          <fieldset className="flex flex-col gap-2" aria-describedby={targetError ? 'targets-error' : undefined}>
            <legend className="mb-1.5 text-body-sm font-medium text-ink-muted">Target languages<span aria-hidden className="text-danger-text"> *</span></legend>
            {LANGUAGES.filter((l) => l.code !== source).map((l) => (
              <Checkbox key={l.code} checked={targets.includes(l.code)} onCheckedChange={(v) => toggle(l.code, v)} label={l.label} />
            ))}
            {targetError && <p id="targets-error" role="alert" className="text-caption text-danger-text">{targetError}</p>}
          </fieldset>
          <Checkbox checked={approval} onCheckedChange={setApproval} label="Requires approval" description="Participants wait in the lobby until the host admits them." />
          {create.isError && <Alert tone="danger">{errMsg(create.error)}</Alert>}
        </form>
      </DialogContent>
    </Dialog>
  );
}
