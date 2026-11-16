# enso

A small, pure-Rust desktop suite for the [Niri](https://github.com/YaLTeR/niri)
Wayland compositor: a bar, a control center, a launcher, and a notification
daemon that share one palette and one way of moving.

| Tool | What it is | State |
| --- | --- | --- |
| `enso-bar` | Status bar: windows and workspaces (Niri IPC), clock, battery | working |
| `enso-launch` | Keyboard-first application launcher: a bare search field until you type | apps working; file search planned |
| `enso-notify` | Freedesktop notification daemon with gliding toasts | working (text notifications) |
| `enso-ctl` | Control center panel + CLI | theming and power readout working; Wi-Fi, Bluetooth, audio planned |

This is the initial scaffold: the architecture, shared libraries, and a thin
working slice of every tool. See [Roadmap](#roadmap) for what is not built yet.

## Quick start

```sh
cargo build --release            # everything
cargo build --release -p enso-bar   # or just one tool

./target/release/enso-bar &
./target/release/enso-notify &
./target/release/enso-launch
./target/release/enso-ctl                    # panel
./target/release/enso-ctl theme set rose-pine-dawn   # running tools recolour instantly
```

Each tool is standalone: no config file, theme file, or other enso tool needs
to exist. To install one: `cargo install --path crates/enso-bar`.

Hooking them into Niri (`~/.config/niri/config.kdl`):

```kdl
spawn-at-startup "enso-bar"
spawn-at-startup "enso-notify"

binds {
    Mod+D { spawn "enso-launch"; }
    Mod+C { spawn "enso-ctl"; }
}
```

Icons are Nerd Font glyphs. Without a Nerd Font installed the tools fall back
to short text labels; on Arch, `ttf-nerd-fonts-symbols` is enough.

## Layout

```
crates/
  enso-core/     palette, themes, config, motion + style tokens   (serde, toml)
    themes/      built-in themes; every *.toml here is embedded at build time
  enso-ui/       layer-shell runtime, software canvas, text, input
  enso-sys/      direct system interfaces: /sys power + thermals  (no dependencies)
  enso-bar/  enso-ctl/  enso-launch/  enso-notify/               the four binaries
config.example.toml   every config key with its default
```

**`enso-core`** knows nothing about Wayland. `Context::load()` resolves
config, themes, and motion preferences in one call and never fails — a broken
config file becomes a warning and defaults, not a dead desktop.

**`enso-ui`** is the whole toolkit: implement the `App` trait (`surface`,
`event`, `draw`) and call `run`. The runtime owns the layer-shell surface,
buffers, HiDPI scale, keyboard layouts and key repeat, pointer, and frame
pacing; it redraws only when asked and paces animations with frame callbacks,
so an idle surface costs nothing. `Offscreen::render` draws any app without a
compositor, which is how the interfaces are snapshot-tested.

**`enso-sys`** is where system backends live. The rule: talk to the kernel or
the service itself (`/sys`, sockets, D-Bus), never to a CLI wrapper.

### Choices

- **No C libraries.** Wayland protocol via `smithay-client-toolkit` without
  libwayland, keyboard layouts via `kbvm` instead of libxkbcommon, drawing via
  `tiny-skia`, text via `cosmic-text`, D-Bus via `zbus`. Release binaries link
  only libc.
- **CPU rendering.** The surfaces are small and mostly static; a software
  canvas keeps the dependency tree and memory use far below a GPU toolkit.
- **Lean per-tool dependencies.** `zbus` is only in `enso-notify`, `niri-ipc`
  and `jiff` only in `enso-bar`. CLI parsing is `lexopt`.
- **`unsafe` is forbidden** across the workspace.

## The tools up close

**`enso-bar`** — the left island is about where you are. At rest it shows one
dot per window on the current workspace, the focused one stretched into a
pill. Rest the pointer on it (anywhere from the screen corner) and it opens
out to every workspace: each a group of dots, one per window — a count, not
titles — with a soft capsule behind the current one. An empty workspace keeps
a single quiet dot. Click a dot to focus that window, or a group to switch to
that workspace.

**`enso-launch`** — opens as a search field and nothing else; matches appear
once you type. Applications are read on a background thread, so the field is
on screen before the first `.desktop` file is opened, and the result is kept
in `~/.cache/enso/apps`. The next launch answers from that one small file and
rescans behind it, correcting itself (and the file) if something was
installed, removed, or edited — so there are no invalidation rules to get
wrong, and deleting the file is always safe.

**`enso-notify`** — toasts live on the overlay layer, above every window
including fullscreen ones, and the surface exists only while something is
showing.

## Theming

A theme is a name, a variant, and fifteen colour roles:

```toml
name = "Rosé Pine Moon"
variant = "dark"            # or "light"

[colors]
base = "#232136"
surface = "#2a273f"
# overlay, muted, subtle, text, love, gold, rose, pine, foam, iris,
# highlight-low, highlight-med, highlight-high
```

- Built-ins (`rose-pine-moon` — the default — `rose-pine-main`,
  `rose-pine-dawn`) are compiled into every binary.
- Drop `<id>.toml` into `~/.config/enso/themes/` to add a theme, or to
  override a built-in with the same id. The file name is the theme id.
- To add a built-in, drop the file into `crates/enso-core/themes/`; the build
  script embeds it.

```sh
enso-ctl theme list
enso-ctl theme set rose-pine-dawn
enso-ctl theme next
enso-ctl theme show rose-pine-main
```

`theme set` records the choice in `~/.local/state/enso/theme`. Running tools
watch that directory (inotify) and recolour immediately. The active theme is,
in order: the recorded choice, `theme` in `config.toml`, the default.

## Design language

All of it lives in `enso-core` as tokens, so tools never hard-code a look:

- `style` — radii (14 px cards, 12 px panels, pills), spacing scale, type
  scale, shadows, and the icon vocabulary.
- `motion` — easing curves, named transitions (`SELECT` 180 ms, `TOGGLE`
  200 ms, `REVEAL` 220 ms, `DISMISS` 250 ms), and `Tween`, a value that glides
  to each new target and retargets smoothly mid-flight.
- `input` (in `enso-ui`) — the shared navigation keys: arrows,
  `Ctrl+n`/`Ctrl+p`, and `j`/`k` where no text field has focus. Overlays
  focus on open and dismiss on `Esc` or a click outside.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets --all-features
cargo fmt --all
```

Render tests write PNGs of each interface to `target/snapshots/` — the
quickest way to see a visual change without restarting anything.

## Roadmap

- **enso-ctl** — Wi-Fi (iwd), Bluetooth (BlueZ), audio (PipeWire), writable
  power profile and backlight; theme targets beyond enso itself: Niri,
  Alacritty, Neovim (`rose-pine/neovim`), GTK/system colour scheme, Qt,
  Flatpak. `theme set` already runs a list of `Target`s; today the list has
  one entry.
- **enso-launch** — fuzzy file search, frecency ranking, toggle on second
  invocation.
- **enso-notify** — actions, body markup, images, pause-on-hover, and a
  notification indicator on the bar.
- **enso-bar** — one bar per output, more status modules (network, audio).
- **enso-ui** — fractional scaling, clipping (for animated scrolling), a
  shaped-text cache.
