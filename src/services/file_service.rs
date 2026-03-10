use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};
use sqlx::SqlitePool;
use tracing::{error, info};
use uuid::Uuid;

use crate::config::StorageBackend;
use crate::db;
use crate::file_compressor::FileCompressor;
use crate::models::file_model::{ FileRecord, NewFileRecord, UploadResult };
use crate::services::media_transcode_service::MediaTranscodeService;
use crate::services::r2_storage_service::R2StorageService;

#[derive(Clone)]
pub struct FileService {
    db: SqlitePool,
    compressor: FileCompressor,
    media_transcoder: MediaTranscodeService,
    storage_backend: StorageBackend,
    r2_storage: Option<R2StorageService>,
    upload_dir: PathBuf,
}

impl FileService {
    pub fn new(
        db: SqlitePool,
        compressor: FileCompressor,
        media_transcoder: MediaTranscodeService,
        storage_backend: StorageBackend,
        r2_storage: Option<R2StorageService>,
        upload_dir: PathBuf,
    ) -> Self {
        Self {
            db,
            compressor,
            media_transcoder,
            storage_backend,
            r2_storage,
            upload_dir,
        }
    }

    pub async fn upload_and_compress_from_path(
        &self,
        file_name: Option<&str>,
        input_path: &Path,
        uploaded_by: i64,
        folder_id: Option<i64>,
    ) -> Result<UploadResult> {
        let original_name = file_name
            .map(sanitize_filename)
            .unwrap_or_else(|| "upload.bin".to_string());
        let ext = file_extension(&original_name);

        let original_size = tokio::fs::metadata(input_path).await?.len() as i64;
        let stored_filename = format!("{}_{}", Uuid::new_v4(), original_name);
        let stored_path = self.upload_dir.join(&stored_filename);
        let stored_path_str = stored_path.to_string_lossy().to_string();

        move_file_to_destination(input_path, &stored_path).await?;

        let mut compressed_path_str = stored_path_str.clone();
        let mut compressed_size = original_size;
        let mut is_compressed = false;

        if let Some(transcoded_size) = self
            .media_transcoder
            .transcode_if_smaller(&stored_path, &ext)
            .await?
        {
            compressed_size = transcoded_size as i64;
        }

        if should_try_zstd(&ext) {
            let candidate_compressed_path = self.upload_dir.join(format!("{}.zst", stored_filename));
            let candidate_compressed_path_str = candidate_compressed_path.to_string_lossy().to_string();

            let compressor = self.compressor.clone();
            let input_path = stored_path_str.clone();
            let output_path = candidate_compressed_path_str.clone();

            tokio::task::spawn_blocking(move ||
                compressor.compress_file(&input_path, &output_path)
            ).await??;

            let candidate_size = tokio::fs::metadata(&candidate_compressed_path).await?.len() as i64;

            if candidate_size < compressed_size {
                compressed_path_str = candidate_compressed_path_str;
                compressed_size = candidate_size;
                is_compressed = true;

                if self.storage_backend == StorageBackend::Local {
                    let cleanup_service = self.clone();
                    let cleanup_path = stored_path_str.clone();
                    tokio::spawn(async move {
                        if let Err(e) = cleanup_service.delete_by_path(&cleanup_path).await {
                            error!(file = %cleanup_path, ?e, "background delete failed");
                        }
                    });
                }
            } else {
                self.delete_by_path(&candidate_compressed_path_str).await?;
                info!(
                    file = %original_name,
                    baseline_size = compressed_size,
                    candidate_size,
                    "kept file because zstd output was not smaller"
                );
            }
        } else {
            info!(file = %original_name, "skipping zstd for already-compressed file type");
        }

        let mut db_stored_path = stored_path_str.clone();
        let mut db_compressed_path = compressed_path_str.clone();

        if self.storage_backend == StorageBackend::R2 {
            let r2_storage = self.r2_required()?;
            let stored_key = r2_storage.object_key_for_filename(&stored_filename);

            if is_compressed {
                let compressed_name = format!("{stored_filename}.zst");
                let compressed_key = r2_storage.object_key_for_filename(&compressed_name);
                let compressed_local_path = PathBuf::from(&compressed_path_str);
                r2_storage.upload_path(&compressed_key, &compressed_local_path).await?;

                db_compressed_path = r2_storage.db_path_for_key(&compressed_key);
                // compressed object becomes the source of truth in object storage mode.
                db_stored_path = db_compressed_path.clone();
            } else {
                r2_storage.upload_path(&stored_key, &stored_path).await?;
                db_stored_path = r2_storage.db_path_for_key(&stored_key);
                db_compressed_path = db_stored_path.clone();
            }

            self.delete_by_path(&stored_path_str).await?;
            if compressed_path_str != stored_path_str {
                self.delete_by_path(&compressed_path_str).await?;
            }
        }

        let file_id = db::insert_file(
            &self.db,
            &(NewFileRecord {
                original_name: &original_name,
                folder_id,
                stored_path: &db_stored_path,
                compressed_path: &db_compressed_path,
                is_compressed,
                original_size,
                compressed_size,
                uploaded_by,
            })
        ).await?;

        Ok(UploadResult {
            id: file_id,
            original_name,
            folder_id,
            stored_path: db_stored_path,
            compressed_path: db_compressed_path,
            is_compressed,
            original_size,
            compressed_size,
        })
    }

    pub async fn list_files_for_user(
        &self,
        user_id: i64,
        folder_id: Option<i64>,
    ) -> Result<Vec<FileRecord>> {
        let files = db::list_files_for_user_and_folder(&self.db, user_id, folder_id).await?;
        Ok(files)
    }

    pub async fn get_file_by_id(&self, file_id: i64) -> Result<Option<FileRecord>> {
        let file = db::get_file_by_id(&self.db, file_id).await?;
        Ok(file)
    }

    pub async fn view_file_bytes(&self, file_id: i64) -> Result<Option<(String, Vec<u8>)>> {
        let Some(file) = self.get_file_by_id(file_id).await? else {
            return Ok(None);
        };

        let file_name = file.original_name.clone();
        let bytes = if file.is_compressed {
            if R2StorageService::is_r2_path(&file.compressed_path) {
                let r2_storage = self.r2_required()?;
                let compressed_bytes = r2_storage.get_object_bytes(&file.compressed_path).await?;
                let compressor = self.compressor.clone();
                tokio::task::spawn_blocking(move || compressor.decompress_bytes(&compressed_bytes))
                    .await??
            } else {
                let compressed_path = file.compressed_path.clone();
                let compressor = self.compressor.clone();

                tokio::task::spawn_blocking(move || compressor.decompress_file_to_bytes(&compressed_path))
                    .await??
            }
        } else if R2StorageService::is_r2_path(&file.stored_path) {
            let r2_storage = self.r2_required()?;
            r2_storage.get_object_bytes(&file.stored_path).await?
        } else {
            tokio::fs::read(&file.stored_path).await?
        };

        Ok(Some((file_name, bytes)))
    }

    pub async fn delete_file_by_id(&self, file_id: i64, user_id: i64) -> Result<bool> {
        let Some(file) = self.get_file_by_id(file_id).await? else {
            return Ok(false);
        };
        if file.uploaded_by != user_id {
            return Ok(false);
        }

        // upload_jobs.file_id references files.id, so clear those links first.
        db::clear_upload_job_file_refs(&self.db, file_id).await?;
        let deleted = db::delete_file_by_id_for_user(&self.db, file_id, user_id).await?;
        if !deleted {
            return Ok(false);
        }

        if let Err(e) = self.delete_by_path(&file.stored_path).await {
            error!(file = %file.stored_path, ?e, "failed to delete stored file after DB delete");
        }
        if file.compressed_path != file.stored_path {
            if let Err(e) = self.delete_by_path(&file.compressed_path).await {
                error!(file = %file.compressed_path, ?e, "failed to delete compressed file after DB delete");
            }
        }

        Ok(deleted)
    }

    async fn delete_by_path(&self, file_path: &str) -> Result<()> {
        if R2StorageService::is_r2_path(file_path) {
            let r2_storage = self.r2_required()?;
            match r2_storage.delete_object(file_path).await {
                Ok(()) => {
                    info!(file = %file_path, "R2 object deleted");
                    return Ok(());
                }
                Err(e) => {
                    error!(file = %file_path, ?e, "failed to delete R2 object");
                    return Err(e);
                }
            }
        }

        match tokio::fs::remove_file(file_path).await {
            Ok(()) => {
                info!(file = %file_path, "file deleted");
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                error!(file = %file_path, ?e, "failed to delete file");
                Err(e.into())
            }
        }
    }

    fn r2_required(&self) -> Result<&R2StorageService> {
        self.r2_storage
            .as_ref()
            .ok_or_else(|| anyhow!("R2 storage is not configured"))
    }
}

fn sanitize_filename(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|ch| {
            match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => ch,
                _ => '_',
            }
        })
        .collect();

    if sanitized.is_empty() {
        "upload.bin".to_string()
    } else {
        sanitized
    }
}

fn file_extension(file_name: &str) -> String {
    file_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

async fn move_file_to_destination(source: &Path, destination: &Path) -> Result<()> {
    if source == destination {
        return Ok(());
    }

    match tokio::fs::rename(source, destination).await {
        Ok(()) => Ok(()),
        Err(rename_error) => {
            tokio::fs::copy(source, destination).await.map_err(|copy_error| {
                anyhow!(
                    "failed to move file from {} to {} (rename: {}; copy: {})",
                    source.display(),
                    destination.display(),
                    rename_error,
                    copy_error
                )
            })?;
            tokio::fs::remove_file(source).await?;
            Ok(())
        }
    }
}

fn should_try_zstd(ext: &str) -> bool {
    !matches!(
        ext,
        "jpg"
            | "jpeg"
            | "png"
            | "gif"
            | "webp"
            | "bmp"
            | "svg"
            | "heic"
            | "avif"
            | "mp4"
            | "webm"
            | "mov"
            | "avi"
            | "mkv"
            | "mp3"
            | "wav"
            | "flac"
            | "ogg"
            | "m4a"
            | "zip"
            | "rar"
            | "7z"
            | "gz"
            | "bz2"
            | "xz"
            | "zst"
            | "pdf"
    )
}
