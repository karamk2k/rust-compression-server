PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS upload_jobs (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    original_name TEXT NOT NULL,
    temp_path TEXT NOT NULL,
    uploaded_by INTEGER NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    file_id INTEGER,
    error TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    started_at TEXT,
    finished_at TEXT,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY(uploaded_by) REFERENCES users(id),
    FOREIGN KEY(file_id) REFERENCES files(id)
);

CREATE INDEX IF NOT EXISTS idx_upload_jobs_status_created_at
    ON upload_jobs(status, created_at);

CREATE INDEX IF NOT EXISTS idx_upload_jobs_uploaded_by
    ON upload_jobs(uploaded_by);
