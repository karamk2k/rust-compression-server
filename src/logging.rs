use std::path::Path;

use anyhow::{anyhow, Result};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

pub fn init_logging(log_level: &str, log_file: &str) -> Result<WorkerGuard> {
    let log_path = Path::new(log_file);
    let parent_dir = log_path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent_dir)?;

    let file_name = log_path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| anyhow!("invalid LOG_FILE path: {}", log_file))?;

    let file_appender = tracing_appender::rolling::never(parent_dir, file_name);
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_new(log_level).unwrap_or_else(|_| EnvFilter::new("info"));
    let stdout_layer = tracing_subscriber::fmt::layer().with_target(false);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(file_writer);

    tracing_subscriber::registry()
        .with(filter)
        .with(stdout_layer)
        .with(file_layer)
        .init();

    Ok(guard)
}
