// TEST-ONLY database helpers.
// Email verification no longer touches Postgres: fixtures read the raw token from the dev mail
// capture in Redis (see api.ts `verifyEmail`). What is left here is for state with no public API:
//  - `expireBridgeLease`: age the capturer lease (the real lease is 45s).
//  - `seedMeetingRow`: the `meeting.meeting_rooms` row that `POST /meetings/{id}/join` creates after it has asked
//    LiveKit for a room. The e2e backend has no LiveKit, so chat (REST only) needs the row seeded.
//  - `makeSystemAdmin`: grants the SYSTEM_ADMIN role (no public API; the platform operator does this by hand).
//  Workspace membership uses the real invitation endpoints (`inviteAndAccept` in api.ts), never psql.
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

/** TEST-ONLY: an ACTIVE current meeting row for a translation room, as a LiveKit-backed join would leave it. */
export function seedMeetingRow(roomId: string, hostId: string) {
  psql(
    `INSERT INTO meeting.meeting_rooms (id, translation_room_id, provider_room_name, status, is_current, is_active, created_at, created_by) VALUES ('${randomUUID()}', '${assertUuid(roomId)}', '${assertUuid(roomId)}', 'ACTIVE', true, true, now(), '${assertUuid(hostId)}') ON CONFLICT DO NOTHING`,
  );
}

/** TEST-ONLY: what the chat translation worker would store (it needs AI provider keys, absent in e2e). */
export function seedChatTranslation(messageId: string, lang: string, text: string) {
  const q = (v: string) => `'${v.replace(/'/g, "''")}'`;
  psql(
    `INSERT INTO meeting.meeting_chat_translations (id, message_id, meeting_room_id, source_language, target_language, translated_text, created_at) SELECT '${randomUUID()}', m.id, m.meeting_room_id, 'und', ${q(lang)}, ${q(text)}, now() FROM meeting.meeting_chat_messages m WHERE m.id = '${assertUuid(messageId)}'`,
  );
}

/** TEST-ONLY: grant SYSTEM_ADMIN. The user must log in again to get the `sa` claim. */
export function makeSystemAdmin(userId: string) {
  psql(
    `INSERT INTO auth.user_roles (id, user_id, role_id, assigned_at) SELECT gen_random_uuid(), '${assertUuid(userId)}', id, now() FROM auth.roles WHERE name = 'SYSTEM_ADMIN'`,
  );
}

/** Pretend the capturer's last heartbeat was long ago (the real lease is 45s; tests must not wait). TEST-ONLY. */
export function expireBridgeLease(roomId: string, secondsAgo = 600) {
  const n = psql(
    `UPDATE translation_room.translation_rooms SET bridge_capturer_lease_at = now() - interval '${Math.floor(secondsAgo)} seconds' WHERE id = '${assertUuid(roomId)}' RETURNING 1`,
  );
  if (n !== '1') throw new Error(`bridge room not found: ${roomId}`);
}

const q = (v: string) => `'${v.replace(/'/g, "''")}'`;

/** TEST-ONLY: an action item as the summary job leaves it (the job needs an LLM, absent in e2e). */
export function seedActionItem(o: { workspaceId: string; roomId: string; task: string; assigneeUserId?: string; status?: 'open' | 'done'; owner?: string; due?: string }): string {
  const id = randomUUID();
  psql(
    `INSERT INTO assistant.action_items (id, workspace_id, room_id, summary_id, task, owner, due, status, assignee_user_id) VALUES ('${id}', '${assertUuid(o.workspaceId)}', '${assertUuid(o.roomId)}', '${randomUUID()}', ${q(o.task)}, ${o.owner ? q(o.owner) : 'NULL'}, ${o.due ? q(o.due) : 'NULL'}, '${o.status ?? 'open'}', ${o.assigneeUserId ? `'${assertUuid(o.assigneeUserId)}'` : 'NULL'})`,
  );
  return id;
}

/** TEST-ONLY: a completed summary version of a room (what the summary job stores), optionally with `previousItems`. */
export function seedSummary(o: { workspaceId: string; roomId: string; short: string; decisions?: string[]; previousItems?: { task: string; status: 'done' | 'open' }[]; completedAt?: string }) {
  const content = JSON.stringify({
    summaryShort: o.short, summaryDetailed: '', decisions: o.decisions ?? [], actionItems: [], openQuestions: [], topics: [], language: 'en',
    previousItems: (o.previousItems ?? []).map((p) => ({ ...p, sourceSegmentIds: [] })), meta: {},
  });
  psql(
    `INSERT INTO assistant.meeting_summaries (id, workspace_id, room_id, version, trigger, status, content, completed_at, created_at) VALUES ('${randomUUID()}', '${assertUuid(o.workspaceId)}', '${assertUuid(o.roomId)}', 1, 'manual', 'completed', ${q(content)}::jsonb, ${o.completedAt ? `${q(o.completedAt)}::timestamptz` : 'now()'}, ${o.completedAt ? `${q(o.completedAt)}::timestamptz` : 'now()'})`,
  );
}

/** TEST-ONLY: a published live suggestion snapshot (rows normally come from the suggestion pipeline, which needs an LLM). */
export function seedSuggestion(o: { workspaceId: string; roomId: string; category: string; content?: string }): string {
  const id = randomUUID();
  psql(
    `INSERT INTO assistant.suggestions (id, workspace_id, room_id, segment_id, category, content) VALUES ('${id}', '${assertUuid(o.workspaceId)}', '${assertUuid(o.roomId)}', 'seg-${id.slice(0, 8)}', ${q(o.category)}, ${q(o.content ?? 'Suggested text')})`,
  );
  return id;
}
