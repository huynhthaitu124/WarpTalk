// TEST-ONLY database helpers.
// Email verification no longer touches Postgres: fixtures read the raw token from the dev mail
// capture in Redis (see api.ts `verifyEmail`). What is left here is for state with no public API:
//  - `expireBridgeLease`: age the capturer lease (the real lease is 45s).
//  - `addWorkspaceMember`: DEPRECATED direct insert kept for specs that have not moved to
//    `inviteAndAccept` (api.ts), which uses the real invitation endpoints.
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
const MEMBER_ROLE = '00000000-0000-7000-8000-000000000003';
export function addWorkspaceMember(workspaceId: string, userId: string) {
  psql(
    `INSERT INTO workspace.workspace_members (id, workspace_id, user_id, role_id, joined_at) VALUES ('${randomUUID()}', '${assertUuid(workspaceId)}', '${assertUuid(userId)}', '${MEMBER_ROLE}', now())`,
  );
}

/** Pretend the capturer's last heartbeat was long ago (the real lease is 45s; tests must not wait). TEST-ONLY. */
export function expireBridgeLease(roomId: string, secondsAgo = 600) {
  const n = psql(
    `UPDATE translation_room.translation_rooms SET bridge_capturer_lease_at = now() - interval '${Math.floor(secondsAgo)} seconds' WHERE id = '${assertUuid(roomId)}' RETURNING 1`,
  );
  if (n !== '1') throw new Error(`bridge room not found: ${roomId}`);
}
