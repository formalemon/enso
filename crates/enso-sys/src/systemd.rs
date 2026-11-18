//! The user's own service manager: restarting a service of the session.
//!
//! Some programs read their settings once, when they start, and live for
//! the whole session — the GTK portal, which draws every file chooser, is
//! one. A setting changed under them reaches them only by a restart.

use crate::bus::{Result, Service};

const MANAGER: &str = "org.freedesktop.systemd1.Manager";
const MANAGER_PATH: &str = "/org/freedesktop/systemd1";

/// Restart a user unit (`"xdg-desktop-portal-gtk.service"`) if it is
/// running, and leave it be if not: a service started on demand starts
/// with the new settings anyway. Returns once systemd has queued the job.
pub fn try_restart(unit: &str) -> Result<()> {
    // The reply is the job's path, of use only to one who waits on it.
    let _job: zbus::wire::OwnedObjectPath = Service::session("org.freedesktop.systemd1")?.call(
        MANAGER_PATH,
        MANAGER,
        "TryRestartUnit",
        &(unit, "replace"),
    )?;
    Ok(())
}
