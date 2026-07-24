CREATE TABLE live_streams (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    public_id TEXT NOT NULL UNIQUE,
    owner_id INTEGER NOT NULL,
    title TEXT NOT NULL,
    publish_key_hash TEXT NOT NULL,
    enabled INTEGER NOT NULL DEFAULT 1,
    is_live INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (owner_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE INDEX idx_live_streams_owner ON live_streams(owner_id, id DESC);

CREATE TABLE live_viewer_keys (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    stream_id INTEGER NOT NULL,
    label TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    expires_at TEXT,
    revoked_at TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (stream_id) REFERENCES live_streams(id) ON DELETE CASCADE
);

CREATE INDEX idx_live_viewer_keys_stream ON live_viewer_keys(stream_id, id DESC);
