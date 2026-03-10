mod app_state;
mod auth;
mod config;
mod db;
mod file_compressor;
mod folder_watcher;
mod logging;
mod models;
mod routes;
mod server;
mod services;

use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use app_state::AppState;
use config::{AppConfig, StorageBackend};
use db::run_migrations;
use file_compressor::FileCompressor;
use folder_watcher::FolderWatcher;
use logging::init_logging;
use server::Server;
use services::auth_service::AuthService;
use services::media_transcode_service::MediaTranscodeService;
use services::r2_storage_service::R2StorageService;
use services::upload_job_worker;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    let config = AppConfig::from_env();
    let _log_guard = init_logging(&config.log_level, &config.log_file)?;
    prepare_storage(&config)?;

    let compressor = FileCompressor::new(config.compression_level);
    let media_transcoder = MediaTranscodeService::new(
        config.enable_media_transcode,
        config.ffmpeg_bin.clone(),
        config.ffmpeg_video_crf,
        config.ffmpeg_video_preset.clone(),
        config.ffmpeg_jpeg_quality,
        config.ffmpeg_webp_quality,
    );
    let r2_storage = match config.storage_backend {
        StorageBackend::Local => None,
        StorageBackend::R2 => {
            let storage = R2StorageService::new(
                config.r2_endpoint.clone(),
                config.r2_bucket.clone(),
                config.r2_region.clone(),
                config.r2_access_key_id.clone(),
                config.r2_secret_access_key.clone(),
                config.r2_key_prefix.clone(),
            )
            .await?;
            info!(bucket = %config.r2_bucket, "R2 storage backend enabled");
            Some(storage)
        }
    };
    let connect_options = SqliteConnectOptions::from_str(&config.database_url)?.create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(connect_options)
        .await?;

    run_migrations(&pool).await?;
    let auth_service = AuthService::new(pool.clone());
    auth_service
        .ensure_admin_user(&config.admin_username, &config.admin_password)
        .await?;

    let state = AppState {
        db: pool,
        compressor: compressor.clone(),
        media_transcoder,
        storage_backend: config.storage_backend.clone(),
        r2_storage,
        upload_dir: config.upload_dir.clone().into(),
        log_file: config.log_file.clone().into(),
    };

    upload_job_worker::spawn(state.clone());

    let watcher = FolderWatcher::new(config.watch_folders.clone(), compressor.clone());
    std::thread::spawn(move || {
        if let Err(error) = watcher.watch() {
            error!(?error, "folder watcher failed");
        }
    });

    let server = Server::new(config.server_host, config.server_port, state);
    info!("application startup complete");
    server.run().await;
    Ok(())
}

fn prepare_storage(config: &AppConfig) -> std::io::Result<()> {
    std::fs::create_dir_all(&config.upload_dir)?;
    if let Some(parent) = Path::new(&config.log_file).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    if let Some(db_path) = sqlite_path_from_url(&config.database_url) {
        let db_path = Path::new(db_path);
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
    }

    Ok(())
}

fn sqlite_path_from_url(database_url: &str) -> Option<&str> {
    if database_url == "sqlite::memory:" {
        return None;
    }

    let path = if let Some(path) = database_url.strip_prefix("sqlite://") {
        path
    } else if let Some(path) = database_url.strip_prefix("sqlite:") {
        path
    } else {
        return None;
    };

    let path = path.split('?').next().unwrap_or(path);
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}
