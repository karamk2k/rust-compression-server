use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio::sync::{RwLock, mpsc};
use uuid::Uuid;

use crate::websocket::{channels::ChannelKind, message::WsMessage};

const CONNECTION_CHANNEL_SIZE: usize = 128;

pub type ConnectionId = Uuid;

#[derive(Clone)]
pub struct WebSocketHub {
    connections: Arc<RwLock<HashMap<ConnectionId, Connection>>>,
}

struct Connection {
    user_id: i64,
    sender: mpsc::Sender<WsMessage>,
    subscriptions: HashMap<String, ChannelKind>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Subscription {
    pub members: Vec<i64>,
    pub member_added: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PresenceDeparture {
    pub channel: String,
    pub user_id: i64,
}

impl WebSocketHub {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(&self, user_id: i64) -> (ConnectionId, mpsc::Receiver<WsMessage>) {
        let connection_id = Uuid::new_v4();
        let (sender, receiver) = mpsc::channel(CONNECTION_CHANNEL_SIZE);

        self.connections.write().await.insert(
            connection_id,
            Connection {
                user_id,
                sender,
                subscriptions: HashMap::new(),
            },
        );
        (connection_id, receiver)
    }

    /// Removes a socket and returns presence members that fully left a channel.
    pub async fn unregister(&self, connection_id: ConnectionId) -> Vec<PresenceDeparture> {
        let mut connections = self.connections.write().await;
        let Some(connection) = connections.remove(&connection_id) else {
            return Vec::new();
        };

        connection
            .subscriptions
            .into_iter()
            .filter_map(|(channel, kind)| {
                if kind != ChannelKind::Presence {
                    return None;
                }

                let user_still_present = connections.values().any(|candidate| {
                    candidate.user_id == connection.user_id
                        && candidate.subscriptions.contains_key(&channel)
                });
                (!user_still_present).then_some(PresenceDeparture {
                    channel,
                    user_id: connection.user_id,
                })
            })
            .collect()
    }

    pub async fn subscribe(
        &self,
        connection_id: ConnectionId,
        channel: &str,
        kind: ChannelKind,
    ) -> Option<Subscription> {
        let mut connections = self.connections.write().await;
        let user_id = connections.get(&connection_id)?.user_id;

        let mut members = if kind == ChannelKind::Presence {
            connections
                .values()
                .filter(|connection| connection.subscriptions.contains_key(channel))
                .map(|connection| connection.user_id)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        members.sort_unstable();
        let member_added = kind == ChannelKind::Presence && !members.contains(&user_id);

        connections
            .get_mut(&connection_id)?
            .subscriptions
            .insert(channel.to_string(), kind);

        if kind == ChannelKind::Presence && member_added {
            members.push(user_id);
            members.sort_unstable();
        }
        Some(Subscription {
            members,
            member_added,
        })
    }

    pub async fn unsubscribe(
        &self,
        connection_id: ConnectionId,
        channel: &str,
    ) -> Option<PresenceDeparture> {
        let mut connections = self.connections.write().await;
        let connection = connections.get_mut(&connection_id)?;
        let user_id = connection.user_id;
        let kind = connection.subscriptions.remove(channel)?;

        if kind != ChannelKind::Presence {
            return None;
        }

        let user_still_present = connections.values().any(|candidate| {
            candidate.user_id == user_id && candidate.subscriptions.contains_key(channel)
        });
        (!user_still_present).then_some(PresenceDeparture {
            channel: channel.to_string(),
            user_id,
        })
    }

    pub async fn is_subscribed(&self, connection_id: ConnectionId, channel: &str) -> bool {
        self.connections
            .read()
            .await
            .get(&connection_id)
            .is_some_and(|connection| connection.subscriptions.contains_key(channel))
    }

    pub async fn send_to_connection(
        &self,
        connection_id: ConnectionId,
        message: WsMessage,
    ) -> bool {
        let sender = self
            .connections
            .read()
            .await
            .get(&connection_id)
            .map(|connection| connection.sender.clone());

        let Some(sender) = sender else {
            return false;
        };
        sender.try_send(message).is_ok()
    }

    pub async fn send_to_user(&self, user_id: i64, message: WsMessage) -> usize {
        let senders = self
            .connections
            .read()
            .await
            .values()
            .filter(|connection| connection.user_id == user_id)
            .map(|connection| connection.sender.clone())
            .collect::<Vec<_>>();
        Self::send(senders, message).await
    }

    pub async fn broadcast(&self, channel: &str, message: WsMessage) -> usize {
        self.broadcast_except(channel, message, None).await
    }

    pub async fn broadcast_except(
        &self,
        channel: &str,
        message: WsMessage,
        excluded: Option<ConnectionId>,
    ) -> usize {
        let senders = self
            .connections
            .read()
            .await
            .iter()
            .filter(|(id, connection)| {
                Some(**id) != excluded && connection.subscriptions.contains_key(channel)
            })
            .map(|(_, connection)| connection.sender.clone())
            .collect::<Vec<_>>();
        Self::send(senders, message).await
    }

    async fn send(senders: Vec<mpsc::Sender<WsMessage>>, message: WsMessage) -> usize {
        let mut delivered = 0;
        for sender in senders {
            // A slow browser must not block an application request or every
            // other subscriber. A dropped event is recovered by HTTP state
            // reconciliation when the client reconnects.
            if sender.try_send(message.clone()).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    pub async fn connection_count(&self) -> usize {
        self.connections.read().await.len()
    }

    pub async fn user_connection_count(&self, user_id: i64) -> usize {
        self.connections
            .read()
            .await
            .values()
            .filter(|connection| connection.user_id == user_id)
            .count()
    }
}

impl Default for WebSocketHub {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::WebSocketHub;
    use crate::websocket::channels::ChannelKind;

    #[tokio::test]
    async fn one_connection_can_subscribe_to_multiple_channels() {
        let hub = WebSocketHub::new();
        let (connection_id, _receiver) = hub.register(7).await;
        hub.subscribe(connection_id, "one", ChannelKind::Public)
            .await;
        hub.subscribe(connection_id, "two", ChannelKind::Private)
            .await;
        assert!(hub.is_subscribed(connection_id, "one").await);
        assert!(hub.is_subscribed(connection_id, "two").await);
    }

    #[tokio::test]
    async fn presence_tracks_users_instead_of_browser_tabs() {
        let hub = WebSocketHub::new();
        let (first, _first_receiver) = hub.register(7).await;
        let (second, _second_receiver) = hub.register(7).await;

        let first_join = hub
            .subscribe(first, "room", ChannelKind::Presence)
            .await
            .unwrap();
        let second_join = hub
            .subscribe(second, "room", ChannelKind::Presence)
            .await
            .unwrap();
        assert!(first_join.member_added);
        assert!(!second_join.member_added);
        assert!(hub.unsubscribe(first, "room").await.is_none());

        let departure = hub.unsubscribe(second, "room").await.unwrap();
        assert_eq!(departure.user_id, 7);
        assert_eq!(departure.channel, "room");
    }
}
