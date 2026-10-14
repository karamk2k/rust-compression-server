use serde_json::{Value, json};

use crate::{events::RealtimeEvent, models::file_model::UploadResult};

pub struct FileCreated<'a> {
    pub file: &'a UploadResult,
}

impl RealtimeEvent for FileCreated<'_> {
    const NAME: &'static str = "file.created";

    fn payload(&self) -> Value {
        json!({
            "file_id": self.file.id,
            "folder_id": self.file.folder_id,
        })
    }
}

pub struct FilesMoved<'a> {
    pub file_ids: &'a [i64],
    pub folder_id: Option<i64>,
}

impl RealtimeEvent for FilesMoved<'_> {
    const NAME: &'static str = "file.moved";

    fn payload(&self) -> Value {
        json!({
            "file_ids": self.file_ids,
            "folder_id": self.folder_id,
        })
    }
}

pub struct FilesDeleted<'a> {
    pub file_ids: &'a [i64],
}

impl RealtimeEvent for FilesDeleted<'_> {
    const NAME: &'static str = "file.deleted";

    fn payload(&self) -> Value {
        json!({ "file_ids": self.file_ids })
    }
}

pub struct FolderCreated {
    pub folder_id: i64,
    pub parent_id: Option<i64>,
}

impl RealtimeEvent for FolderCreated {
    const NAME: &'static str = "folder.created";

    fn payload(&self) -> Value {
        json!({
            "folder_id": self.folder_id,
            "parent_id": self.parent_id,
        })
    }
}

pub struct FolderRenamed {
    pub folder_id: i64,
    pub name: String,
}

impl RealtimeEvent for FolderRenamed {
    const NAME: &'static str = "folder.renamed";

    fn payload(&self) -> Value {
        json!({
            "folder_id": self.folder_id,
            "name": self.name,
        })
    }
}

pub struct FolderMoved {
    pub folder_id: i64,
    pub parent_id: Option<i64>,
}

impl RealtimeEvent for FolderMoved {
    const NAME: &'static str = "folder.moved";

    fn payload(&self) -> Value {
        json!({
            "folder_id": self.folder_id,
            "parent_id": self.parent_id,
        })
    }
}

pub struct FolderDeleted {
    pub folder_id: i64,
}

impl RealtimeEvent for FolderDeleted {
    const NAME: &'static str = "folder.deleted";

    fn payload(&self) -> Value {
        json!({ "folder_id": self.folder_id })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{FilesMoved, FolderDeleted};
    use crate::events::RealtimeEvent;

    #[test]
    fn drive_events_use_stable_names_and_payloads() {
        let moved = FilesMoved {
            file_ids: &[2, 4],
            folder_id: Some(8),
        }
        .message();
        assert_eq!(moved.event, "file.moved");
        assert_eq!(moved.payload["file_ids"], json!([2, 4]));

        let deleted = FolderDeleted { folder_id: 3 }.message();
        assert_eq!(deleted.event, "folder.deleted");
        assert_eq!(deleted.payload["folder_id"], 3);
    }
}
