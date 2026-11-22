import { useState, type FormEvent } from 'react';
import { useSearchParams } from 'react-router-dom';
import { ArrowClockwise, Crown, DotsThree, EnvelopeSimple, ProhibitInset, ShieldCheck, UserMinus, UserPlus, Users } from '@phosphor-icons/react';
import {
  Alert, Avatar, Badge, Button, Card, CardBody, CardHeader, Dialog, DialogContent, Dropdown, DropdownContent, DropdownItem, DropdownSeparator, DropdownTrigger,
  EmptyState, Field, Input, PageHeader, Select, Skeleton, StatusPill, Table, TBody, TD, TH, THead, TR, useToast,
} from '@/ui';
import { useAuth } from '@/lib/auth';
import { canAdmin, useWorkspace } from '@/lib/workspace';
import { useChangeRole, useInvitations, useInvite, useMembers, useRemoveMember, useResendInvitation, useRevokeInvitation, useTransferOwnership } from '@/lib/queries';
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

type Action =
  | { kind: 'role'; member: Member; role: 'ADMIN' | 'MEMBER' }
  | { kind: 'remove'; member: Member }
  | { kind: 'transfer'; member: Member };

const nameOf = (m: Member) => m.fullName || m.email || 'this member';

/** What the caller may do to `target` (mirrors the server rules: owner changes roles/transfers; admins remove plain members). */
export function memberActions(me: Workspace['myRole'], target: Member, selfId: string | null): ('ADMIN' | 'MEMBER' | 'transfer' | 'remove')[] {
  if (target.userId === selfId || target.role === 'OWNER') return [];
  if (me === 'OWNER') return [target.role === 'ADMIN' ? 'MEMBER' : 'ADMIN', 'transfer', 'remove'];
  if (me === 'ADMIN' && target.role === 'MEMBER') return ['remove'];
  return [];
}

function MemberMenu({ member, ws, selfId, onAction }: { member: Member; ws: Workspace; selfId: string | null; onAction(a: Action): void }) {
  const acts = memberActions(ws.myRole, member, selfId);
  if (acts.length === 0) return null;
  return (
    <Dropdown>
      <DropdownTrigger asChild>
        <Button variant="ghost" size="icon" aria-label={`Actions for ${nameOf(member)}`}><DotsThree size={18} weight="bold" aria-hidden /></Button>
      </DropdownTrigger>
      <DropdownContent align="end">
        {acts.includes('ADMIN') && <DropdownItem icon={<ShieldCheck size={16} aria-hidden />} onSelect={() => onAction({ kind: 'role', member, role: 'ADMIN' })}>Make admin</DropdownItem>}
        {acts.includes('MEMBER') && <DropdownItem icon={<Users size={16} aria-hidden />} onSelect={() => onAction({ kind: 'role', member, role: 'MEMBER' })}>Make member</DropdownItem>}
        {acts.includes('transfer') && <DropdownItem icon={<Crown size={16} aria-hidden />} onSelect={() => onAction({ kind: 'transfer', member })}>Transfer ownership…</DropdownItem>}
        {acts.includes('remove') && <><DropdownSeparator /><DropdownItem danger icon={<UserMinus size={16} aria-hidden />} onSelect={() => onAction({ kind: 'remove', member })}>Remove from workspace</DropdownItem></>}
      </DropdownContent>
    </Dropdown>
  );
}

function ConfirmMember({ ws, action, onClose }: { ws: Workspace; action: Action | null; onClose(): void }) {
  const change = useChangeRole(ws.id);
  const remove = useRemoveMember(ws.id);
  const transfer = useTransferOwnership(ws.id);
  const toast = useToast();
  const busy = change.isPending || remove.isPending || transfer.isPending;
  const error = change.error ?? remove.error ?? transfer.error;
  const close = () => { change.reset(); remove.reset(); transfer.reset(); onClose(); };
  if (!action) return <Dialog open={false} onOpenChange={() => undefined} />;
  const n = nameOf(action.member);
  const copy = action.kind === 'remove'
    ? { title: `Remove ${n}?`, body: `${n} loses access to ${ws.name}, its meetings and glossaries. They can be invited again later.`, cta: 'Remove member', danger: true }
    : action.kind === 'transfer'
      ? { title: `Transfer ownership to ${n}?`, body: `${n} becomes the owner of ${ws.name}. You stay as an admin and can no longer change roles or transfer ownership.`, cta: 'Transfer ownership', danger: true }
      : { title: `Make ${n} ${action.role === 'ADMIN' ? 'an admin' : 'a member'}?`, body: action.role === 'ADMIN' ? 'Admins can invite members, manage glossaries and settings.' : 'Members can join meetings and read glossaries, but cannot change settings.', cta: action.role === 'ADMIN' ? 'Make admin' : 'Make member', danger: false };
  async function run() {
    if (!action) return;
    try {
      if (action.kind === 'remove') await remove.mutateAsync(action.member.userId);
      else if (action.kind === 'transfer') await transfer.mutateAsync(action.member.userId);
      else await change.mutateAsync({ userId: action.member.userId, role: action.role });
      toast({ tone: 'success', title: action.kind === 'remove' ? 'Member removed' : action.kind === 'transfer' ? 'Ownership transferred' : 'Role updated', description: n });
      close();
    } catch { /* shown inline */ }
  }
  return (
    <Dialog open onOpenChange={(v) => { if (!v) close(); }}>
      <DialogContent
        title={copy.title}
        description={copy.body}
        footer={<><Button onClick={close}>Cancel</Button><Button variant={copy.danger ? 'danger' : 'primary'} loading={busy} onClick={() => void run()}>{copy.cta}</Button></>}
      >
        {error && <Alert tone="danger">{errMsg(error)}</Alert>}
      </DialogContent>
    </Dialog>
  );
}

function MembersCard({ ws }: { ws: Workspace }) {
  const q = useMembers(ws.id);
  const { userId } = useAuth();
  const [action, setAction] = useState<Action | null>(null);
  const items: Member[] = q.data?.items ?? [];
  const manage = ws.myRole === 'OWNER' || ws.myRole === 'ADMIN';
  return (
    <Card>
      <CardHeader title={`Members${q.data ? ` (${q.data.total})` : ''}`} description="People who can use this workspace." />
      {q.isLoading ? <CardBody><Skeleton className="h-24" /></CardBody> : q.isError ? <CardBody><Alert tone="danger">{errMsg(q.error)}</Alert></CardBody> : (
        <>
          <div className="max-sm:hidden">
            <Table bare aria-label="Workspace members">
              <THead><tr><TH>Member</TH><TH>Email</TH><TH>Role</TH><TH>Joined</TH>{manage && <TH className="w-12"><span className="sr-only">Actions</span></TH>}</tr></THead>
              <TBody>
                {items.map((m) => (
                  <TR key={m.userId} data-testid="member-row" data-user-id={m.userId}>
                    <TD><span className="flex items-center gap-2"><Avatar name={m.fullName || m.email || 'Member'} size="sm" /><span className="max-w-56 truncate" title={m.fullName ?? undefined}>{m.fullName || '—'}{m.userId === userId ? ' (you)' : ''}</span></span></TD>
                    <TD className="max-w-64 truncate text-ink-muted" title={m.email ?? undefined}>{m.email ?? '—'}</TD>
                    <TD><StatusPill status={m.role} /></TD>
                    <TD className="whitespace-nowrap text-ink-subtle">{new Date(m.joinedAt).toLocaleDateString()}</TD>
                    {manage && <TD className="text-right"><MemberMenu member={m} ws={ws} selfId={userId} onAction={setAction} /></TD>}
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
                  <p className="truncate text-body font-medium" title={m.fullName ?? undefined}>{m.fullName || '—'}{m.userId === userId ? ' (you)' : ''}</p>
                  <p className="truncate text-caption text-ink-subtle" title={m.email ?? undefined}>{m.email ?? '—'}</p>
                </div>
                <StatusPill status={m.role} />
                {manage && <MemberMenu member={m} ws={ws} selfId={userId} onAction={setAction} />}
              </li>
            ))}
          </ul>
        </>
      )}
      <ConfirmMember ws={ws} action={action} onClose={() => setAction(null)} />
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
