//! Workspace and window state from Niri's IPC socket.
//!
//! A background thread follows the event stream and reports a fresh snapshot
//! whenever something the bar draws changes. Outside Niri the bar simply has
//! no workspace island.

use std::{io, thread, time::Duration};

use niri_ipc::{
    Action, Event, Request, Response, WorkspaceReferenceArg,
    socket::Socket,
    state::{EventStreamState, EventStreamStatePart},
};

/// One window, reduced to what the bar draws: no title, no app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub id: u64,
    /// The window its workspace focuses (the focused one, on the workspace
    /// being looked at).
    pub active: bool,
    pub urgent: bool,
}

/// One workspace and the windows on it, in layout order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Workspace {
    pub id: u64,
    /// Position on its output, starting at 1.
    pub index: u8,
    /// Connector name of the output it is on.
    pub output: Option<String>,
    /// Shown on its output.
    pub active: bool,
    pub windows: Vec<Window>,
}

/// Follow workspace and window changes on a background thread, calling
/// `on_change` with each new snapshot (ordered by output, then index).
/// Reconnects if Niri restarts; stops when `on_change` returns `false`.
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
    let mut have_windows = false;
    let mut last = None;
    loop {
        let event = read_event()?;
        have_windows |= matches!(event, Event::WindowsChanged { .. });
        state.apply(event);
        // Niri opens the stream with the workspaces and only then the
        // windows; reporting in between would show every workspace as empty
        // for a moment.
        if !have_windows {
            continue;
        }
        let snapshot = snapshot(
            state.workspaces.workspaces.values(),
            state.windows.windows.values(),
        );
        // Most events (titles, sizes, focus timestamps) change nothing the
        // bar shows, and so cost it nothing.
        if last.as_ref() != Some(&snapshot) {
            last = Some(snapshot.clone());
            if !on_change(snapshot) {
                return Ok(());
            }
        }
    }
}

/// Reduce Niri's state to the bar's view of it: workspaces ordered by output
/// then index, each with its windows left to right (columns, then rows within
/// a column), floating windows last.
fn snapshot<'a>(
    workspaces: impl Iterator<Item = &'a niri_ipc::Workspace>,
    windows: impl Iterator<Item = &'a niri_ipc::Window>,
) -> Vec<Workspace> {
    let mut windows: Vec<&niri_ipc::Window> = windows.collect();
    windows.sort_by_key(|window| {
        let tiled = window.layout.pos_in_scrolling_layout;
        (tiled.is_none(), tiled, window.id)
    });

    let mut snapshot: Vec<Workspace> = workspaces
        .map(|ws| Workspace {
            id: ws.id,
            index: ws.idx,
            output: ws.output.clone(),
            active: ws.is_active,
            windows: windows
                .iter()
                .filter(|window| window.workspace_id == Some(ws.id))
                .map(|window| Window {
                    id: window.id,
                    active: ws.active_window_id == Some(window.id),
                    urgent: window.is_urgent,
                })
                .collect(),
        })
        .collect();
    snapshot.sort_by(|a, b| (&a.output, a.index).cmp(&(&b.output, b.index)));
    snapshot
}

/// Ask Niri to switch to a workspace.
pub fn focus_workspace(id: u64) -> io::Result<()> {
    act(Action::FocusWorkspace {
        reference: WorkspaceReferenceArg::Id(id),
    })
}

/// Ask Niri to focus a window, switching to its workspace if needed.
pub fn focus_window(id: u64) -> io::Result<()> {
    act(Action::FocusWindow { id })
}

fn act(action: Action) -> io::Result<()> {
    Socket::connect()?
        .send(Request::Action(action))?
        .map(drop)
        .map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use niri_ipc::WindowLayout;

    use super::*;

    fn workspace(id: u64, idx: u8, active_window_id: Option<u64>) -> niri_ipc::Workspace {
        niri_ipc::Workspace {
            id,
            idx,
            name: None,
            output: Some("eDP-1".into()),
            is_urgent: false,
            is_active: idx == 1,
            is_focused: idx == 1,
            active_window_id,
        }
    }

    fn window(id: u64, workspace: u64, column: Option<usize>) -> niri_ipc::Window {
        niri_ipc::Window {
            id,
            title: Some(format!("window {id}")),
            app_id: None,
            pid: None,
            workspace_id: Some(workspace),
            is_focused: false,
            is_floating: column.is_none(),
            is_urgent: id == 7,
            layout: WindowLayout {
                pos_in_scrolling_layout: column.map(|column| (column, 1)),
                tile_size: (0.0, 0.0),
                window_size: (0, 0),
                tile_pos_in_workspace_view: None,
                window_offset_in_tile: (0.0, 0.0),
            },
            focus_timestamp: None,
        }
    }

    #[test]
    fn snapshot_groups_windows_by_workspace_in_layout_order() {
        // Deliberately out of order, as a hash map would hand them over.
        let workspaces = [workspace(30, 2, Some(7)), workspace(10, 1, Some(2))];
        let windows = [
            window(9, 10, None),
            window(2, 10, Some(3)),
            window(7, 30, Some(1)),
            window(5, 10, Some(1)),
        ];
        let snapshot = snapshot(workspaces.iter(), windows.iter());

        let ids = |ws: &Workspace| ws.windows.iter().map(|w| w.id).collect::<Vec<_>>();
        assert_eq!(
            snapshot.iter().map(|ws| ws.id).collect::<Vec<_>>(),
            [10, 30]
        );
        // Tiled windows by column, the floating one last.
        assert_eq!(ids(&snapshot[0]), [5, 2, 9]);
        assert_eq!(ids(&snapshot[1]), [7]);
        assert!(snapshot[0].active && !snapshot[1].active);

        let active = |ws: &Workspace| ws.windows.iter().find(|w| w.active).map(|w| w.id);
        assert_eq!(active(&snapshot[0]), Some(2));
        assert_eq!(active(&snapshot[1]), Some(7));
        assert!(snapshot[1].windows[0].urgent);
    }

    #[test]
    fn title_changes_do_not_change_the_snapshot() {
        let workspaces = [workspace(10, 1, Some(2))];
        let mut windows = [window(2, 10, Some(1))];
        let before = snapshot(workspaces.iter(), windows.iter());
        windows[0].title = Some("something else".into());
        assert_eq!(before, snapshot(workspaces.iter(), windows.iter()));
    }
}
