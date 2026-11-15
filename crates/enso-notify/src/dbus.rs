//! The `org.freedesktop.Notifications` service (Desktop Notifications
//! Specification 1.2), served with zbus on its own thread.

use std::{collections::HashMap, thread, time::Duration};

use enso_ui::Proxy;
use zbus::{Connection, connection, interface, object_server::SignalEmitter, wire::OwnedVariant};

use crate::{
    daemon::Message,
    stack::{CloseReason, Notification, Timeout, Urgency},
};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

struct Server {
    proxy: Proxy<Message>,
    next_id: u32,
}

#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    /// What this server can render. Markup, actions, and images are not
    /// advertised yet, so well-behaved clients send plain text.
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body"]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &mut self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, OwnedVariant>,
        expire_timeout: i32,
    ) -> u32 {
        let id = if replaces_id != 0 {
            replaces_id
        } else {
            let id = self.next_id;
            // Ids are never zero, even after wrapping around.
            self.next_id = self.next_id.checked_add(1).unwrap_or(1);
            id
        };
        let urgency = match hints
            .get("urgency")
            .and_then(|value| value.downcast_ref::<u8>().ok())
        {
            Some(0) => Urgency::Low,
            Some(2) => Urgency::Critical,
            _ => Urgency::Normal,
        };
        let timeout = match expire_timeout {
            0 => Timeout::Never,
            millis if millis > 0 => {
                Timeout::After(Duration::from_millis(millis.unsigned_abs().into()))
            }
            _ => Timeout::Default,
        };
        self.proxy.send(Message::Notify(Notification {
            id,
            app_name,
            summary,
            body,
            urgency,
            timeout,
        }));
        id
    }

    fn close_notification(&self, id: u32) {
        self.proxy.send(Message::Close(id));
    }

    /// Name, vendor, version, and the spec version implemented.
    fn get_server_information(&self) -> (&'static str, &'static str, &'static str, &'static str) {
        ("enso-notify", "enso", env!("CARGO_PKG_VERSION"), "1.2")
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: &str,
    ) -> zbus::Result<()>;
}

/// Claim the notification service on the session bus and serve it from a
/// background thread. The outcome arrives as [`Message::Ready`] or
/// [`Message::Failed`].
pub fn serve(proxy: Proxy<Message>) {
    let failed = proxy.clone();
    let spawned = thread::Builder::new().name("enso-dbus".to_owned()).spawn(move || {
        zbus::block_on(async move {
            let server = Server { proxy: proxy.clone(), next_id: 1 };
            match connection::Builder::session().name(NAME)
                // Never take the name from, or lose it to, another daemon.
                .allow_name_replacements(false)
                .replace_existing_names(false)
                .serve_at(PATH, server).build().await {
                Ok(connection) => {
                    proxy.send(Message::Ready(connection));
                    // Keep serving for the life of the process.
                    std::future::pending::<()>().await;
                }
                Err(zbus::Error::NameTaken) => {
                    proxy.send(Message::Failed(
                        "another notification daemon already owns org.freedesktop.Notifications".to_owned(),
                    ));
                }
                Err(err) => {
                    proxy.send(Message::Failed(format!("cannot serve notifications on the session bus: {err}")));
                }
            }
        });
    });
    if let Err(err) = spawned {
        failed.send(Message::Failed(format!(
            "cannot start the D-Bus thread: {err}"
        )));
    }
}

/// Tell clients a notification is gone.
pub fn emit_closed(connection: &Connection, id: u32, reason: CloseReason) {
    let result = zbus::block_on(async {
        let emitter = SignalEmitter::new(connection, PATH)?;
        Server::notification_closed(&emitter, id, reason as u32).await
    });
    if let Err(err) = result {
        eprintln!("enso-notify: cannot emit NotificationClosed: {err}");
    }
}
