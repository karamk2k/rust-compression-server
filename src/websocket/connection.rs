use axum::extract::ws::{Message, WebSocket};
use serde_json::json;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::{
    app_state::AppState,
    events::protocol,
    websocket::{
        channels::{Channel, ChannelKind},
        hub::{ConnectionId, PresenceDeparture},
        message::{ClientCommand, WsMessage},
    },
};

const MAX_CHANNEL_NAME_LENGTH: usize = 128;

pub async fn handle_connection(socket: WebSocket, state: AppState, user_id: i64) {
    let (connection_id, receiver) = state.websocket_hub.register(user_id).await;
    info!(%connection_id, user_id, "websocket connected");

    state
        .websocket_hub
        .send_to_connection(
            connection_id,
            WsMessage::event(
                protocol::CONNECTION_ESTABLISHED,
                json!({ "connection_id": connection_id.to_string() }),
            ),
        )
        .await;

    handle_socket(socket, receiver, connection_id, user_id, &state).await;

    let departures = state.websocket_hub.unregister(connection_id).await;
    announce_departures(&state, departures).await;
    info!(%connection_id, user_id, "websocket disconnected");
}

async fn handle_socket(
    mut socket: WebSocket,
    mut receiver: mpsc::Receiver<WsMessage>,
    connection_id: ConnectionId,
    user_id: i64,
    state: &AppState,
) {
    loop {
        tokio::select! {
            socket_message = socket.recv() => {
                match socket_message {
                    Some(Ok(Message::Ping(payload))) => {
                        if socket.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(message)) => {
                        if !handle_incoming_message(message, connection_id, user_id, state).await {
                            break;
                        }
                    }
                    Some(Err(error)) => {
                        warn!(%connection_id, ?error, "websocket receive error");
                        break;
                    }
                    None => break,
                }
            }

            hub_message = receiver.recv() => {
                let Some(message) = hub_message else {
                    break;
                };
                let Ok(json) = serde_json::to_string(&message) else {
                    continue;
                };
                if socket.send(Message::Text(json)).await.is_err() {
                    break;
                }
            }
        }
    }
}

async fn handle_incoming_message(
    message: Message,
    connection_id: ConnectionId,
    user_id: i64,
    state: &AppState,
) -> bool {
    match message {
        Message::Text(text) => {
            let command = match serde_json::from_str::<ClientCommand>(&text) {
                Ok(command) => command,
                Err(_) => {
                    state
                        .websocket_hub
                        .send_to_connection(
                            connection_id,
                            WsMessage::error(
                                "invalid_command",
                                "Expected a subscribe, unsubscribe, or ping command",
                                None,
                            ),
                        )
                        .await;
                    return true;
                }
            };

            handle_command(command, connection_id, user_id, state).await;
            true
        }
        Message::Close(_) => false,
        Message::Pong(_) | Message::Binary(_) => true,
        Message::Ping(_) => true,
    }
}

async fn handle_command(
    command: ClientCommand,
    connection_id: ConnectionId,
    user_id: i64,
    state: &AppState,
) {
    match command {
        ClientCommand::Subscribe { channel } => {
            subscribe(connection_id, user_id, channel, state).await;
        }
        ClientCommand::Unsubscribe { channel } => {
            unsubscribe(connection_id, channel, state).await;
        }
        ClientCommand::Ping { id } => {
            let mut message = WsMessage::event(protocol::PONG, json!({}));
            message.id = id;
            state
                .websocket_hub
                .send_to_connection(connection_id, message)
                .await;
        }
    }
}

async fn subscribe(connection_id: ConnectionId, user_id: i64, channel: String, state: &AppState) {
    if channel.is_empty() || channel.len() > MAX_CHANNEL_NAME_LENGTH {
        send_subscription_error(
            state,
            connection_id,
            channel,
            "invalid_channel",
            "Channel names must contain between 1 and 128 bytes",
        )
        .await;
        return;
    }

    let Some(kind) = Channel::kind(&channel) else {
        send_subscription_error(
            state,
            connection_id,
            channel,
            "unknown_channel",
            "The requested channel is not registered",
        )
        .await;
        return;
    };

    if !Channel::can_join(&channel, user_id) {
        send_subscription_error(
            state,
            connection_id,
            channel,
            "subscription_forbidden",
            "You are not allowed to join this channel",
        )
        .await;
        return;
    }

    let Some(subscription) = state
        .websocket_hub
        .subscribe(connection_id, &channel, kind)
        .await
    else {
        return;
    };

    state
        .websocket_hub
        .send_to_connection(
            connection_id,
            WsMessage::channel(
                protocol::SUBSCRIPTION_SUCCEEDED,
                &channel,
                json!({
                    "kind": kind_name(kind),
                    "members": subscription.members,
                }),
            ),
        )
        .await;

    if subscription.member_added {
        state
            .websocket_hub
            .broadcast_except(
                &channel,
                WsMessage::channel(
                    protocol::PRESENCE_MEMBER_ADDED,
                    &channel,
                    json!({ "user_id": user_id }),
                ),
                Some(connection_id),
            )
            .await;
    }
}

async fn unsubscribe(connection_id: ConnectionId, channel: String, state: &AppState) {
    if !state
        .websocket_hub
        .is_subscribed(connection_id, &channel)
        .await
    {
        send_subscription_error(
            state,
            connection_id,
            channel,
            "not_subscribed",
            "This connection is not subscribed to the channel",
        )
        .await;
        return;
    }

    let departure = state
        .websocket_hub
        .unsubscribe(connection_id, &channel)
        .await;
    state
        .websocket_hub
        .send_to_connection(
            connection_id,
            WsMessage::channel(protocol::SUBSCRIPTION_REMOVED, &channel, json!({})),
        )
        .await;

    if let Some(departure) = departure {
        announce_departures(state, vec![departure]).await;
    }
}

async fn send_subscription_error(
    state: &AppState,
    connection_id: ConnectionId,
    channel: String,
    code: &str,
    message: &str,
) {
    state
        .websocket_hub
        .send_to_connection(
            connection_id,
            WsMessage::error(code, message, Some(channel)),
        )
        .await;
}

async fn announce_departures(state: &AppState, departures: Vec<PresenceDeparture>) {
    for departure in departures {
        state
            .websocket_hub
            .broadcast(
                &departure.channel,
                WsMessage::channel(
                    protocol::PRESENCE_MEMBER_REMOVED,
                    &departure.channel,
                    json!({ "user_id": departure.user_id }),
                ),
            )
            .await;
    }
}

fn kind_name(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Public => "public",
        ChannelKind::Private => "private",
        ChannelKind::Presence => "presence",
    }
}
