use crate::websocket::channels::Channel;
use tracing::info;

/// Declare application WebSocket channels here.
pub fn register() {
    Channel::new("general", |_user_id| true);
    Channel::private("private", |_user_id| true);
    Channel::presence("presence", |_user_id| true);
    Channel::private("my-test", test_channel);
}

fn test_channel(var: i64) -> bool {
    info!("test channel");
    info!("test channel var: {}", var);
    true
}
