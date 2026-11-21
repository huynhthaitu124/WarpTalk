// TEST-ONLY database helpers.
// The API never returns or e-mails the verification token (it logs the token length only), so the
// E2E suite marks users as verified straight in Postgres. This bypass exists ONLY in e2e fixtures;
// the application code has no such path. It also seeds MEMBER rows because the backend has no
// invite-member endpoint yet.
import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';

const PG = {
  host: process.env.PGHOST ?? 'localhost',
  user: process.env.PGUSER ?? 'postgres',
  db: process.env.PGDATABASE ?? 'warptalk',
};

function psql(sql: string): string {
  return execFileSync('psql', ['-h', PG.host, '-U', PG.user, '-d', PG.db, '-q', '-v', 'ON_ERROR_STOP=1', '-tAc', sql], {
    encoding: 'utf8',
  }).trim();
}

const assertUuid = (v: string) => {
  if (!/^[0-9a-f-]{36}$/i.test(v)) throw new Error(`not a uuid: ${v}`);
  return v;
};
const assertEmail = (v: string) => {
  if (!/^[\w.+-]+@[\w.-]+$/.test(v)) throw new Error(`unsafe email: ${v}`);
  return v;
};

export function markEmailVerified(email: string) {
  const n = psql(
    `UPDATE auth.users SET email_verified = true, email_verified_at = now() WHERE email = '${assertEmail(email.toLowerCase())}' RETURNING 1`,
  );
  if (n !== '1') throw new Error(`user not found for verification: ${email}`);
}

const MEMBER_ROLE = '00000000-0000-7000-8000-000000000003';
export function addWorkspaceMember(workspaceId: string, userId: string) {
  psql(
    `INSERT INTO workspace.workspace_members (id, workspace_id, user_id, role_id, joined_at) VALUES ('${randomUUID()}', '${assertUuid(workspaceId)}', '${assertUuid(userId)}', '${MEMBER_ROLE}', now())`,
  );
}
