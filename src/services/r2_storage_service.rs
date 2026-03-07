use std::path::Path;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use aws_credential_types::Credentials;
use aws_sdk_s3::config::{
    Builder as S3ConfigBuilder, Region, RequestChecksumCalculation, ResponseChecksumValidation,
};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::Client;
use tokio::time::sleep;
use tracing::{debug, info, warn};

#[derive(Clone, Debug)]
pub struct R2StorageService {
    client: Client,
    bucket: String,
    key_prefix: String,
}

#[derive(Debug)]
pub struct R2ObjectStream {
    pub body: ByteStream,
    pub content_length: Option<i64>,
    pub content_range: Option<String>,
}

impl R2StorageService {
    pub async fn new(
        endpoint: String,
        bucket: String,
        region: String,
        access_key_id: String,
        secret_access_key: String,
        key_prefix: String,
    ) -> Result<Self> {
        if endpoint.trim().is_empty() {
            return Err(anyhow!("R2 endpoint is required when STORAGE_BACKEND=r2"));
        }
        if bucket.trim().is_empty() {
            return Err(anyhow!("R2 bucket is required when STORAGE_BACKEND=r2"));
        }
        if access_key_id.trim().is_empty() {
            return Err(anyhow!("R2 access key is required when STORAGE_BACKEND=r2"));
        }
        if secret_access_key.trim().is_empty() {
            return Err(anyhow!("R2 secret key is required when STORAGE_BACKEND=r2"));
        }

        let shared_config = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(Region::new(region))
            .credentials_provider(Credentials::new(
                access_key_id,
                secret_access_key,
                None,
                None,
                "static-r2",
            ))
            .load()
            .await;

        let s3_config = S3ConfigBuilder::from(&shared_config)
            .endpoint_url(endpoint)
            .force_path_style(true)
            .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
            .response_checksum_validation(ResponseChecksumValidation::WhenRequired)
            .build();

        Ok(Self {
            client: Client::from_conf(s3_config),
            bucket,
            key_prefix: normalize_prefix(&key_prefix),
        })
    }

    pub fn db_path_for_key(&self, key: &str) -> String {
        format!("r2://{key}")
    }

    pub fn key_from_db_path<'a>(&self, db_path: &'a str) -> Result<&'a str> {
        db_path
            .strip_prefix("r2://")
            .ok_or_else(|| anyhow!("invalid R2 path format: {db_path}"))
    }

    pub fn is_r2_path(path: &str) -> bool {
        path.starts_with("r2://")
    }

    pub fn object_key_for_filename(&self, file_name: &str) -> String {
        if self.key_prefix.is_empty() {
            file_name.to_string()
        } else {
            format!("{}/{}", self.key_prefix, file_name)
        }
    }

    pub async fn upload_path(&self, key: &str, local_path: &Path) -> Result<()> {
        let file_size = tokio::fs::metadata(local_path)
            .await
            .with_context(|| format!("failed to stat local file {}", local_path.display()))?
            .len() as i64;
        let max_attempts = 4u8;

        for attempt in 1..=max_attempts {
            let body = ByteStream::from_path(local_path.to_path_buf())
                .await
                .with_context(|| format!("failed to read local file {}", local_path.display()))?;

            let result = self
                .client
                .put_object()
                .bucket(&self.bucket)
                .key(key)
                .content_length(file_size)
                .body(body)
                .send()
                .await;

            match result {
                Ok(_) => {
                    info!(
                        bucket = %self.bucket,
                        key = %key,
                        attempt,
                        "uploaded object to R2"
                    );
                    return Ok(());
                }
                Err(error) if attempt < max_attempts => {
                    warn!(
                        bucket = %self.bucket,
                        key = %key,
                        attempt,
                        ?error,
                        "R2 upload failed; retrying"
                    );
                    let backoff_ms = 250u64 * 2u64.pow((attempt - 1) as u32);
                    sleep(Duration::from_millis(backoff_ms)).await;
                }
                Err(error) => {
                    return Err(error).with_context(|| format!(
                        "failed to upload object key={key} after {max_attempts} attempts"
                    ));
                }
            }
        }

        unreachable!("upload loop always returns before this point")
    }

    pub async fn get_object_bytes(&self, db_path: &str) -> Result<Vec<u8>> {
        let key = self.key_from_db_path(db_path)?;
        let output = self.client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("failed to read object key={key}"))?;

        let bytes = output
            .body
            .collect()
            .await
            .with_context(|| format!("failed to collect object bytes key={key}"))?
            .into_bytes()
            .to_vec();

        debug!(bucket = %self.bucket, key = %key, size = bytes.len(), "downloaded object from R2");
        Ok(bytes)
    }

    pub async fn get_object_stream(
        &self,
        db_path: &str,
        range_header: Option<&str>,
    ) -> Result<R2ObjectStream> {
        let key = self.key_from_db_path(db_path)?;
        let mut request = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key);

        if let Some(range) = range_header {
            if !range.trim().is_empty() {
                request = request.range(range);
            }
        }

        let output = request
            .send()
            .await
            .with_context(|| format!("failed to stream object key={key}"))?;

        Ok(R2ObjectStream {
            body: output.body,
            content_length: output.content_length,
            content_range: output.content_range,
        })
    }

    pub async fn delete_object(&self, db_path: &str) -> Result<()> {
        let key = self.key_from_db_path(db_path)?;
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .with_context(|| format!("failed to delete object key={key}"))?;

        info!(bucket = %self.bucket, key = %key, "deleted object from R2");
        Ok(())
    }
}

fn normalize_prefix(prefix: &str) -> String {
    prefix.trim_matches('/').to_string()
}
