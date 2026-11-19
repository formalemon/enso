//! The screens, by the names the compositor gives them (`eDP-1`, `DP-2`):
//! to put a surface on one of them, and to follow them as they are plugged
//! in and out.

use smithay_client_toolkit::reexports::client::{
    Connection, Dispatch, EventQueue, Proxy as _, QueueHandle,
    globals::{GlobalList, GlobalListContents, registry_queue_init},
    protocol::{
        wl_output::{self, WlOutput},
        wl_registry::{self, WlRegistry},
    },
};

use crate::shell::Error;

/// `wl_output` names its screen from version 4.
const VERSION: u32 = 4;

/// The screens seen on one queue: the global's number, the object, and
/// the name once it has come.
#[derive(Default)]
pub(crate) struct Names {
    outputs: Vec<(u32, WlOutput, Option<String>)>,
    /// A screen has finished describing itself since this was last looked at.
    done: bool,
}

impl Names {
    fn named(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .outputs
            .iter()
            .filter_map(|(_, _, name)| name.clone())
            .collect();
        names.sort();
        names
    }
}

impl Dispatch<WlOutput, u32> for Names {
    fn event(
        names: &mut Self,
        _: &WlOutput,
        event: wl_output::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Name { name } => {
                if let Some(entry) = names.outputs.iter_mut().find(|(g, ..)| g == global) {
                    entry.2 = Some(name);
                }
            }
            wl_output::Event::Done => names.done = true,
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Names {
    fn event(
        names: &mut Self,
        registry: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } if interface == "wl_output" => {
                let output = registry.bind::<WlOutput, _, _>(name, version.min(VERSION), qh, name);
                names.outputs.push((name, output, None));
            }
            wl_registry::Event::GlobalRemove { name } => {
                let before = names.outputs.len();
                names.outputs.retain(|(global, output, _)| {
                    let keep = *global != name;
                    if !keep && output.version() >= 3 {
                        output.release();
                    }
                    keep
                });
                names.done |= names.outputs.len() != before;
            }
            _ => {}
        }
    }
}

/// The screen called `wanted`, looked up on a queue of its own so that any
/// runtime can use it: the object may be passed in requests on any queue.
/// The queue comes back too, to be kept as long as the object is used.
pub(crate) fn find(
    conn: &Connection,
    globals: &GlobalList,
    wanted: &str,
) -> Option<(WlOutput, EventQueue<Names>)> {
    let mut queue = conn.new_event_queue::<Names>();
    let qh = queue.handle();
    let mut names = Names::default();
    let found: Vec<(u32, u32)> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|global| global.interface == "wl_output")
            .map(|global| (global.name, global.version))
            .collect()
    });
    for (name, version) in found {
        let output = globals
            .registry()
            .bind::<WlOutput, _, _>(name, version.min(VERSION), &qh, name);
        names.outputs.push((name, output, None));
    }
    queue.roundtrip(&mut names).ok()?;
    let output = names
        .outputs
        .into_iter()
        .find(|(_, _, name)| name.as_deref() == Some(wanted))
        .map(|(_, output, _)| output)?;
    Some((output, queue))
}

/// Follow the screens, calling `on_change` with their names, sorted, at
/// first and whenever one comes or goes. Blocks; returns only if the
/// connection fails.
pub fn watch(mut on_change: impl FnMut(&[String])) -> Result<(), Error> {
    let conn = Connection::connect_to_env()
        .map_err(|err| Error::new("cannot connect to a Wayland compositor", err))?;
    let (globals, mut queue) = registry_queue_init::<Names>(&conn)
        .map_err(|err| Error::new("Wayland registry", err))?;
    let qh = queue.handle();
    let mut names = Names::default();
    let found: Vec<(u32, u32)> = globals.contents().with_list(|list| {
        list.iter()
            .filter(|global| global.interface == "wl_output")
            .map(|global| (global.name, global.version))
            .collect()
    });
    for (name, version) in found {
        let output = globals
            .registry()
            .bind::<WlOutput, _, _>(name, version.min(VERSION), &qh, name);
        names.outputs.push((name, output, None));
    }
    let mut said: Option<Vec<String>> = None;
    loop {
        queue
            .roundtrip(&mut names)
            .map_err(|err| Error::new("Wayland connection", err))?;
        if std::mem::take(&mut names.done) || said.is_none() {
            let now = names.named();
            if said.as_ref() != Some(&now) {
                on_change(&now);
                said = Some(now);
            }
        }
        queue
            .blocking_dispatch(&mut names)
            .map_err(|err| Error::new("Wayland connection", err))?;
        if std::mem::take(&mut names.done) {
            let now = names.named();
            if said.as_ref() != Some(&now) {
                on_change(&now);
                said = Some(now);
            }
        }
    }
}
