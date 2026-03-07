use std::path::PathBuf;

use sqlx::SqlitePool;

use crate::config::StorageBackend;
use crate::file_compressor::FileCompressor;
use crate::services::media_transcode_service::MediaTranscodeService;
use crate::services::r2_storage_service::R2StorageService;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub compressor: FileCompressor,
    pub media_transcoder: MediaTranscodeService,
    pub storage_backend: StorageBackend,
    pub r2_storage: Option<R2StorageService>,
    pub upload_dir: PathBuf,
    pub log_file: PathBuf,
}
