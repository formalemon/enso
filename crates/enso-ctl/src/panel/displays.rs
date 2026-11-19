//! The Displays page's dealings with niri: the screens it drives, and
//! turning one on or off, its size and refresh, its scale, and variable
//! refresh. niri applies each at once and keeps it while it runs; to keep
//! it for good, each screen changed here is written whole to
//! `~/.local/state/enso/outputs.kdl`, which niri's config includes ahead
//! of its own outputs (niri reads the first section for a screen).

use std::{io, path::Path};

use niri_ipc::{
    ConfiguredMode, ModeToSet, OutputAction, Request, Response, ScaleToSet, Transform, VrrToSet,
    socket::Socket,
};

/// The file the screens are kept in, in the state directory.
pub const KEPT: &str = "outputs.kdl";

/// The scales offered, smallest to largest.
pub const SCALES: [f64; 7] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0];

/// One screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    /// The connector: `eDP-1`, `DP-2`.
    pub name: String,
    /// What to call it: a laptop's own panel, or the monitor's make and model.
    pub label: String,
    pub on: bool,
    /// Sizes and refresh rates: (width, height, millihertz), largest first.
    pub modes: Vec<(u16, u16, u32)>,
    /// Index into `modes` of the one in use.
    pub mode: Option<usize>,
    pub scale: f64,
    pub vrr_supported: bool,
    pub vrr: bool,
    /// What niri's config calls it: the connector for a built-in screen,
    /// which never moves; make, model, and serial for a monitor, which may
    /// be plugged in anywhere.
    pub kept_as: String,
    /// Where it sits among the others, and how it's turned, while on.
    pub position: (i32, i32),
    pub transform: Option<Transform>,
}

impl Display {
    /// `1920 × 1080 · 144 Hz`.
    pub fn mode_text(&self) -> String {
        self.mode
            .and_then(|index| self.modes.get(index))
            .map_or_else(String::new, |&(w, h, mhz)| {
                format!("{w}×{h} · {} Hz", (f64::from(mhz) / 1000.0).round())
            })
    }

    /// `1.25×`.
    pub fn scale_text(&self) -> String {
        format!("{}×", (self.scale * 100.0).round() / 100.0)
    }

    /// The scale `step` choices along, if there is one.
    pub fn scale_step(&self, step: isize) -> Option<f64> {
        let at = SCALES
            .iter()
            .position(|scale| (scale - self.scale).abs() < 0.01)
            .map_or_else(
                // Somewhere between: start from the nearest below.
                || {
                    SCALES
                        .iter()
                        .rposition(|scale| *scale < self.scale)
                        .unwrap_or(0) as isize
                        + isize::from(step < 0)
                },
                |at| at as isize,
            );
        let to = at + step;
        usize::try_from(to)
            .ok()
            .and_then(|to| SCALES.get(to))
            .copied()
    }

    /// The mode `step` along the list, if there is one.
    pub fn mode_step(&self, step: isize) -> Option<(u16, u16, u32)> {
        let to = self.mode? as isize + step;
        usize::try_from(to)
            .ok()
            .and_then(|to| self.modes.get(to))
            .copied()
    }
}

/// The screens niri drives, the built-in one first.
pub fn read() -> io::Result<Vec<Display>> {
    let reply = Socket::connect()?.send(Request::Outputs)?;
    let Ok(Response::Outputs(outputs)) = reply else {
        return Ok(Vec::new());
    };
    let mut displays: Vec<Display> = outputs
        .into_values()
        .map(|output| {
            let built_in = output.name.starts_with("eDP") || output.name.starts_with("LVDS");
            let label = if built_in {
                "Built-in screen".to_owned()
            } else {
                let name = format!("{} {}", output.make.trim(), output.model.trim());
                let name = name.trim();
                if name.is_empty() {
                    output.name.clone()
                } else {
                    name.to_owned()
                }
            };
            // Largest first, then fastest; the one in use found again after.
            let current = output
                .current_mode
                .and_then(|index| output.modes.get(index))
                .copied();
            let mut modes: Vec<(u16, u16, u32)> = output
                .modes
                .iter()
                .map(|mode| (mode.width, mode.height, mode.refresh_rate))
                .collect();
            modes.sort_by(|a, b| {
                (u32::from(b.0) * u32::from(b.1), b.2).cmp(&(u32::from(a.0) * u32::from(a.1), a.2))
            });
            modes.dedup();
            let mode = current.and_then(|current| {
                modes
                    .iter()
                    .position(|m| *m == (current.width, current.height, current.refresh_rate))
            });
            let kept_as = if built_in || output.make.trim().is_empty() {
                output.name.clone()
            } else {
                format!(
                    "{} {} {}",
                    output.make.trim(),
                    output.model.trim(),
                    output.serial.as_deref().unwrap_or("Unknown").trim()
                )
            };
            Display {
                on: output.logical.is_some(),
                scale: output.logical.map_or(1.0, |logical| logical.scale),
                position: output.logical.map_or((0, 0), |logical| (logical.x, logical.y)),
                transform: output.logical.map(|logical| logical.transform),
                kept_as,
                label,
                modes,
                mode,
                vrr_supported: output.vrr_supported,
                vrr: output.vrr_enabled,
                name: output.name,
            }
        })
        .collect();
    displays.sort_by_key(|display| (display.label != "Built-in screen", display.name.clone()));
    Ok(displays)
}

/// The screen as niri's config would say it, whole: a section of its own
/// replaces the one in `outputs.kdl`, so it says everything.
fn section(display: &Display) -> String {
    let mut lines = vec![format!("output \"{}\" {{", display.kept_as.replace('"', "\\\""))];
    if display.on {
        if let Some(&(width, height, mhz)) = display.mode.and_then(|at| display.modes.get(at)) {
            lines.push(format!(
                "    mode \"{width}x{height}@{:.3}\"",
                f64::from(mhz) / 1000.0
            ));
        }
        lines.push(format!("    scale {}", (display.scale * 1000.0).round() / 1000.0));
        let (x, y) = display.position;
        lines.push(format!("    position x={x} y={y}"));
        let transform = match display.transform {
            Some(Transform::_90) => "90",
            Some(Transform::_180) => "180",
            Some(Transform::_270) => "270",
            Some(Transform::Flipped) => "flipped",
            Some(Transform::Flipped90) => "flipped-90",
            Some(Transform::Flipped180) => "flipped-180",
            Some(Transform::Flipped270) => "flipped-270",
            Some(Transform::Normal) | None => "normal",
        };
        lines.push(format!("    transform \"{transform}\""));
        if display.vrr {
            lines.push("    variable-refresh-rate".to_owned());
        }
    } else {
        lines.push("    off".to_owned());
    }
    lines.push("}".to_owned());
    lines.join("\n")
}

/// `text` (a kept file) with `display`'s section put in, in place of the
/// one it had: the sections of screens not plugged in now stay.
pub fn keeping(text: &str, display: &Display) -> String {
    let opening = format!("output \"{}\" {{", display.kept_as.replace('"', "\\\""));
    let mut sections: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        if let Some(section) = &mut current {
            section.push('\n');
            section.push_str(line);
            if line == "}" {
                sections.extend(current.take());
            }
        } else if line.starts_with("output \"") {
            current = Some(line.to_owned());
        }
    }
    let mine = section(display);
    match sections.iter().position(|section| section.starts_with(&opening)) {
        Some(at) => sections[at] = mine,
        None => sections.push(mine),
    }
    let mut out = String::from(
        "// Written by enso-ctl: the screens as its Displays page left them.\n\
         // niri reads these before its own outputs.kdl, so a screen here is\n\
         // set from here; remove its section (or this file) to go back.\n",
    );
    for section in sections {
        out.push('\n');
        out.push_str(&section);
        out.push('\n');
    }
    out
}

/// Keep `display` as it is now in `dir`'s file, for when niri starts.
pub fn keep(dir: &Path, display: &Display) -> io::Result<()> {
    let path = dir.join(KEPT);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    enso_core::disk::write_atomic(&path, keeping(&text, display).as_bytes(), enso_core::disk::SHARED)
}

fn apply(output: &str, action: OutputAction) -> Result<(), String> {
    let mut socket = Socket::connect().map_err(|err| format!("can't reach niri: {err}"))?;
    let reply = socket.send(Request::Output {
        output: output.to_owned(),
        action,
    });
    match reply {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(err)) => Err(err),
        Err(err) => Err(format!("can't reach niri: {err}")),
    }
}

pub fn set_on(output: &str, on: bool) -> Result<(), String> {
    apply(
        output,
        if on {
            OutputAction::On
        } else {
            OutputAction::Off
        },
    )
}

pub fn set_mode(output: &str, (width, height, mhz): (u16, u16, u32)) -> Result<(), String> {
    apply(
        output,
        OutputAction::Mode {
            mode: ModeToSet::Specific(ConfiguredMode {
                width,
                height,
                refresh: Some(f64::from(mhz) / 1000.0),
            }),
        },
    )
}

pub fn set_scale(output: &str, scale: f64) -> Result<(), String> {
    apply(
        output,
        OutputAction::Scale {
            scale: ScaleToSet::Specific(scale),
        },
    )
}

pub fn set_vrr(output: &str, vrr: bool) -> Result<(), String> {
    apply(
        output,
        OutputAction::Vrr {
            vrr: VrrToSet {
                vrr,
                on_demand: false,
            },
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laptop() -> Display {
        Display {
            name: "eDP-1".into(),
            label: "Built-in screen".into(),
            on: true,
            modes: vec![
                (1920, 1080, 144_003),
                (1920, 1080, 60_004),
                (1280, 720, 60_000),
            ],
            mode: Some(0),
            scale: 1.0,
            vrr_supported: true,
            vrr: false,
            kept_as: "eDP-1".into(),
            position: (0, 0),
            transform: Some(Transform::Normal),
        }
    }

    #[test]
    fn keeps_each_screen_whole_and_the_others_as_they_were() {
        let mut laptop = laptop();
        laptop.scale = 1.25;
        let monitor = Display {
            name: "DP-2".into(),
            label: "Dell U2720Q".into(),
            kept_as: "Dell Inc. DELL U2720Q ABC123".into(),
            on: false,
            ..self::laptop()
        };
        let once = keeping("", &laptop);
        assert_eq!(
            once.lines().filter(|l| !l.starts_with("//")).collect::<Vec<_>>().join("\n"),
            "\noutput \"eDP-1\" {\n    mode \"1920x1080@144.003\"\n    scale 1.25\n    \
             position x=0 y=0\n    transform \"normal\"\n}"
        );
        let both = keeping(&once, &monitor);
        assert!(both.contains("output \"Dell Inc. DELL U2720Q ABC123\" {\n    off\n}"));
        // Changed again, a screen's section is replaced, not added to.
        laptop.vrr = true;
        let again = keeping(&both, &laptop);
        assert_eq!(again.matches("output \"eDP-1\"").count(), 1);
        assert!(again.contains("variable-refresh-rate"));
        assert!(again.contains("DELL U2720Q"));
    }

    #[test]
    fn steps_through_scales_and_modes() {
        let mut display = laptop();
        assert_eq!(display.mode_text(), "1920×1080 · 144 Hz");
        assert_eq!(display.scale_text(), "1×");
        assert_eq!(display.scale_step(1), Some(1.25));
        assert_eq!(display.scale_step(-1), None);
        display.scale = 1.6;
        assert_eq!(display.scale_step(1), Some(1.75));
        assert_eq!(display.scale_step(-1), Some(1.5));
        assert_eq!(display.mode_step(1), Some((1920, 1080, 60_004)));
        assert_eq!(display.mode_step(-1), None);
    }
}
