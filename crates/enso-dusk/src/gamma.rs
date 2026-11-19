//! The screens' colour, through `wlr-gamma-control`: for each output, a
//! ramp per channel, held for as long as this program lives — the
//! compositor puts the screen back as it was the moment the program goes.

use std::{
    fs::File,
    io::{Seek, Write},
    os::fd::AsFd,
};

use rustix::fs::{MemfdFlags, memfd_create};
use smithay_client_toolkit::reexports::{
    client::{
        Connection, Dispatch, Proxy, QueueHandle,
        globals::GlobalListContents,
        protocol::{
            wl_output::WlOutput,
            wl_registry::{self, WlRegistry},
        },
    },
    protocols_wlr::gamma_control::v1::client::{
        zwlr_gamma_control_manager_v1::ZwlrGammaControlManagerV1,
        zwlr_gamma_control_v1::{self, ZwlrGammaControlV1},
    },
};

use crate::Dusk;

/// One screen's gamma.
#[derive(Debug)]
pub struct Screen {
    /// The registry's name for its output, to know it when it goes.
    global: u32,
    output: WlOutput,
    control: ZwlrGammaControlV1,
    /// Entries per ramp, once the compositor has said.
    size: Option<u32>,
}

/// Every screen's gamma, set alike.
#[derive(Debug, Default)]
pub struct Gamma {
    manager: Option<ZwlrGammaControlManagerV1>,
    screens: Vec<Screen>,
    /// What every screen is given: red, green, blue.
    channels: [f64; 3],
}

impl Gamma {
    pub fn new(manager: ZwlrGammaControlManagerV1) -> Self {
        Self {
            manager: Some(manager),
            screens: Vec::new(),
            channels: [1.0; 3],
        }
    }

    /// Take charge of an output's gamma.
    pub fn add(&mut self, global: u32, output: WlOutput, qh: &QueueHandle<Dusk>) {
        let Some(manager) = &self.manager else {
            return;
        };
        let control = manager.get_gamma_control(&output, qh, global);
        self.screens.push(Screen {
            global,
            output,
            control,
            size: None,
        });
    }

    /// An output went away.
    pub fn remove(&mut self, global: u32) {
        self.screens.retain(|screen| {
            if screen.global == global {
                screen.control.destroy();
                if screen.output.version() >= 3 {
                    screen.output.release();
                }
                false
            } else {
                true
            }
        });
    }

    /// Give every screen these channels.
    pub fn set(&mut self, channels: [f64; 3]) {
        self.channels = channels;
        for screen in &self.screens {
            if let Some(size) = screen.size
                && let Err(err) = apply(&screen.control, size, channels)
            {
                eprintln!("enso-dusk: cannot set the gamma: {err}");
            }
        }
    }

    fn sized(&mut self, global: u32, size: u32) {
        let channels = self.channels;
        if let Some(screen) = self.screens.iter_mut().find(|s| s.global == global) {
            screen.size = Some(size);
            if let Err(err) = apply(&screen.control, size, channels) {
                eprintln!("enso-dusk: cannot set the gamma: {err}");
            }
        }
    }

    fn failed(&mut self, global: u32) {
        eprintln!("enso-dusk: another program holds a screen's gamma; leaving it be");
        self.screens.retain(|screen| screen.global != global);
    }
}

/// A screen's ramps, written to a memory file and handed over.
fn apply(control: &ZwlrGammaControlV1, size: u32, channels: [f64; 3]) -> std::io::Result<()> {
    let size = size.max(2) as usize;
    let mut bytes = Vec::with_capacity(size * 3 * 2);
    for channel in channels {
        for i in 0..size {
            let level = (i as f64 / (size - 1) as f64 * channel * 65535.0).round();
            bytes.extend_from_slice(&(level.clamp(0.0, 65535.0) as u16).to_ne_bytes());
        }
    }
    let fd = memfd_create("enso-dusk-gamma", MemfdFlags::CLOEXEC)?;
    let mut file = File::from(fd);
    file.write_all(&bytes)?;
    file.rewind()?;
    control.set_gamma(file.as_fd());
    Ok(())
}

impl Dispatch<ZwlrGammaControlV1, u32> for Dusk {
    fn event(
        dusk: &mut Self,
        _: &ZwlrGammaControlV1,
        event: zwlr_gamma_control_v1::Event,
        global: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_gamma_control_v1::Event::GammaSize { size } => dusk.gamma.sized(*global, size),
            zwlr_gamma_control_v1::Event::Failed => dusk.gamma.failed(*global),
            _ => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Dusk {
    fn event(
        dusk: &mut Self,
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
            } if interface == WlOutput::interface().name => {
                let output: WlOutput = registry.bind(name, version.min(4), qh, ());
                dusk.gamma.add(name, output, qh);
            }
            wl_registry::Event::GlobalRemove { name } => dusk.gamma.remove(name),
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for Dusk {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: <WlOutput as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrGammaControlManagerV1, ()> for Dusk {
    fn event(
        _: &mut Self,
        _: &ZwlrGammaControlManagerV1,
        _: <ZwlrGammaControlManagerV1 as Proxy>::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
