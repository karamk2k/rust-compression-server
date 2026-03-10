use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use tokio::time::sleep;
use tracing::{error, info};

use crate::app_state::AppState;
use crate::db;
use crate::services::file_service::FileService;

pub async fn spawn(state: AppState, worker_count: usize) {
    let worker_count = worker_count.max(1);

    match db::reset_processing_upload_jobs(&state.db).await {
        Ok(reset_count) if reset_count > 0 => {
            info!(count = reset_count, "re-queued stale processing upload jobs");
        }
        Ok(_) => {}
        Err(error) => {
            error!(?error, "failed to re-queue stale processing upload jobs");
        }
    }

    info!(workers = worker_count, "starting upload job workers");

    for worker_id in 0..worker_count {
        let worker_state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = run(worker_state, worker_id).await {
                error!(worker_id, ?error, "upload job worker stopped");
            }
        });
    }
}

async fn run(state: AppState, worker_id: usize) -> Result<()> {
    loop {
        match process_one(&state, worker_id).await {
            Ok(true) => continue,
            Ok(false) => sleep(Duration::from_millis(250)).await,
            Err(error) => {
                error!(worker_id, ?error, "upload job worker iteration failed");
                sleep(Duration::from_secs(3)).await;
            }
        }
    }
}

async fn process_one(state: &AppState, worker_id: usize) -> Result<bool> {
    let Some(job) = db::claim_next_pending_upload_job(&state.db).await? else {
        return Ok(false);
    };

    info!(
        worker_id,
        job_id = job.id,
        file = %job.original_name,
        "processing upload job"
    );

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
                worker_id,
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
            error!(
                worker_id,
                job_id = job.id,
                file = %job.original_name,
                error = %message,
                "upload job failed"
            );
        }
    }

    Ok(true)
}
