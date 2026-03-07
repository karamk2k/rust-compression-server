use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::Result;
use tokio::process::Command;
use tracing::{info, warn};

#[derive(Clone, Debug)]
pub struct MediaTranscodeService {
    pub enabled: bool,
    pub ffmpeg_bin: String,
    pub video_crf: u8,
    pub video_preset: String,
    pub jpeg_quality: u8,
    pub webp_quality: u8,
}

impl MediaTranscodeService {
    pub fn new(
        enabled: bool,
        ffmpeg_bin: String,
        video_crf: u8,
        video_preset: String,
        jpeg_quality: u8,
        webp_quality: u8,
    ) -> Self {
        Self {
            enabled,
            ffmpeg_bin,
            video_crf,
            video_preset,
            jpeg_quality,
            webp_quality,
        }
    }

    pub async fn transcode_if_smaller(&self, input_path: &Path, ext: &str) -> Result<Option<u64>> {
        if !self.enabled {
            return Ok(None);
        }

        if !is_transcode_candidate(ext) {
            return Ok(None);
        }

        let output_path = temp_output_path(input_path, ext);
        let input_path_str = input_path.to_string_lossy().to_string();
        let output_path_str = output_path.to_string_lossy().to_string();
        let args = build_ffmpeg_args(ext, &input_path_str, &output_path_str, self);

        let output = match Command::new(&self.ffmpeg_bin).args(&args).output().await {
            Ok(output) => output,
            Err(error) => {
                warn!(
                    cmd = %self.ffmpeg_bin,
                    file = %input_path_str,
                    ?error,
                    "ffmpeg not available, keeping original media"
                );
                let _ = tokio::fs::remove_file(&output_path).await;
                return Ok(None);
            }
        };

        if !output.status.success() {
            let stderr = truncate_log_text(&output.stderr, 260);
            warn!(
                file = %input_path_str,
                stderr = %stderr,
                status = ?output.status.code(),
                "ffmpeg transcoding failed, keeping original media"
            );
            let _ = tokio::fs::remove_file(&output_path).await;
            return Ok(None);
        }

        let original_size = tokio::fs::metadata(input_path).await?.len();
        let transcoded_size = match tokio::fs::metadata(&output_path).await {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                warn!(file = %input_path_str, ?error, "missing ffmpeg output file");
                return Ok(None);
            }
        };

        if transcoded_size >= original_size {
            info!(
                file = %input_path_str,
                original_size,
                transcoded_size,
                "kept original media because transcoded file was not smaller"
            );
            let _ = tokio::fs::remove_file(&output_path).await;
            return Ok(None);
        }

        match tokio::fs::rename(&output_path, input_path).await {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                tokio::fs::remove_file(input_path).await?;
                tokio::fs::rename(&output_path, input_path).await?;
            }
            Err(error) => return Err(error.into()),
        }

        info!(
            file = %input_path_str,
            original_size,
            transcoded_size,
            "media transcoding applied"
        );
        Ok(Some(transcoded_size))
    }
}

fn temp_output_path(input_path: &Path, ext: &str) -> PathBuf {
    let file_name = input_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("upload.bin");
    let parent = input_path.parent().unwrap_or_else(|| Path::new("."));

    let temp_name = if ext.is_empty() {
        format!("{file_name}.ffmpeg.tmp")
    } else {
        format!("{file_name}.ffmpeg.tmp.{ext}")
    };

    parent.join(temp_name)
}

fn build_ffmpeg_args(
    ext: &str,
    input_path: &str,
    output_path: &str,
    settings: &MediaTranscodeService,
) -> Vec<String> {
    let mut args = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
        "-y".to_string(),
        "-i".to_string(),
        input_path.to_string(),
    ];

    match ext {
        "mp4" | "mov" | "mkv" => {
            args.extend([
                "-map_metadata".to_string(),
                "-1".to_string(),
                "-c:v".to_string(),
                "libx264".to_string(),
                "-preset".to_string(),
                settings.video_preset.clone(),
                "-crf".to_string(),
                settings.video_crf.to_string(),
                "-c:a".to_string(),
                "aac".to_string(),
                "-b:a".to_string(),
                "128k".to_string(),
            ]);
            if ext == "mp4" {
                args.extend(["-movflags".to_string(), "+faststart".to_string()]);
            }
        }
        "jpg" | "jpeg" => {
            args.extend([
                "-map_metadata".to_string(),
                "-1".to_string(),
                "-c:v".to_string(),
                "mjpeg".to_string(),
                "-q:v".to_string(),
                settings.jpeg_quality.to_string(),
            ]);
        }
        "png" => {
            args.extend([
                "-map_metadata".to_string(),
                "-1".to_string(),
                "-c:v".to_string(),
                "png".to_string(),
                "-compression_level".to_string(),
                "9".to_string(),
            ]);
        }
        "webp" => {
            args.extend([
                "-map_metadata".to_string(),
                "-1".to_string(),
                "-c:v".to_string(),
                "libwebp".to_string(),
                "-quality".to_string(),
                settings.webp_quality.to_string(),
            ]);
        }
        _ => {}
    }

    args.push(output_path.to_string());
    args
}

fn is_transcode_candidate(ext: &str) -> bool {
    matches!(ext, "mp4" | "mov" | "mkv" | "jpg" | "jpeg" | "png" | "webp")
}

fn truncate_log_text(bytes: &[u8], max_len: usize) -> String {
    let mut text = String::from_utf8_lossy(bytes).to_string();
    if text.len() > max_len {
        text.truncate(max_len);
        text.push_str("...");
    }
    text
}
