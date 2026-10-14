/// Event names owned by the WebSocket protocol itself.
///
/// Application/domain event names belong in their feature module instead.
pub const CONNECTION_ESTABLISHED: &str = "connection.established";
pub const SUBSCRIPTION_SUCCEEDED: &str = "subscription.succeeded";
pub const SUBSCRIPTION_REMOVED: &str = "subscription.removed";
pub const PRESENCE_MEMBER_ADDED: &str = "presence.member_added";
pub const PRESENCE_MEMBER_REMOVED: &str = "presence.member_removed";
pub const PONG: &str = "pong";
pub const ERROR: &str = "error";
