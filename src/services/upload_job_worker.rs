use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use tokio::time::sleep;
use tracing::{error, info};

use crate::app_state::AppState;
use crate::db;
use crate::services::file_service::FileService;

pub fn spawn(state: AppState) {
    tokio::spawn(async move {
        if let Err(error) = run(state).await {
            error!(?error, "upload job worker stopped");
        }
    });
}

async fn run(state: AppState) -> Result<()> {
    let reset_count = db::reset_processing_upload_jobs(&state.db).await?;
    if reset_count > 0 {
        info!(count = reset_count, "re-queued stale processing upload jobs");
    }

    loop {
        match process_one(&state).await {
            Ok(true) => continue,
            Ok(false) => sleep(Duration::from_secs(1)).await,
            Err(error) => {
                error!(?error, "upload job worker iteration failed");
                sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

async fn process_one(state: &AppState) -> Result<bool> {
    let Some(job) = db::claim_next_pending_upload_job(&state.db).await? else {
        return Ok(false);
    };

    info!(job_id = job.id, file = %job.original_name, "processing upload job");

    let file_service = FileService::new(
        state.db.clone(),
        state.compressor.clone(),
        state.media_transcoder.clone(),
        state.storage_backend.clone(),
        state.r2_storage.clone(),
        state.upload_dir.clone(),
    );

    let process_result = file_service
        .upload_and_compress_from_path(
            Some(&job.original_name),
            Path::new(&job.temp_path),
            job.uploaded_by,
            job.folder_id,
        )
        .await;

    match process_result {
        Ok(upload_result) => {
            db::mark_upload_job_done(&state.db, job.id, upload_result.id).await?;
            info!(
                job_id = job.id,
                file_id = upload_result.id,
                file = %job.original_name,
                "upload job completed"
            );
        }
        Err(error) => {
            let message = format!("{:#}", error);
            let _ = tokio::fs::remove_file(&job.temp_path).await;
            db::mark_upload_job_failed(&state.db, job.id, &message).await?;
            error!(job_id = job.id, file = %job.original_name, error = %message, "upload job failed");
        }
    }

    Ok(true)
}
