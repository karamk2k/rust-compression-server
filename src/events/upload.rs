use serde_json::{Value, json};

use crate::{events::RealtimeEvent, models::file_model::UploadResult};

pub struct UploadQueued<'a> {
    pub job_id: i64,
    pub original_name: &'a str,
    pub folder_id: Option<i64>,
}

impl RealtimeEvent for UploadQueued<'_> {
    const NAME: &'static str = "upload.queued";

    fn payload(&self) -> Value {
        json!({
            "job_id": self.job_id,
            "original_name": self.original_name,
            "folder_id": self.folder_id,
            "status": "pending",
        })
    }
}

pub struct UploadCompleted<'a> {
    pub job_id: i64,
    pub file: &'a UploadResult,
}

impl RealtimeEvent for UploadCompleted<'_> {
    const NAME: &'static str = "upload.completed";

    fn payload(&self) -> Value {
        // Do not expose internal local/R2 storage paths over the socket.
        json!({
            "job_id": self.job_id,
            "status": "done",
            "file": {
                "id": self.file.id,
                "original_name": self.file.original_name,
                "folder_id": self.file.folder_id,
                "thumbnail_path": self.file.thumbnail_path,
                "is_compressed": self.file.is_compressed,
                "original_size": self.file.original_size,
                "compressed_size": self.file.compressed_size,
            },
        })
    }
}

pub struct UploadFailed<'a> {
    pub job_id: i64,
    pub original_name: &'a str,
    pub folder_id: Option<i64>,
    pub error: &'a str,
}

impl RealtimeEvent for UploadFailed<'_> {
    const NAME: &'static str = "upload.failed";

    fn payload(&self) -> Value {
        json!({
            "job_id": self.job_id,
            "original_name": self.original_name,
            "folder_id": self.folder_id,
            "status": "failed",
            "error": self.error,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::UploadQueued;
    use crate::events::RealtimeEvent;

    #[test]
    fn queued_event_has_a_stable_envelope() {
        let message = UploadQueued {
            job_id: 12,
            original_name: "photo.jpg",
            folder_id: Some(4),
        }
        .message();

        assert_eq!(message.event, "upload.queued");
        assert_eq!(message.channel, None);
        assert_eq!(message.payload["job_id"], 12);
        assert_eq!(message.payload["folder_id"], 4);
    }
}
