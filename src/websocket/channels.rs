use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, RwLock},
};

type AuthorizeCallback = Arc<dyn Fn(i64) -> bool + Send + Sync>;

static CHANNELS: LazyLock<RwLock<HashMap<String, Channel>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelKind {
    Public,
    Private,
    Presence,
}

pub struct Channel {
    kind: ChannelKind,
    authorize: AuthorizeCallback,
}

impl Channel {
    /// Declares a public channel. Returns `false` if the name is already in use.
    pub fn new<F>(name: impl Into<String>, callback: F) -> bool
    where
        F: Fn(i64) -> bool + Send + Sync + 'static,
    {
        Self::declare(name, ChannelKind::Public, callback)
    }

    pub fn private<F>(name: impl Into<String>, callback: F) -> bool
    where
        F: Fn(i64) -> bool + Send + Sync + 'static,
    {
        Self::declare(name, ChannelKind::Private, callback)
    }

    pub fn presence<F>(name: impl Into<String>, callback: F) -> bool
    where
        F: Fn(i64) -> bool + Send + Sync + 'static,
    {
        Self::declare(name, ChannelKind::Presence, callback)
    }

    fn declare<F>(name: impl Into<String>, kind: ChannelKind, callback: F) -> bool
    where
        F: Fn(i64) -> bool + Send + Sync + 'static,
    {
        let mut channels = CHANNELS.write().expect("channel registry lock poisoned");
        let name = name.into();

        if channels.contains_key(&name) {
            return false;
        }

        channels.insert(
            name,
            Self {
                kind,
                authorize: Arc::new(callback),
            },
        );
        true
    }

    pub fn exists(name: &str) -> bool {
        CHANNELS
            .read()
            .expect("channel registry lock poisoned")
            .contains_key(name)
    }

    pub fn kind(name: &str) -> Option<ChannelKind> {
        CHANNELS
            .read()
            .expect("channel registry lock poisoned")
            .get(name)
            .map(|channel| channel.kind)
    }

    pub fn can_join(name: &str, user_id: i64) -> bool {
        let authorize = CHANNELS
            .read()
            .expect("channel registry lock poisoned")
            .get(name)
            .map(|channel| Arc::clone(&channel.authorize));

        authorize.is_some_and(|callback| callback(user_id))
    }
}

#[cfg(test)]
mod tests {
    use super::{Channel, ChannelKind};

    #[test]
    fn channel_callback_controls_access() {
        let name = format!("private-{}", uuid::Uuid::new_v4());
        assert!(Channel::private(&name, |user_id| user_id == 42));
        assert_eq!(Channel::kind(&name), Some(ChannelKind::Private));
        assert!(Channel::can_join(&name, 42));
        assert!(!Channel::can_join(&name, 7));
        assert!(!Channel::new(&name, |_| true));
    }

    #[test]
    fn presence_channels_are_identified() {
        let name = format!("presence-{}", uuid::Uuid::new_v4());
        assert!(Channel::presence(&name, |_| true));
        assert_eq!(Channel::kind(&name), Some(ChannelKind::Presence));
    }
}
