import { useState, type FormEvent } from 'react';
import { useSearchParams } from 'react-router-dom';
import { ArrowClockwise, EnvelopeSimple, ProhibitInset, UserPlus, Users } from '@phosphor-icons/react';
import {
  Alert, Avatar, Badge, Button, Card, CardBody, CardHeader, Dialog, DialogContent, EmptyState, Field, Input, PageHeader, Select, Skeleton, StatusPill,
  Table, TBody, TD, TH, THead, TR, useToast,
} from '@/ui';
import { canAdmin, useWorkspace } from '@/lib/workspace';
import { useInvitations, useInvite, useMembers, useResendInvitation, useRevokeInvitation } from '@/lib/queries';
import type { Invitation, Member, Workspace } from '@/lib/schemas';
import { PendingInvitations, formatWhen } from './common';
import { errMsg } from './errors';

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

function InviteDialog({ ws, open, onOpenChange }: { ws: Workspace; open: boolean; onOpenChange(v: boolean): void }) {
  const invite = useInvite(ws.id);
  const toast = useToast();
  const [email, setEmail] = useState('');
  const [role, setRole] = useState<'MEMBER' | 'ADMIN'>('MEMBER');
  const [tried, setTried] = useState(false);
  const emailError = tried && !EMAIL.test(email.trim()) ? 'Enter a valid email address.' : undefined;

  async function submit(e: FormEvent) {
    e.preventDefault();
    setTried(true);
    if (!EMAIL.test(email.trim())) return;
    try {
      await invite.mutateAsync({ email: email.trim(), role });
      toast({ tone: 'success', title: 'Invitation sent', description: email.trim() });
      setEmail(''); setTried(false); invite.reset();
      onOpenChange(false);
    } catch { /* inline */ }
  }
  const roles = [{ value: 'MEMBER', label: 'Member' }, ...(ws.myRole === 'OWNER' ? [{ value: 'ADMIN', label: 'Admin' }] : [])];
  return (
    <Dialog open={open} onOpenChange={(v) => { if (!v) { setTried(false); invite.reset(); } onOpenChange(v); }}>
      <DialogContent
        title="Invite to workspace"
        description="They get an email with a link to join. Invitations expire after a few days."
        footer={<><Button onClick={() => onOpenChange(false)}>Cancel</Button><Button variant="primary" type="submit" form="invite-form" loading={invite.isPending}>Send invitation</Button></>}
      >
        <form id="invite-form" onSubmit={submit} className="flex flex-col gap-4" noValidate>
          <Field label="Email" required error={emailError}><Input type="email" autoComplete="off" autoFocus value={email} onChange={(e) => setEmail(e.target.value)} /></Field>
          <Field label="Role" hint={ws.myRole === 'OWNER' ? undefined : 'Only the owner can invite admins.'}><Select value={role} onValueChange={(v) => setRole(v as 'MEMBER' | 'ADMIN')} options={roles} /></Field>
          {invite.isError && <Alert tone="danger">{errMsg(invite.error)}</Alert>}
        </form>
      </DialogContent>
    </Dialog>
  );
}

function MembersCard({ ws }: { ws: Workspace }) {
  const q = useMembers(ws.id);
  const items: Member[] = q.data?.items ?? [];
  return (
    <Card>
      <CardHeader title={`Members${q.data ? ` (${q.data.total})` : ''}`} description="People who can use this workspace." />
      {q.isLoading ? <CardBody><Skeleton className="h-24" /></CardBody> : q.isError ? <CardBody><Alert tone="danger">{errMsg(q.error)}</Alert></CardBody> : (
        <>
          <div className="max-sm:hidden">
            <Table bare aria-label="Workspace members">
              <THead><tr><TH>Member</TH><TH>Email</TH><TH>Role</TH><TH>Joined</TH></tr></THead>
              <TBody>
                {items.map((m) => (
                  <TR key={m.userId}>
                    <TD><span className="flex items-center gap-2"><Avatar name={m.fullName || m.email || 'Member'} size="sm" /><span className="max-w-56 truncate" title={m.fullName ?? undefined}>{m.fullName || '—'}</span></span></TD>
                    <TD className="max-w-64 truncate text-ink-muted" title={m.email ?? undefined}>{m.email ?? '—'}</TD>
                    <TD><StatusPill status={m.role} /></TD>
                    <TD className="whitespace-nowrap text-ink-subtle">{new Date(m.joinedAt).toLocaleDateString()}</TD>
                  </TR>
                ))}
              </TBody>
            </Table>
          </div>
          <ul className="divide-y divide-hairline sm:hidden" aria-label="Workspace members (list)">
            {items.map((m) => (
              <li key={m.userId} className="flex items-center gap-3 px-4 py-3">
                <Avatar name={m.fullName || m.email || 'Member'} />
                <div className="min-w-0 flex-1">
                  <p className="truncate text-body font-medium" title={m.fullName ?? undefined}>{m.fullName || '—'}</p>
                  <p className="truncate text-caption text-ink-subtle" title={m.email ?? undefined}>{m.email ?? '—'}</p>
                </div>
                <StatusPill status={m.role} />
              </li>
            ))}
          </ul>
        </>
      )}
    </Card>
  );
}

const INV_TONE: Record<string, 'info' | 'warning' | 'neutral' | 'success'> = { pending: 'info', expired: 'warning', revoked: 'neutral', accepted: 'success' };

function InvitationRow({ inv, wsId }: { inv: Invitation; wsId: string }) {
  const resend = useResendInvitation(wsId);
  const revoke = useRevokeInvitation(wsId);
  const toast = useToast();
  const open = inv.status === 'pending' || inv.status === 'expired';
  const run = async (kind: 'resend' | 'revoke') => {
    try {
      if (kind === 'resend') await resend.mutateAsync(inv.id); else await revoke.mutateAsync(inv.id);
      toast({ tone: 'success', title: kind === 'resend' ? 'Invitation re-sent' : 'Invitation revoked', description: inv.email });
    } catch (e) { toast({ tone: 'danger', title: kind === 'resend' ? 'Could not resend' : 'Could not revoke', description: errMsg(e) }); }
  };
  return (
    <li className="flex flex-col gap-2 px-4 py-3 sm:flex-row sm:items-center sm:gap-3" data-testid="invitation" data-email={inv.email}>
      <div className="min-w-0 flex-1">
        <p className="truncate text-body font-medium" title={inv.email}>{inv.email}</p>
        <p className="text-caption text-ink-subtle">
          Invited {formatWhen(inv.createdAt)}{inv.lastSentAt ? ` · sent ${inv.sentCount}x, last ${formatWhen(inv.lastSentAt)}` : ''}{inv.status === 'pending' ? ` · expires ${formatWhen(inv.expiresAt)}` : ''}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-2 sm:shrink-0">
        <Badge tone={INV_TONE[inv.status] ?? 'neutral'} data-status={inv.status}>{inv.status[0]!.toUpperCase() + inv.status.slice(1)}</Badge>
        <Badge>{inv.role[0] + inv.role.slice(1).toLowerCase()}</Badge>
        {inv.deliveryStatus === 'Failed' && <Badge tone="danger">Email failed</Badge>}
        {open && (
          <>
            <Button size="sm" loading={resend.isPending} onClick={() => void run('resend')} aria-label={`Resend invitation to ${inv.email}`}><ArrowClockwise size={14} aria-hidden />Resend</Button>
            <Button size="sm" variant="danger-soft" loading={revoke.isPending} onClick={() => void run('revoke')} aria-label={`Revoke invitation to ${inv.email}`}><ProhibitInset size={14} aria-hidden />Revoke</Button>
          </>
        )}
      </div>
    </li>
  );
}

function InvitationsCard({ ws, onInvite }: { ws: Workspace; onInvite(): void }) {
  const q = useInvitations(ws.id, true);
  const items = (q.data ?? []).filter((i) => i.status !== 'replaced');
  return (
    <Card className="mt-6">
      <CardHeader title={`Invitations${q.data ? ` (${items.length})` : ''}`} description="People who were invited but have not joined yet." actions={<Button size="sm" onClick={onInvite}><UserPlus size={14} aria-hidden />Invite</Button>} />
      {q.isLoading ? <CardBody><Skeleton className="h-16" /></CardBody> : q.isError ? <CardBody><Alert tone="danger">{errMsg(q.error)}</Alert></CardBody> : items.length === 0 ? (
        <CardBody><p className="flex items-center gap-2 text-body text-ink-subtle"><EnvelopeSimple size={16} aria-hidden />No pending invitations.</p></CardBody>
      ) : (
        <ul className="divide-y divide-hairline" aria-label="Invitations">{items.map((i) => <InvitationRow key={i.id} inv={i} wsId={ws.id} />)}</ul>
      )}
    </Card>
  );
}

export function MembersPage() {
  const { current, loading } = useWorkspace();
  const [params, setParams] = useSearchParams();
  const inviting = params.get('invite') === '1';
  const setInviting = (v: boolean) => { const p = new URLSearchParams(params); if (v) p.set('invite', '1'); else p.delete('invite'); setParams(p, { replace: true }); };
  if (loading) return <Skeleton className="h-40" />;
  if (!current) return (<><PageHeader title="Members" /><PendingInvitations className="mb-6" /><EmptyState icon={<Users size={20} weight="duotone" />} title="No workspace" description="Create or join a workspace to see its members." /></>);
  const admin = canAdmin(current);
  return (
    <>
      <PageHeader
        title="Members"
        description={`People in ${current.name}.`}
        actions={admin ? <Button variant="primary" onClick={() => setInviting(true)}><UserPlus size={16} aria-hidden />Invite people</Button> : undefined}
      />
      <PendingInvitations className="mb-6" />
      <MembersCard ws={current} />
      {admin ? <InvitationsCard ws={current} onInvite={() => setInviting(true)} /> : <Alert tone="info" className="mt-6">Only workspace owners and admins can invite people. Ask an admin to send an invitation.</Alert>}
      {admin && <InviteDialog ws={current} open={inviting} onOpenChange={setInviting} />}
    </>
  );
}
