use std::collections::HashMap;
use std::env;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StorageBackend {
    Local,
    R2,
}

pub struct AppConfig {
    pub server_host: String,
    pub server_port: u16,
    pub compression_level: i32,
    pub enable_media_transcode: bool,
    pub ffmpeg_bin: String,
    pub ffmpeg_video_crf: u8,
    pub ffmpeg_video_preset: String,
    pub ffmpeg_jpeg_quality: u8,
    pub ffmpeg_webp_quality: u8,
    pub storage_backend: StorageBackend,
    pub r2_endpoint: String,
    pub r2_bucket: String,
    pub r2_region: String,
    pub r2_access_key_id: String,
    pub r2_secret_access_key: String,
    pub r2_key_prefix: String,
    pub database_url: String,
    pub upload_dir: String,
    pub log_file: String,
    pub log_level: String,
    pub admin_username: String,
    pub admin_password: String,
    pub watch_folders: HashMap<String, String>,
}

impl AppConfig {
    pub fn from_env() -> Self {
        dotenvy::dotenv().ok();

        let server_host = env::var("SERVER_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
        let server_port = env::var("SERVER_PORT")
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .unwrap_or(8080);
        let compression_level = env::var("COMPRESSION_LEVEL")
            .ok()
            .and_then(|value| value.parse::<i32>().ok())
            .unwrap_or(22);
        let enable_media_transcode = parse_bool_env(env::var("ENABLE_MEDIA_TRANSCODE").ok(), false);
        let ffmpeg_bin = env::var("FFMPEG_BIN").unwrap_or_else(|_| "ffmpeg".to_string());
        let ffmpeg_video_crf = env::var("FFMPEG_VIDEO_CRF")
            .ok()
            .and_then(|value| value.parse::<u8>().ok())
            .map(|value| value.clamp(18, 40))
            .unwrap_or(28);
        let ffmpeg_video_preset =
            env::var("FFMPEG_VIDEO_PRESET").unwrap_or_else(|_| "medium".to_string());
        let ffmpeg_jpeg_quality = env::var("FFMPEG_JPEG_QUALITY")
            .ok()
            .and_then(|value| value.parse::<u8>().ok())
            .map(|value| value.clamp(2, 31))
            .unwrap_or(4);
        let ffmpeg_webp_quality = env::var("FFMPEG_WEBP_QUALITY")
            .ok()
            .and_then(|value| value.parse::<u8>().ok())
            .map(|value| value.clamp(10, 100))
            .unwrap_or(75);
        let storage_backend = parse_storage_backend(env::var("STORAGE_BACKEND").ok());
        let r2_endpoint = env::var("R2_ENDPOINT").unwrap_or_default();
        let r2_bucket = env::var("R2_BUCKET").unwrap_or_default();
        let r2_region = env::var("R2_REGION").unwrap_or_else(|_| "auto".to_string());
        let r2_access_key_id = env::var("R2_ACCESS_KEY_ID").unwrap_or_default();
        let r2_secret_access_key = env::var("R2_SECRET_ACCESS_KEY").unwrap_or_default();
        let r2_key_prefix = env::var("R2_KEY_PREFIX").unwrap_or_else(|_| "uploads".to_string());
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://storage/app.db".to_string());
        let upload_dir = env::var("UPLOAD_DIR").unwrap_or_else(|_| "storage/uploads".to_string());
        let log_file = env::var("LOG_FILE").unwrap_or_else(|_| "storage/logs/app.log".to_string());
        let log_level = env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let admin_username = env::var("ADMIN_USERNAME").unwrap_or_else(|_| "admin".to_string());
        let admin_password =
            env::var("ADMIN_PASSWORD").unwrap_or_else(|_| "admin123".to_string());
        let watch_folders = parse_watch_folders(env::var("WATCH_FOLDERS").ok());

        Self {
            server_host,
            server_port,
            compression_level,
            enable_media_transcode,
            ffmpeg_bin,
            ffmpeg_video_crf,
            ffmpeg_video_preset,
            ffmpeg_jpeg_quality,
            ffmpeg_webp_quality,
            storage_backend,
            r2_endpoint,
            r2_bucket,
            r2_region,
            r2_access_key_id,
            r2_secret_access_key,
            r2_key_prefix,
            database_url,
            upload_dir,
            log_file,
            log_level,
            admin_username,
            admin_password,
            watch_folders,
        }
    }
}

fn parse_bool_env(raw: Option<String>, default_value: bool) -> bool {
    match raw.as_deref().map(str::trim).map(str::to_ascii_lowercase) {
        Some(value) if matches!(value.as_str(), "1" | "true" | "yes" | "on") => true,
        Some(value) if matches!(value.as_str(), "0" | "false" | "no" | "off") => false,
        _ => default_value,
    }
}

fn parse_storage_backend(raw: Option<String>) -> StorageBackend {
    match raw.as_deref().map(str::trim).map(str::to_ascii_lowercase) {
        Some(value) if value == "r2" => StorageBackend::R2,
        _ => StorageBackend::Local,
    }
}

fn parse_watch_folders(raw: Option<String>) -> HashMap<String, String> {
    let mut folders = HashMap::new();

    if let Some(value) = raw {
        for pair in value.split(',') {
            let entry = pair.trim();
            if entry.is_empty() {
                continue;
            }

            if let Some((category, path)) = entry.split_once('=') {
                let category = category.trim();
                let path = path.trim();

                if !category.is_empty() && !path.is_empty() {
                    folders.insert(category.to_string(), path.to_string());
                }
            }
        }
    }

    if folders.is_empty() {
        folders.insert("file1".to_string(), "storage/file1".to_string());
    }

    folders
}
