ALTER TABLE files ADD COLUMN thumbnail_path TEXT;

CREATE INDEX IF NOT EXISTS idx_files_missing_thumbnails
    ON files(id)
    WHERE thumbnail_path IS NULL;
