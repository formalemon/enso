# enso

A small, pure-Rust desktop suite for the [Niri](https://github.com/YaLTeR/niri)
Wayland compositor: a bar, a control center, a launcher, and a notification
daemon that share one palette and one way of moving.

| Tool | What it is | State |
| --- | --- | --- |
| `enso-bar` | Status bar: windows and workspaces (Niri IPC), clock, network, volume, battery, notification bell | working |
| `enso-launch` | Keyboard-first launcher for applications and files: a bare search field until you type | working |
| `enso-notify` | Freedesktop notification daemon with gliding toasts: actions, pictures, level bars, and a history | working |
| `enso-ctl` | Control center panel + CLI: Wi-Fi, Bluetooth, sound, brightness, power, theme | working |

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
    Mod+D { spawn "enso-launch"; }   // press again to close
    Mod+C { spawn "enso-ctl"; }

    XF86MonBrightnessUp   allow-when-locked=true { spawn "enso-ctl" "brightness" "up"; }
    XF86MonBrightnessDown allow-when-locked=true { spawn "enso-ctl" "brightness" "down"; }
    XF86AudioRaiseVolume  allow-when-locked=true { spawn "enso-ctl" "volume" "up"; }
    XF86AudioLowerVolume  allow-when-locked=true { spawn "enso-ctl" "volume" "down"; }
    XF86AudioMute         allow-when-locked=true { spawn "enso-ctl" "volume" "toggle"; }
}
```

With `enso-bar` and `enso-ctl` installed, resting the pointer on the bar's
right island opens the control center beneath it; with `enso-notify`
running, clicking the island opens the notification history there instead.

Icons are Nerd Font glyphs. Without a Nerd Font installed the tools fall back
to short text labels; on Arch, `ttf-nerd-fonts-symbols` is enough.

## Layout

```
crates/
  enso-core/     palette, themes, config, motion + style tokens   (serde, toml)
    themes/      built-in themes; every *.toml here is embedded at build time
  enso-ui/       layer-shell runtime, software canvas, text, input
  enso-sys/      direct system interfaces: /sys, iwd, BlueZ, logind
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

| Module | Speaks to | For | Feature |
| --- | --- | --- | --- |
| `power` | `/sys` | battery, power profile, temperature, fans, backlight levels | always on, no dependencies |
| `wifi` | iwd (`net.connman.iwd`) | radio, scan, networks, connect with a passphrase agent, forget | `iwd` |
| `bluetooth` | BlueZ (`org.bluez`) | radio, discovery, pair, connect, battery levels | `bluez` |
| `logind` | `org.freedesktop.login1` | setting brightness without root | `logind` |
| `audio` | PipeWire, over its PulseAudio socket | outputs and microphones (which, level, mute) and each application's volume | `audio` |
| `net` | `/sys`, `/proc` | online by cable or Wi-Fi, and the signal | always on, no dependencies |

The service backends are opt-in features, so a tool links only the clients
it uses: `enso-bar` takes `audio` for its volume glyph and no D-Bus at all.
Each takes a complete snapshot of its service (`state()`), blocks while
acting, and has a `watch()` that reports when a new snapshot is worth taking.
An interface keeps them on worker threads; `enso-ctl`'s `system` module is
the pattern.

Sound goes through the PulseAudio protocol that PipeWire serves
(`pipewire-pulse`), spoken in pure Rust. It has exactly a mixer's model — an
output, the applications playing, a volume and a mute each — and leaves the
mapping onto devices and routes to PipeWire. PipeWire's own Rust client still
loads C plugins, which this suite does not do.

### Choices

- **No C libraries.** Wayland protocol via `smithay-client-toolkit` without
  libwayland, keyboard layouts via `kbvm` instead of libxkbcommon, drawing via
  `tiny-skia`, text via `cosmic-text`, D-Bus via `zbus`. Release binaries link
  only libc.
- **CPU rendering.** The surfaces are small and mostly static; a software
  canvas keeps the dependency tree and memory use far below a GPU toolkit.
- **Lean per-tool dependencies.** `zbus` is only in `enso-notify` and
  `enso-ctl`, `niri-ipc` and `jiff` only in `enso-bar`. CLI parsing is
  `lexopt`.
- **`unsafe` is forbidden** across the workspace.

## The tools up close

**`enso-bar`** — the left island is about where you are. At rest it shows one
dot per window on the current workspace, the focused one stretched into a
pill. Rest the pointer on it (anywhere from the screen corner) and it opens
out to every workspace: each a group of dots, one per window — a count, not
titles — with a soft capsule behind the current one. An empty workspace keeps
a single quiet dot. Click a dot to focus that window, or a group to switch to
that workspace.

The right island is the state of the machine in a few glyphs: network (a
cable, or Wi-Fi with its signal), volume, battery with its charge, and a
bell. Each is there only if the machine has it — a desktop shows no battery —
except the bell, which always is, so there is always an island to rest on.
The bell brightens and grows a dot when notifications went by unattended.

The island warms a shade under the pointer, and does three things:

- **Rest** on it for a moment and the control center opens beneath it,
  closing again once the pointer is on neither. The bar runs
  `enso-ctl --dropdown` for this — found next to its own binary, or on
  `PATH`. `control_center = false` under `[bar]` turns it off.
- **Click** it and the notification history opens in the control center's
  place (`enso-notify history`); click again to close it.
- **Scroll** on it to turn the volume, 5 % a notch.

Without `enso-ctl` or `enso-notify` installed the island is simply a readout.

**`enso-ctl`** — one card. The first page is the glance and the quick
controls: battery (with time left), power profile, CPU temperature and fan
speed; a slider each for screen brightness and volume; and a row each for
Wi-Fi, Bluetooth, and the theme, the first two with their on/off switch.

The volume row's speaker is its mute button, and the chevron at its end
leads to the Sound page.

A row opens a page of its own:

- **Wi-Fi** lists the networks in range with their signal. The selected row
  says what choosing it does (`Connect`, `Disconnect`); a new secured network
  asks for its passphrase in a field right under it.
- **Bluetooth** lists paired devices and, while the page is open, looks for
  nearby ones. Choosing a new device pairs, trusts, and connects it. Devices
  that need a PIN typed or confirmed have to be paired once elsewhere.
- **Sound** starts with where sound goes: the output's level, then every
  output there is — laptop speakers, a Bluetooth headset or speaker — with the
  one in use marked; choosing another moves what is playing over to it. Then
  one slider, with its own mute, for every application that is playing (all
  of a browser's tabs count as one). Then the microphone: an on/off switch,
  its level, and the microphones to choose between.
- **Theme** is the picker.

Keys: `j`/`k` or arrows move; `Enter` or `l` opens or chooses; `h`, `Esc`, or
`Backspace` goes back; `h`/`l` adjust a slider; `m` mutes; `Space` flips the
switch of the row or page; `p` cycles the power profile; `q` closes.

Opened from a key binding the panel takes the keyboard. Opened from the bar
it never does — hovering must not steal your typing — except for as long as a
passphrase field is open.

Everything is also a command, for key bindings and scripts:

```sh
enso-ctl wifi                       # radio, connection, networks in range
enso-ctl wifi connect Tea House     # asks for the passphrase if it needs one
enso-ctl wifi off
enso-ctl bluetooth scan
enso-ctl bluetooth connect buds     # a name, the start of one, or an address
enso-ctl brightness up              # 5 % steps; `brightness 40` sets it
enso-ctl volume down                # or `volume 40`, `volume toggle`
enso-ctl volume app Firefox 30      # one application
enso-ctl volume output buds         # play through another device
enso-ctl mic toggle                 # microphone on/off; `mic 70`, `mic input webcam`
enso-ctl profile next
enso-ctl power
```

A passphrase is never taken from the command line, where other users could
read it: it is typed unseen at the terminal, or piped in.

Brightness needs no setup: where `/sys` is read-only, as it normally is,
`enso-ctl` asks logind, which lets the user of the active session set it.
The **power profile** is different — the kernel leaves
`/sys/firmware/acpi/platform_profile` writable by root alone, and until that
changes `enso-ctl` shows the profile but cannot switch it, and says so. No
daemon is needed to change it: `contrib/90-enso-power-profile.rules` is a
udev rule that opens the file to the `wheel` group at boot (the three
commands to install it are at its top).

**`enso-launch`** — opens as a search field and nothing else; matches appear
once you type: applications from the first letter, files from the second,
listed under the applications. `Enter` launches the application or opens the
file with whatever the desktop associates with its type; `Ctrl+Enter` opens
the folder a file is in. Running `enso-launch` while it is open closes it, so
one key binding toggles it. What you chose before ranks a little higher next
time.

Everything is gathered on background threads, so the field is on screen
before the first `.desktop` file is opened, and kept in `~/.cache/enso`
(`apps`, `files`). The next launch answers from those and rescans behind
them, correcting itself (and the files) if something changed — so there are
no invalidation rules to get wrong, and deleting either is always safe.

File search is built to be fast rather than clever:

- The index is one string, a path per line. Reading it back is one read with
  nothing to parse; a search is a linear pass split across the CPU's cores,
  and only the newest query is ever answered.
- The walk is a pool of threads over a shared queue of directories, taking
  each entry's type from the directory listing itself rather than a second
  system call. Links to directories are listed but not followed.
- On this laptop, 180,000 entries walk in about 50 ms with a warm cache and
  a query over them takes 5–8 ms. `enso-launch --find <query>` prints the
  matches with those timings.

What is searched: your home directory and `~/.config` (`file_roots`), less
what nobody goes looking for. Five things decide, each with the say over
the ones after it:

1. **Version-control internals** (`.git`, `.hg`, …) are never listed.
2. **Your list**, `ignore` under `[launch]`. A name or glob matches at any
   depth, a relative path matches the end of a path, and one starting with
   `/` or `~` matches exactly that path. A leading `!` *keeps* something the
   rules below would drop.
3. **Dotfiles** are left out unless `hidden = true`.
4. **`.gitignore` and `.ignore` files** apply below the directory they are
   in, read the way Git reads them — `!` exceptions, `/anchored` paths,
   `dir/` and `**` included — but with no repository required: a
   `.gitignore` in a plain folder counts. The one directly in your home
   directory is the exception, because that belongs to a dotfiles
   repository and usually says `*`. `gitignore = false` turns this off.
5. **The built-in list**, for everything with no ignore file to speak for
   it. `default_ignores = false` turns it off.

```toml
[launch]
ignore = ["*.iso", "Music/incoming", "~/VMs", "!target"]
```

The built-in list (`crates/enso-launch/src/ignores.rs`, drawn from the
[github/gitignore](https://github.com/github/gitignore) templates) has two
kinds of entry, because some names are only sometimes junk:

- **Junk wherever it appears** — `node_modules`, `__pycache__`, `.venv`,
  `cache`, `npm_cache`, `CMakeFiles`, `.gradle`, `*.o`, `*.pyc`, `*.class`,
  editor swap files, and the rest of about a hundred names — is dropped on
  sight.
- **Build output only in the right company** — `target`, `build`, `dist`,
  `out`, `bin`, `obj`, `vendor`, `coverage`, … — is dropped only beside the
  project file that makes it so: `target` next to a `Cargo.toml`, `dist`
  next to a `package.json`, `bin` next to a `.csproj`. Your own `~/bin` or a
  `build` folder of photos is searched like anything else.

A directory that marks itself — a `CACHEDIR.TAG`, or the `pyvenv.cfg` of a
Python virtual environment under any name — is listed but not entered.

Opening a file uses the freedesktop databases directly — the shared MIME
database for the type, `mimeapps.list` and `mimeinfo.cache` for the
application — rather than `xdg-open`. Where nothing is associated, a text
file opens in `$VISUAL`/`$EDITOR` and a folder opens a terminal there.

**`enso-notify`** — toasts live on the overlay layer, above every window
including fullscreen ones, and the surface exists only while something is
showing.

- **Actions.** Clicking a toast opens what it is about, when the sender
  offers that, and dismisses it; other actions are buttons along its bottom
  edge. A right click dismisses without doing anything.
- **Pictures.** Avatars, album art, and thumbnails sent as pixels
  (`image-data`) or named as a PNG file are shown beside the text. Icon-theme
  names are not looked up: the suite carries no icon theme.
- **Level bars.** A notification with a `value` hint draws it as a bar, and
  one tagged `x-canonical-private-synchronous` replaces the last with the
  same tag — together, a volume or brightness pop-up that updates in place:
  `notify-send -h int:value:60 -h string:x-canonical-private-synchronous:volume Volume`.
- **Markup** in the body is understood and shown as plain text.
- **Reading time.** Nothing expires while the pointer is on the toasts.
- **History.** `enso-notify history` — or a click on the bar's bell — opens
  a card where the toasts appear, listing the last 50 notifications, newest
  first. Five show at once and the rest are a scroll away: the list glides
  rather than steps, and instead of a scroll bar it fades toward whichever
  edge has more beyond it. Rows follow the card in one after another; click
  one to remove it and the rest close ranks, or `Clear` them all. `Esc`, a
  second `history`, or moving the pointer away closes it. While it is open
  new notifications arrive in the list rather than as toasts. The history
  lives in the daemon's memory, so it starts empty with each session.

The daemon also keeps a two-line file, `~/.local/state/enso/notifications`
(the unattended count, and whether the history is open). That is all the bar
knows about it — the two share no process and neither needs the other.

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

- **enso-ctl** — Bluetooth pairing that needs a PIN; forgetting networks and devices from the panel (the command
  line can); theme targets beyond enso itself: Niri,
  Alacritty, Neovim (`rose-pine/neovim`), GTK/system colour scheme, Qt,
  Flatpak. `theme set` already runs a list of `Target`s; today the list has
  one entry.
- **enso-launch** — Git's global excludes file and `.git/info/exclude`;
  searching file contents.
- **enso-notify** — acting on a notification from its history row; keeping
  the history across restarts; pictures in formats other than PNG; styled
  (bold, italic) body text.
- **enso-bar** — one bar per output.
- **enso-ui** — fractional scaling, a shaped-text cache.
