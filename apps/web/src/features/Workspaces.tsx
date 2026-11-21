import { useState, type FormEvent } from 'react';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { Buildings, Plus } from '@phosphor-icons/react';
import {
  Alert, Button, Dialog, DialogContent, EmptyState, Field, Input, ListRow, PageHeader, Skeleton, StatusPill, Textarea, useToast, Avatar,
} from '@/ui';
import { useWorkspace } from '@/lib/workspace';
import { useCreateWorkspace } from '@/lib/queries';
import { PendingInvitations } from './common';
import { errMsg } from './errors';

export function CreateWorkspaceDialog({ open, onOpenChange }: { open: boolean; onOpenChange(v: boolean): void }) {
  const create = useCreateWorkspace();
  const { select } = useWorkspace();
  const toast = useToast();
  const nav = useNavigate();
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');

  async function submit(e: FormEvent) {
    e.preventDefault();
    try {
      const ws = await create.mutateAsync({ name: name.trim(), description: description.trim() || undefined });
      select(ws.id);
      toast({ tone: 'success', title: 'Workspace created', description: ws.name });
      setName('');
      setDescription('');
      onOpenChange(false);
      nav(`/${ws.slug}/home`);
    } catch { /* shown inline */ }
  }
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        title="Create workspace"
        description="Workspaces group rooms, members and settings."
        footer={
          <>
            <Button onClick={() => onOpenChange(false)}>Cancel</Button>
            <Button variant="primary" type="submit" form="create-workspace-form" loading={create.isPending} disabled={!name.trim()}>Create workspace</Button>
          </>
        }
      >
        <form id="create-workspace-form" onSubmit={submit} className="flex flex-col gap-4">
          <Field label="Name"><Input required maxLength={150} value={name} onChange={(e) => setName(e.target.value)} autoFocus /></Field>
          <Field label="Description (optional)"><Textarea value={description} onChange={(e) => setDescription(e.target.value)} /></Field>
          {create.isError && <Alert tone="danger">{errMsg(create.error)}</Alert>}
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function WorkspacesPage() {
  const { workspaces, current, select, loading } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const open = params.get('new') === '1';
  const setOpen = (v: boolean) => setParams(v ? { new: '1' } : {}, { replace: true });
  return (
    <>
      <PageHeader
        title="Workspaces"
        description="Switch between the workspaces you belong to."
        actions={<Button variant="primary" onClick={() => setOpen(true)}><Plus size={16} weight="bold" aria-hidden />Create workspace</Button>}
      />
      <PendingInvitations className="mb-6" />
      {loading ? (
        <Skeleton className="h-32" />
      ) : workspaces.length === 0 ? (
        <EmptyState icon={<Buildings size={20} weight="duotone" />} title="No workspaces yet" description="Create your first workspace to start hosting translated rooms." action={<Button variant="primary" onClick={() => setOpen(true)}>Create workspace</Button>} />
      ) : (
        <div className="overflow-hidden rounded-lg border border-hairline bg-surface-1" role="list" aria-label="Workspaces">
          {workspaces.map((w) => (
            <ListRow key={w.id} role="listitem" className="max-sm:flex-nowrap">
              <Avatar name={w.name} />
              <div className="min-w-0 flex-1">
                <p className="truncate text-body font-medium text-ink" title={w.name}>{w.name}</p>
                <p className="truncate text-caption text-ink-subtle">{w.description || w.slug}</p>
              </div>
              <StatusPill status={w.myRole} />
              {w.id === current?.id ? (
                <span className="w-20 text-right text-body-sm text-ink-subtle max-sm:w-auto">Current</span>
              ) : (
                <Button size="sm" className="w-20" onClick={() => select(w.id)} aria-label={`Select workspace ${w.name}`}>Select</Button>
              )}
            </ListRow>
          ))}
        </div>
      )}
      <CreateWorkspaceDialog open={open} onOpenChange={setOpen} />
    </>
  );
}
