//! Sending a desktop notification (`org.freedesktop.Notifications`), for
//! a command that has something to say when nobody is watching a terminal:
//! "Colour copied".

use std::collections::HashMap;

use zbus::Variant;

use crate::bus::{Result, Service};

/// Show a notification from `app`; returns its id. Nothing waits for it to
/// be read.
pub fn send(app: &str, summary: &str, body: &str) -> Result<u32> {
    let hints: HashMap<&str, Variant<'_>> = HashMap::from([("transient", Variant::from(true))]);
    Service::session("org.freedesktop.Notifications")?.call(
        "/org/freedesktop/Notifications",
        "org.freedesktop.Notifications",
        "Notify",
        &(
            app,
            0u32,
            "",
            summary,
            body,
            Vec::<&str>::new(),
            hints,
            4000i32,
        ),
    )
}
