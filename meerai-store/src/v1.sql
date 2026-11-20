CREATE TABLE meta (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL
);

CREATE TABLE vault_secrets (
    provider_id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL,            -- 'api' | 'oauth'
    nonce BLOB NOT NULL,           -- 12-byte ChaCha20Poly1305 nonce
    ciphertext BLOB NOT NULL,      -- AEAD encrypted JSON payload (AAD = provider_id)
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE user_config (
    key TEXT PRIMARY KEY NOT NULL,          -- 'settings', 'ui_tabs', 'trusted_projects'
    json_value TEXT NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE model_routes (
    task_type TEXT PRIMARY KEY NOT NULL,    -- 'default','explore','plan','build','review','compact','recovery','memory'
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    effort TEXT,
    max_tokens_per_turn INTEGER,
    max_cost_usd REAL,
    max_concurrency INTEGER,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE sessions (
    id TEXT PRIMARY KEY NOT NULL,
    project_root TEXT NOT NULL,
    cwd TEXT NOT NULL,
    title TEXT,
    model TEXT,
    approval_mode TEXT,
    git_branch TEXT,
    git_sha TEXT,
    forked_from TEXT,
    inherited_event_count INTEGER,
    parent_session_id TEXT,
    harness TEXT,
    remote_session_id TEXT,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE session_events (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq INTEGER NOT NULL,
    ts_ms INTEGER NOT NULL,
    event_type TEXT NOT NULL,
    turn_id TEXT,
    payload_json TEXT NOT NULL,
    PRIMARY KEY (session_id, seq)
);

CREATE VIRTUAL TABLE session_events_fts USING fts5(
    session_id UNINDEXED,
    seq UNINDEXED,
    text_content
);

CREATE TABLE session_blobs (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    blob_key TEXT NOT NULL,
    byte_len INTEGER NOT NULL,
    content_zstd BLOB NOT NULL,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, blob_key)
);

CREATE TABLE session_snapshots (
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    turn_id TEXT NOT NULL,
    rel_path TEXT NOT NULL,
    existed INTEGER NOT NULL,
    content_zstd BLOB,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (session_id, turn_id, rel_path)
);

CREATE TABLE project_memories (
    id TEXT PRIMARY KEY NOT NULL,
    project_root TEXT NOT NULL,
    kind TEXT NOT NULL,            -- 'decision' | 'progress' | 'fact' | 'convention'
    title TEXT NOT NULL,
    body TEXT NOT NULL,
    source_session_id TEXT,
    pinned INTEGER NOT NULL DEFAULT 0,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE VIRTUAL TABLE project_memories_fts USING fts5(
    id UNINDEXED,
    project_root UNINDEXED,
    title,
    body
);

CREATE TABLE installed_packs (
    id TEXT PRIMARY KEY NOT NULL,
    source_kind TEXT NOT NULL,     -- 'git' | 'local'
    source_uri TEXT NOT NULL,
    pinned_rev TEXT NOT NULL,      -- git commit SHA or version tag
    manifest_json TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    trusted INTEGER NOT NULL DEFAULT 0,
    installed_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE usage_ledger (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    parent_session_id TEXT,
    turn_id TEXT,
    agent_id TEXT,
    task_type TEXT,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt_tokens INTEGER NOT NULL,
    completion_tokens INTEGER NOT NULL,
    cached_tokens INTEGER NOT NULL,
    est_cost_usd REAL,
    ts_ms INTEGER NOT NULL
);

CREATE INDEX sessions_by_project_updated ON sessions(project_root, updated_at_ms DESC);
CREATE INDEX sessions_by_updated ON sessions(updated_at_ms DESC);
CREATE INDEX sessions_by_parent ON sessions(parent_session_id);
CREATE INDEX session_events_by_turn ON session_events(session_id, turn_id, seq);
CREATE INDEX project_memories_by_project ON project_memories(project_root, pinned DESC, updated_at_ms DESC);
CREATE INDEX usage_ledger_by_session_time ON usage_ledger(session_id, ts_ms);
CREATE INDEX usage_ledger_by_parent_time ON usage_ledger(parent_session_id, ts_ms);

CREATE TRIGGER session_events_fts_insert AFTER INSERT ON session_events BEGIN
    INSERT INTO session_events_fts(session_id, seq, text_content)
    VALUES (new.session_id, new.seq, (
        SELECT group_concat(atom, ' ' ORDER BY id)
        FROM json_tree(CASE WHEN json_valid(new.payload_json) THEN new.payload_json
                            ELSE json_quote(new.payload_json) END)
        WHERE type IN ('text', 'integer', 'real')
    ));
END;

CREATE TRIGGER session_events_fts_update AFTER UPDATE ON session_events BEGIN
    DELETE FROM session_events_fts WHERE session_id = old.session_id AND seq = old.seq;
    INSERT INTO session_events_fts(session_id, seq, text_content)
    VALUES (new.session_id, new.seq, (
        SELECT group_concat(atom, ' ' ORDER BY id)
        FROM json_tree(CASE WHEN json_valid(new.payload_json) THEN new.payload_json
                            ELSE json_quote(new.payload_json) END)
        WHERE type IN ('text', 'integer', 'real')
    ));
END;

CREATE TRIGGER session_events_fts_delete AFTER DELETE ON session_events BEGIN
    DELETE FROM session_events_fts WHERE session_id = old.session_id AND seq = old.seq;
END;

CREATE TRIGGER project_memories_fts_insert AFTER INSERT ON project_memories BEGIN
    INSERT INTO project_memories_fts(id, project_root, title, body)
    VALUES (new.id, new.project_root, new.title, new.body);
END;

CREATE TRIGGER project_memories_fts_update AFTER UPDATE ON project_memories BEGIN
    DELETE FROM project_memories_fts WHERE id = old.id;
    INSERT INTO project_memories_fts(id, project_root, title, body)
    VALUES (new.id, new.project_root, new.title, new.body);
END;

CREATE TRIGGER project_memories_fts_delete AFTER DELETE ON project_memories BEGIN
    DELETE FROM project_memories_fts WHERE id = old.id;
END;
