PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS folders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    parent_id INTEGER,
    created_by INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY(parent_id) REFERENCES folders(id) ON DELETE CASCADE,
    FOREIGN KEY(created_by) REFERENCES users(id) ON DELETE CASCADE,
    UNIQUE(name, parent_id, created_by)
);

CREATE INDEX IF NOT EXISTS idx_folders_created_by_parent
    ON folders(created_by, parent_id, name);

CREATE UNIQUE INDEX IF NOT EXISTS idx_folders_unique_name_per_parent
    ON folders(created_by, COALESCE(parent_id, 0), name);

ALTER TABLE files ADD COLUMN folder_id INTEGER;
CREATE INDEX IF NOT EXISTS idx_files_uploaded_by_folder_created_at
    ON files(uploaded_by, folder_id, created_at DESC);

ALTER TABLE upload_jobs ADD COLUMN folder_id INTEGER;
CREATE INDEX IF NOT EXISTS idx_upload_jobs_uploaded_by_folder
    ON upload_jobs(uploaded_by, folder_id);
