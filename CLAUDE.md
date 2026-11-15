# enso

A modular, pure-Rust desktop ecosystem built for the Niri Wayland compositor on a minimal Arch Linux installation (`ren@kiri`).

## What We Are Building

- **`enso-bar`** — Minimal Wayland status bar for Niri.
- **`enso-ctl`** — Control center GUI and CLI for Bluetooth, Wi-Fi, audio, laptop power/thermals, and unified Rosé Pine theming (applying the active palette across `enso` components, Niri, Alacritty, Neovim via `rose-pine/neovim`, GTK/system dark-light color-scheme, Qt, and Flatpak).
- **`enso-launch`** — Fast application launcher and fuzzy file search.
- **`enso-notify`** — Pure-Rust Wayland notification daemon implementing the Freedesktop desktop notifications specification.

## Core Design Principles

1. **Modular & Standalone:** Structured as a single Cargo workspace monorepo, but each tool must work independently. Anyone should be able to install and run just `enso-bar` or `enso-ctl` without needing the rest of the suite or external config files to exist beforehand.
2. **Pure Rust & Direct System Interfaces:** Prioritize pure Rust and direct communication with system sockets, D-Bus (`bluez`, `iwd`, `org.freedesktop.Notifications`), PipeWire, Niri IPC, and `/sys` hardware interfaces. Never shell out to CLI wrappers (`bluetoothctl`, `iwctl`, `wpctl`, `pactl`, `nmcli`).
3. **Extensible Data-Driven Theming:** Themes are defined as standalone `.toml` files (`rose-pine-moon.toml` as the default, alongside `rose-pine-main.toml` and `rose-pine-dawn.toml`). Adding new `.toml` themes must be effortless while keeping built-in defaults available out of the box.
4. **Minimal Overhead:** Keep dependencies lean and avoid pulling heavy toolkit or C-library dependencies into crates that don't need them.

## Aesthetic & Motion Direction

- **Surface Type:** `enso-bar`, `enso-ctl`, `enso-launch`, and `enso-notify` are native Wayland layer-shell GUIs; `enso-ctl` also exposes CLI subcommands (including `enso-ctl theme ...`).
- **Visual Philosophy (Soft Pastel Zen):** Warm, cute, uncluttered minimalism with generous breathing room. Every control and indicator should be self-explanatory at a glance—no crowded widgets, noisy labels, or heavy chrome.
- **Geometry & Icons:** Soft rounded corners (`12px`–`14px` on floating cards, panels, and notification toasts; pill-shaped active indicators) with clean typography and Nerd Font glyphs so binaries need zero external SVG or icon-pack dependencies.
- **Fluid, Purposeful Animation:** Use smooth, natural easing (~180–250ms) rather than abrupt snaps. Notification toasts in `enso-notify` should stack neatly with clean spacing and, when dismissed or expired, glide smoothly upward toward `enso-bar` (optionally updating a subtle notification indicator on the bar when present). Keep panel reveals, toggle switches, and launcher selection highlights equally smooth and responsive.
- **Keyboard-First Interaction:** Floating overlays (`enso-launch`, `enso-ctl`) focus immediately on open, dismiss on `Esc` or click-outside, and support both arrow keys and `Ctrl+n`/`Ctrl+p` or `j`/`k`.

## Workspace Layout

- `crates/enso-core` — palette, themes, config, motion + style tokens. No Wayland; only `serde` + `toml`. Built-in themes live in `crates/enso-core/themes/` and are embedded by `build.rs` (drop a `.toml` in to add one).
- `crates/enso-ui` — the shared toolkit: layer-shell runtime (`App` trait + `run`), software `Canvas`, text, input, `Presence` (overlay reveal/dismiss), `watch_dir` (live theme switching).
- `crates/enso-sys` — direct system interfaces (currently `/sys` power and thermals, zero dependencies). New backends (BlueZ, iwd, PipeWire) go here behind cargo features.
- `crates/enso-{bar,ctl,launch,notify}` — the binaries. Each owns its `[bar]`/`[ctl]`/`[launch]`/`[notify]` config table, read with `Context::section`.

Conventions: take colours, radii, spacing, and durations from `enso_core::{style, motion}` rather than literals; animate with `Tween` + a named `Transition`; `unsafe` is forbidden workspace-wide; keep `config.example.toml` in step with config defaults (tests enforce it).

## Commands

- `cargo test --workspace` — unit tests plus headless render tests, which write PNGs of each interface to `target/snapshots/`.
- `cargo clippy --workspace --all-targets --all-features` and `cargo fmt --all` — both must be clean.
- `cargo run -p enso-ctl -- theme set rose-pine-dawn` — switch theme; running tools follow live.
