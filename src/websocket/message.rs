use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::events::protocol;

#[derive(Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ClientCommand {
    Subscribe { channel: String },
    Unsubscribe { channel: String },
    Ping { id: Option<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WsMessage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(default)]
    pub payload: Value,
}

impl WsMessage {
    pub fn event(event: impl Into<String>, payload: Value) -> Self {
        Self {
            id: None,
            event: event.into(),
            channel: None,
            payload,
        }
    }

    pub fn channel(event: impl Into<String>, channel: impl Into<String>, payload: Value) -> Self {
        Self {
            id: None,
            event: event.into(),
            channel: Some(channel.into()),
            payload,
        }
    }

    pub fn error(code: &str, message: &str, channel: Option<String>) -> Self {
        Self {
            id: None,
            event: protocol::ERROR.to_string(),
            channel,
            payload: json!({ "code": code, "message": message }),
        }
    }
}
