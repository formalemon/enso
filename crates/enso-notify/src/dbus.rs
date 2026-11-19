//! The `org.freedesktop.Notifications` service (Desktop Notifications
//! Specification 1.2), served with zbus on its own thread.

use std::{collections::HashMap, path::Path, thread, time::Duration};

use enso_ui::{Image, Proxy};
use zbus::{Connection, connection, interface, object_server::SignalEmitter, wire::OwnedVariant};

use crate::{
    daemon::Message,
    local, markup,
    stack::{Action, CloseReason, Notification, Timeout, Urgency},
};

const NAME: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";

struct Server {
    proxy: Proxy<Message>,
    /// Where a picture must be for enso to offer to open it.
    home: std::path::PathBuf,
    next_id: u32,
    /// Ids of notifications that named a stack tag, so the next one with
    /// the same tag replaces it: how volume pop-ups update in place.
    tagged: HashMap<String, u32>,
}

/// Hints under which senders tag notifications that replace each other.
const TAG_HINTS: [&str; 3] = [
    "x-canonical-private-synchronous",
    "synchronous",
    "x-dunst-stack-tag",
];
/// Hints carrying raw pixels, newest name first.
const IMAGE_DATA_HINTS: [&str; 3] = ["image-data", "image_data", "icon_data"];
const IMAGE_PATH_HINTS: [&str; 2] = ["image-path", "image_path"];

/// Raw pixels as the specification sends them: width, height, rowstride,
/// has-alpha, bits per sample, channels, data.
type RawImage = (i32, i32, i32, bool, i32, i32, Vec<u8>);

fn image_from_raw(raw: RawImage) -> Option<Image> {
    let (width, height, rowstride, has_alpha, bits, channels, data) = raw;
    if bits != 8 || channels != if has_alpha { 4 } else { 3 } {
        return None;
    }
    Image::from_raw(
        width.try_into().ok()?,
        height.try_into().ok()?,
        rowstride.try_into().ok()?,
        has_alpha,
        &data,
    )
}

/// Longest side a picture is decoded at: it's drawn far smaller.
const PICTURE_SIDE: u32 = 512;

/// A picture named by path or `file://` URL. Icon-theme names are not
/// looked up: the suite carries no icon theme.
fn image_from_path(name: &str) -> Option<Image> {
    let path = name.strip_prefix("file://").unwrap_or(name);
    if !path.starts_with('/') {
        return None;
    }
    let path = Path::new(path);
    // Nothing a sender names is worth reading megabytes for.
    if !path.metadata().is_ok_and(|meta| meta.len() <= 8 << 20) {
        return None;
    }
    let pixels = enso_fs::picture::open(path, PICTURE_SIDE).ok()?;
    Image::from_raw(
        pixels.width,
        pixels.height,
        pixels.width as usize * 4,
        true,
        &pixels.rgba,
    )
}

/// Pair up the flat `[key, label, key, label, …]` list.
fn actions_from(list: &[String]) -> Vec<Action> {
    list.as_chunks::<2>()
        .0
        .iter()
        .map(|[key, label]| Action {
            key: key.clone(),
            label: label.clone(),
        })
        .collect()
}

#[interface(name = "org.freedesktop.Notifications")]
impl Server {
    /// What this server can do. Markup is understood and shown as plain
    /// text; hyperlinks and sounds are not offered.
    fn get_capabilities(&self) -> Vec<&'static str> {
        vec!["body", "body-markup", "actions", "icon-static"]
    }

    #[allow(clippy::too_many_arguments)]
    fn notify(
        &mut self,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        mut hints: HashMap<String, OwnedVariant>,
        expire_timeout: i32,
    ) -> u32 {
        let tag = TAG_HINTS
            .iter()
            .find_map(|hint| hints.remove(*hint))
            .and_then(|value| String::try_from(value).ok())
            .filter(|tag| !tag.is_empty());
        let tagged = tag.as_ref().and_then(|tag| self.tagged.get(tag)).copied();
        let id = if replaces_id != 0 {
            replaces_id
        } else if let Some(id) = tagged {
            id
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
        if let Some(tag) = tag {
            self.tagged.insert(tag, id);
        }
        let progress = hints
            .get("value")
            .and_then(|value| {
                value
                    .downcast_ref::<i32>()
                    .ok()
                    .or_else(|| value.downcast_ref::<u32>().ok().map(|v| v.min(100) as i32))
            })
            .map(|value| value.clamp(0, 100) as u8);
        // Some senders give the flag as a number rather than a boolean.
        let transient = hints.get("transient").is_some_and(|value| {
            value
                .downcast_ref::<bool>()
                .ok()
                .or_else(|| value.downcast_ref::<i32>().ok().map(|v| v != 0))
                .or_else(|| value.downcast_ref::<u8>().ok().map(|v| v != 0))
                .unwrap_or(false)
        });
        // A picture named by file: shown, and perhaps something to open.
        let picture = IMAGE_PATH_HINTS
            .iter()
            .find_map(|hint| hints.remove(*hint))
            .and_then(|value| String::try_from(value).ok());
        // In the specification's order of preference: pixels sent along,
        // then a picture named by a hint, then the application's icon.
        let image = IMAGE_DATA_HINTS
            .iter()
            .find_map(|hint| hints.remove(*hint))
            .and_then(|value| RawImage::try_from(value).ok())
            .and_then(image_from_raw)
            .or_else(|| picture.as_deref().and_then(image_from_path))
            .or_else(|| image_from_path(&app_icon));
        let picture_file = picture
            .as_deref()
            .map(|name| Path::new(name.strip_prefix("file://").unwrap_or(name)).to_owned());
        let mut note = Notification {
            id,
            app_name,
            summary: summary.trim().to_owned(),
            body: markup::plain(&body),
            urgency,
            timeout,
            actions: actions_from(&actions),
            image,
            progress,
            transient,
        };
        // Senders can't claim enso's own buttons.
        note.actions
            .retain(|action| !action.key.starts_with(local::PREFIX));
        local::enrich(&mut note, picture_file.as_deref(), &self.home);
        self.proxy.send(Message::Notify(note));
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
            let server = Server {
                proxy: proxy.clone(),
                home: enso_fs::places::home(),
                next_id: 1,
                tagged: HashMap::new(),
            };
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

/// Tell the sender one of its notification's actions was chosen.
pub fn emit_action(connection: &Connection, id: u32, key: &str) {
    let result = zbus::block_on(async {
        let emitter = SignalEmitter::new(connection, PATH)?;
        Server::action_invoked(&emitter, id, key).await
    });
    if let Err(err) = result {
        eprintln!("enso-notify: cannot emit ActionInvoked: {err}");
    }
}

/// Ask the file manager to show `path`, selected in its folder
/// (`org.freedesktop.FileManager1`, which enso-files and the others answer).
pub fn show_in_files(connection: &Connection, path: &Path) -> zbus::Result<()> {
    let uri = enso_fs::uri::from_path(path);
    zbus::block_on(connection.call_method(
        Some("org.freedesktop.FileManager1"),
        "/org/freedesktop/FileManager1",
        Some("org.freedesktop.FileManager1"),
        "ShowItems",
        &(vec![uri], ""),
    ))
    .map(drop)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_come_in_pairs() {
        let list: Vec<String> = ["default", "Open", "reply", "Reply", "dangling"]
            .map(String::from)
            .to_vec();
        let actions = actions_from(&list);
        assert_eq!(actions.len(), 2);
        assert_eq!(
            (actions[1].key.as_str(), actions[1].label.as_str()),
            ("reply", "Reply")
        );
        assert!(actions_from(&[]).is_empty());
    }

    #[test]
    fn raw_images_must_be_eight_bit_rgb_or_rgba() {
        let pixel = vec![10, 20, 30, 255];
        assert!(image_from_raw((1, 1, 4, true, 8, 4, pixel.clone())).is_some());
        assert!(image_from_raw((1, 1, 3, false, 8, 3, pixel.clone())).is_some());
        // Sixteen-bit samples, a channel count that contradicts the alpha
        // flag, and negative sizes are all refused.
        assert!(image_from_raw((1, 1, 4, true, 16, 4, pixel.clone())).is_none());
        assert!(image_from_raw((1, 1, 4, true, 8, 3, pixel.clone())).is_none());
        assert!(image_from_raw((-1, 1, 4, true, 8, 4, pixel)).is_none());
    }

    #[test]
    fn image_hints_convert_from_the_wire_type() {
        use zbus::wire::Variant;
        let raw: RawImage = (1, 1, 3, false, 8, 3, vec![1, 2, 3]);
        let sent = OwnedVariant::try_from(Variant::from(raw.clone())).unwrap();
        assert_eq!(RawImage::try_from(sent).unwrap(), raw);
        // Something else under the same name is simply no picture.
        let wrong = OwnedVariant::try_from(Variant::from("picture")).unwrap();
        assert!(RawImage::try_from(wrong).is_err());
    }

    #[test]
    fn only_png_files_named_by_path_are_loaded() {
        assert!(image_from_path("dialog-information").is_none());
        assert!(image_from_path("").is_none());
        assert!(image_from_path("/nonexistent/picture.png").is_none());
        assert!(image_from_path("file:///nonexistent/picture.png").is_none());
        assert!(image_from_path("/etc/hostname").is_none());
    }
}
