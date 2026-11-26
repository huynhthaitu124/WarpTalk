-- WarpBot conversations (05a "Assistant"). One conversation belongs to one user in one workspace.
CREATE TABLE assistant.assistant_conversations (
    id              uuid         PRIMARY KEY,
    workspace_id    uuid         NOT NULL,
    user_id         uuid         NOT NULL,
    title           varchar(255) NOT NULL DEFAULT 'New chat',
    context_scope   text,
    created_at      timestamptz  NOT NULL,
    last_message_at timestamptz,
    is_archived     boolean      NOT NULL DEFAULT false
);

CREATE INDEX ix_assistant_conversations_owner
    ON assistant.assistant_conversations (workspace_id, user_id, is_archived, last_message_at DESC);

-- user_id is null on the assistant's own replies.
-- tool_results_json on an assistant reply holds the sources the answer cited (JSON array of
-- {marker, kind, title, ref}); the tool calls themselves are rows of assistant_tool_calls.
CREATE TABLE assistant.assistant_messages (
    id                uuid        PRIMARY KEY,
    conversation_id   uuid        NOT NULL
        REFERENCES assistant.assistant_conversations (id) ON DELETE CASCADE,
    workspace_id      uuid        NOT NULL,
    user_id           uuid,
    role              varchar(20) NOT NULL CHECK (role IN ('user', 'assistant', 'system', 'tool')),
    content           text        NOT NULL DEFAULT '',
    tool_calls_json   text,
    tool_results_json text,
    status            varchar(20) NOT NULL DEFAULT 'completed'
        CHECK (status IN ('pending', 'streaming', 'completed', 'failed')),
    created_at        timestamptz NOT NULL,
    completed_at      timestamptz
);

CREATE INDEX ix_assistant_messages_conversation
    ON assistant.assistant_messages (workspace_id, conversation_id, created_at);

CREATE TABLE assistant.assistant_tool_calls (
    id             uuid         PRIMARY KEY,
    message_id     uuid         NOT NULL
        REFERENCES assistant.assistant_messages (id) ON DELETE CASCADE,
    tool_name      varchar(100) NOT NULL,
    arguments_json text         NOT NULL,
    status         varchar(20)  NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'completed', 'failed')),
    result_json    text,
    created_at     timestamptz  NOT NULL,
    completed_at   timestamptz
);

CREATE INDEX ix_assistant_tool_calls_message ON assistant.assistant_tool_calls (message_id);
