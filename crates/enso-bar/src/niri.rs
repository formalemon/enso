//! Workspace state from Niri's IPC socket.
//!
//! A background thread follows the event stream and reports a fresh snapshot
//! whenever the workspace list changes. Outside Niri the bar simply has no
//! workspace indicator.

use std::{io, thread, time::Duration};

use niri_ipc::{
    Action, Request, Response, WorkspaceReferenceArg,
    socket::Socket,
    state::{EventStreamState, EventStreamStatePart},
};

/// One workspace, reduced to what the bar draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: u64,
    /// Position on its output, starting at 1.
    pub index: u8,
    /// Connector name of the output it is on.
    pub output: Option<String>,
    /// Shown on its output.
    pub active: bool,
    /// Has at least one window.
    pub occupied: bool,
    pub urgent: bool,
}

/// Follow workspace changes on a background thread, calling `on_change` with
/// each new snapshot (ordered by output, then index). Reconnects if Niri
/// restarts; stops when `on_change` returns `false`.
pub fn watch(mut on_change: impl FnMut(Vec<Workspace>) -> bool + Send + 'static) {
    let spawned = thread::Builder::new()
        .name("enso-niri".to_owned())
        .spawn(move || {
            loop {
                match follow(&mut on_change) {
                    Ok(()) => return,
                    // No socket at all means we are not running under Niri.
                    Err(err) if err.kind() == io::ErrorKind::NotFound => return,
                    Err(_) => thread::sleep(Duration::from_secs(2)),
                }
            }
        });
    if let Err(err) = spawned {
        eprintln!("enso-bar: cannot follow Niri workspaces: {err}");
    }
}

/// Stream events until the listener goes away (`Ok`) or the socket fails.
fn follow(on_change: &mut impl FnMut(Vec<Workspace>) -> bool) -> io::Result<()> {
    let mut socket = Socket::connect()?;
    match socket.send(Request::EventStream)? {
        Ok(Response::Handled) => {}
        other => return Err(io::Error::other(format!("unexpected reply: {other:?}"))),
    }

    let mut read_event = socket.read_events();
    let mut state = EventStreamState::default();
    let mut last = None;
    loop {
        state.apply(read_event()?);
        let mut snapshot: Vec<Workspace> = state
            .workspaces
            .workspaces
            .values()
            .map(|ws| Workspace {
                id: ws.id,
                index: ws.idx,
                output: ws.output.clone(),
                active: ws.is_active,
                occupied: ws.active_window_id.is_some(),
                urgent: ws.is_urgent,
            })
            .collect();
        snapshot.sort_by(|a, b| (&a.output, a.index).cmp(&(&b.output, b.index)));
        if last.as_ref() != Some(&snapshot) {
            last = Some(snapshot.clone());
            if !on_change(snapshot) {
                return Ok(());
            }
        }
    }
}

/// Ask Niri to switch to a workspace.
pub fn focus_workspace(id: u64) -> io::Result<()> {
    let action = Action::FocusWorkspace {
        reference: WorkspaceReferenceArg::Id(id),
    };
    Socket::connect()?
        .send(Request::Action(action))?
        .map(drop)
        .map_err(io::Error::other)
}
