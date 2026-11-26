-- Speaker-aware summaries and cross-meeting memory.
-- Ids from other modules (user, series) stay plain uuid columns: no cross-schema foreign keys.

-- The user an action item is assigned to (the host when the owner is the host), so clients can
-- list "my" items without matching free-text owner names. NULL = not resolved to a user.
ALTER TABLE assistant.action_items ADD COLUMN IF NOT EXISTS assignee_user_id uuid;
CREATE INDEX IF NOT EXISTS ix_action_items_assignee
    ON assistant.action_items (assignee_user_id, status, created_at DESC)
    WHERE assignee_user_id IS NOT NULL;

-- Room facts stamped on the summary when the job runs, so the meeting memory can select
-- "previous meetings of this host / series" from assistant's own tables only.
ALTER TABLE assistant.meeting_summaries ADD COLUMN IF NOT EXISTS host_user_id uuid;
ALTER TABLE assistant.meeting_summaries ADD COLUMN IF NOT EXISTS series_id uuid;
ALTER TABLE assistant.meeting_summaries ADD COLUMN IF NOT EXISTS title text;
CREATE INDEX IF NOT EXISTS ix_meeting_summaries_host
    ON assistant.meeting_summaries (workspace_id, host_user_id, completed_at DESC)
    WHERE status = 'completed';
