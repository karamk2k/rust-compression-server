pub mod drive;
pub mod protocol;
pub mod upload;

use serde_json::Value;

use crate::websocket::{hub::WebSocketHub, message::WsMessage};

/// An application event that can be delivered through the WebSocket hub.
///
/// Keeping event names and payload construction here prevents producers from
/// inventing slightly different wire formats in routes and background jobs.
pub trait RealtimeEvent {
    const NAME: &'static str;

    fn payload(&self) -> Value;

    fn message(&self) -> WsMessage {
        WsMessage::event(Self::NAME, self.payload())
    }
}

/// Deliver a private application event to every open connection for one user.
pub async fn dispatch_to_user<E: RealtimeEvent>(
    hub: &WebSocketHub,
    user_id: i64,
    event: &E,
) -> usize {
    hub.send_to_user(user_id, event.message()).await
}
