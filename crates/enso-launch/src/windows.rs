//! The windows already open, from niri: so typing an application that is
//! running goes to its window instead of opening another, and a window can
//! be found by its title.

use std::{io, thread};

use niri_ipc::{Action, Request, Response, socket::Socket};

/// One open window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    pub app_id: String,
    pub title: String,
    /// When it was last focused, for choosing the most recent of an app's.
    pub focused_at: Option<(u64, u32)>,
}

/// Ask niri for the open windows on a background thread; `ready` gets them
/// (nothing, outside niri).
pub fn load(ready: impl FnOnce(Vec<Window>) + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("enso-windows".to_owned())
        .spawn(move || ready(list().unwrap_or_default()));
    if let Err(err) = spawned {
        eprintln!("enso-launch: cannot ask for the open windows: {err}");
    }
}

fn list() -> io::Result<Vec<Window>> {
    let reply = Socket::connect()?.send(Request::Windows)?;
    let Ok(Response::Windows(windows)) = reply else {
        return Ok(Vec::new());
    };
    Ok(windows
        .into_iter()
        .map(|window| Window {
            id: window.id,
            app_id: window.app_id.unwrap_or_default(),
            title: window.title.unwrap_or_default(),
            focused_at: window
                .focus_timestamp
                .map(|stamp| (stamp.secs, stamp.nanos)),
        })
        .collect())
}

/// The screen with the focus, by name: where the launcher opens. `None`
/// outside niri, which then puts it where it likes.
pub fn focused_output() -> Option<String> {
    let reply = Socket::connect().ok()?.send(Request::FocusedOutput).ok()?;
    match reply {
        Ok(Response::FocusedOutput(output)) => output.map(|output| output.name),
        _ => None,
    }
}

/// Bring a window forward: its workspace, then it.
pub fn focus(id: u64) -> io::Result<()> {
    Socket::connect()?
        .send(Request::Action(Action::FocusWindow { id }))?
        .map(drop)
        .map_err(io::Error::other)
}
