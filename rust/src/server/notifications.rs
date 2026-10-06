use rmcp::model::{Notification, ResourceUpdatedNotificationParam, ServerNotification};
use rmcp::service::{Peer, RoleServer};

pub async fn send_resource_updated(peer: &Peer<RoleServer>, uri: &str) {
    let notif = Notification::new(ResourceUpdatedNotificationParam::new(uri));
    let server_notif = ServerNotification::ResourceUpdatedNotification(notif);
    if let Err(e) = peer.send_notification(server_notif).await {
        tracing::debug!("Failed to send resource updated notification: {e}");
    }
}

pub const RESOURCE_URI_SUMMARY: &str = "lean-ctx://context/summary";
