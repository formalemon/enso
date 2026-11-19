# enso

A small, pure-Rust desktop suite for the [Niri](https://github.com/YaLTeR/niri)
Wayland compositor: a bar, a control center, a launcher, a notification
daemon, a wallpaper, a power menu, a lock screen, a password prompt, a
keyring, a file manager, what's playing, clipboard history, and a night
light — sharing one palette, one set of drawn icons, and one way of moving.

| Tool | What it is | State |
| --- | --- | --- |
| `enso-bar` | Status bar, on every screen: windows and workspaces (Niri IPC), clock, network, battery, notification bell; the level as volume, brightness, or the keyboard light changes, a calendar under the clock, and what's out of the ordinary (sharing the screen, a live microphone, muted, kept awake) | working |
| `enso-launch` | Keyboard-first launcher for applications, files, open windows, sums, and the desktop's own commands: a bare search field until you type | working |
| `enso-notify` | Freedesktop notification daemon with gliding toasts: actions, pictures, level bars, and a history; low-battery warnings, a button for one-time codes, screenshots that open | working |
| `enso-ctl` | Control center panel + CLI: battery, Do not disturb, keep awake, night light, Wi-Fi, Bluetooth, sound, brightness, displays, power, and one theme for the whole desktop — enso, Niri, Alacritty, Neovim, GTK, fonts, the cursor, Firefox, Qt, Flatpak | working |
| `enso-clip` | Clipboard history: what's copied, kept in memory for the session, and a card to choose from (Mod+Ctrl+V) | new |
| `enso-dusk` | Night light: the screens warm at sunset and cool at sunrise, worked out from the timezone; a day and a night theme, if you like | new |
| `enso-wall` | Wallpaper: one picture per theme with a default to fall back on, fading between them as the theme changes | working |
| `enso-power` | Power menu: lock, sleep, log out, restart, shut down — a key each | working |
| `enso-lock` | Lock screen: the ensō, a clock and a password field, on a machine made quiet while nobody is at it | working |
| `enso-greet` | Login screen, as a greeter for greetd: the ensō, your name, and a password; Tab for someone else | working |
| `enso-boot` | Boot menu, for the firmware: Arch or Windows over rice terraces at dusk, in voxels; then the ensō while Arch starts | new |
| `enso-auth` | Asks for your password when the system needs it (polkit: mounting disks, system settings, pkexec), and for codes when pairing Bluetooth devices | working |
| `enso-keyring` | The desktop's keyring (Secret Service) for gh, git, VS Code, Chromium, anything on libsecret: sealed on disk, opened with your login password, never storing it | working |
| `enso-play` | What's playing, under the clock: the cover, a timeline that waves while it plays, and the buttons — any MPRIS player (a YouTube tab, Spotify, mpv); the media keys too | working |
| `enso-view` | Image viewer: the pictures of a folder one at a time — zoom, turn, copy, make the wallpaper, throw away; what pictures open in | new |
| `enso-files` | File manager, and the file picker for every app (behind the desktop portal), and what folders open in: tabs and a split view, search by name and inside files, previews and thumbnails, archives, git with GitHub and GitLab, drag and drop — keyboard and mouse alike | working |

What's not built yet is under [Roadmap](#roadmap); what changed in each
version is in its [release notes](https://github.com/formalemon/enso/releases).

## Install

Every binary links only the C library — no Wayland, font, or image
libraries to install — so one build runs on any x86-64 Linux with glibc.

**From a release.** Each [release](https://github.com/formalemon/enso/releases)
carries `enso-<version>-x86_64-linux.tar.gz`, every tool in one folder, and
its SHA-256:

```sh
curl -LO https://github.com/formalemon/enso/releases/download/v0.2.0/enso-v0.2.0-x86_64-linux.tar.gz
curl -LO https://github.com/formalemon/enso/releases/download/v0.2.0/enso-v0.2.0-x86_64-linux.tar.gz.sha256
sha256sum -c enso-v0.2.0-x86_64-linux.tar.gz.sha256
tar xf enso-v0.2.0-x86_64-linux.tar.gz
cp enso-v0.2.0-x86_64-linux/enso-* ~/.local/bin/
```

**From source**, with Rust 1.89 or newer:

```sh
cargo build --release            # everything
cargo build --release -p enso-bar   # or just one tool

./target/release/enso-bar &
./target/release/enso-notify &
./target/release/enso-launch
./target/release/enso-ctl                    # panel
./target/release/enso-ctl theme set rose-pine-dawn   # enso recolours instantly, and the rest of the desktop with it

./target/release/enso-wall &
./target/release/enso-wall set ~/Pictures/quiet.jpg                       # the default
./target/release/enso-wall set ~/Pictures/dawn.png --theme rose-pine-dawn # one theme's own
./target/release/enso-power                  # menu; `enso-power lock` acts at once
./target/release/enso-lock
./target/release/enso-auth &                 # then `enso-auth try password` to see its card
```

Each tool is standalone: no config file, theme file, or other enso tool needs
to exist. To install one: `cargo install --path crates/enso-bar`.

Hooking them into Niri (`~/.config/niri/config.kdl`):

```kdl
spawn-at-startup "enso-bar"
spawn-at-startup "enso-notify"
spawn-at-startup "enso-wall"
spawn-at-startup "enso-auth"
spawn-at-startup "enso-lock" "watch"
spawn-at-startup "enso-clip"
spawn-at-startup "enso-dusk"

binds {
    Mod+Space { spawn "enso-launch"; }   // press again to close
    Mod+Ctrl+V { spawn "enso-clip" "pick"; }   // what was copied
    Mod+C { spawn "enso-ctl"; }
    Mod+P { spawn "enso-play"; }     // what's playing; again to close
    Mod+Shift+E { spawn "enso-power"; }  // then l, p, o, r, or s
    Super+Alt+L { spawn "enso-lock"; }

    XF86MonBrightnessUp   allow-when-locked=true { spawn "enso-ctl" "brightness" "up"; }
    XF86MonBrightnessDown allow-when-locked=true { spawn "enso-ctl" "brightness" "down"; }
    XF86AudioRaiseVolume  allow-when-locked=true { spawn "enso-ctl" "volume" "up"; }
    XF86AudioLowerVolume  allow-when-locked=true { spawn "enso-ctl" "volume" "down"; }
    XF86AudioMute         allow-when-locked=true { spawn "enso-ctl" "volume" "toggle"; }
    XF86AudioPlay         allow-when-locked=true { spawn "enso-play" "play-pause"; }
    XF86AudioNext         allow-when-locked=true { spawn "enso-play" "next"; }
    XF86AudioPrev         allow-when-locked=true { spawn "enso-play" "previous"; }
}
```

Two tools are started by the session bus instead, once told to be:
`enso-keyring enable` makes it the keyring, and `enso-files portal enable`
makes it every application's file chooser. A third runs before the
session: `enso-greet enable` readies the login screen and prints the few
lines root has to run (see [`enso-greet`](#the-tools-up-close)).

With `enso-bar` and `enso-ctl` installed, resting the pointer on the bar's
right island opens the control center beneath it; with `enso-notify`
running, clicking the island opens the notification history there instead.

Icons are enso's own drawings (`enso_ui::figure`), the same in every tool:
one pen, round ends and soft corners, each kept to the middle of a 24-unit
grid so every icon has the same breathing room. They need no icon font —
nothing to install. `icons = "text"` under `[appearance]` puts a few letters
in their place.

## Layout

```
crates/
  enso-core/     palette, themes, config, motion + style tokens, atomic writes
    themes/      built-in themes; every *.toml here is embedded at build time
  enso-ui/       window, layer-shell and per-output runtimes, software canvas,
                 text, input, widgets (fields, forms, menus, scrolling)
  enso-sys/      direct system interfaces: /sys, iwd, BlueZ, logind, dconf,
                 PipeWire, udisks2, accounts
  enso-fs/       listing, file operations, trash, pictures, thumbnails,
                 archives, git, search
  enso-bar/  enso-ctl/  enso-launch/  enso-notify/  enso-wall/
  enso-power/  enso-lock/  enso-auth/  enso-keyring/  enso-files/
  enso-play/  enso-greet/  enso-boot/  enso-clip/  enso-dusk/   the binaries
  enso-scene/    the boot menu's picture: terraces, river, mountains, computed
config.example.toml   every config key with its default
contrib/              udev rules: the power profile, and udisks for wheel
```

**`enso-core`** knows nothing about Wayland. `Context::load()` resolves
config, themes, and motion preferences in one call and never fails — a broken
config file becomes a warning and defaults, not a dead desktop.

**`enso-ui`** is the whole toolkit: implement the `App` trait (`surface`,
`event`, `draw`) and call `run`. The surface is a layer-shell one (a bar, a
toast) or an ordinary window (`SurfaceSpec::window`) that Niri tiles like
any other, and can belong to another program's window, as a file chooser
belongs to the browser that asked for it. The runtime owns the surface,
buffers, scale, keyboard layouts and key repeat, pointer, clipboard, drag
and drop, and frame pacing; it redraws only when asked and paces
animations with frame callbacks, so an idle surface costs nothing.

- **Exact pixels at any scale.** On a screen scaled by 1.25 or 1.5, buffers
  are drawn in its real pixels (`wp_fractional_scale` and `wp_viewporter`),
  not at the next whole scale and shrunk.
- **Input methods.** A focused text field shows what an input method
  (fcitx5, IBus) is composing, underlined at the caret, and the candidates
  open beside it (`zwp_text_input_v3`). It's only engaged while a field has
  focus, and never for a password.
- **Drags show what they carry**, drawn by the app beside the pointer.

`Offscreen::render` draws any app without a compositor, which is how the
interfaces are snapshot-tested.

Two tools are not one surface but one per screen, and have a sibling runtime:
implement `Screens` (`role`, `event`, `draw`) and call `run_screens`. A
`Role::Wallpaper` sits behind everything and takes no input; a `Role::Lock`
is the session lock (`ext-session-lock`), which the compositor enforces and
keeps even if the client dies. The runtime follows monitors coming and going,
and sizes buffers in the output's real pixels — fractional scales included —
so a `Picture` fitted to a screen is shown exactly as fitted.

**`enso-sys`** is where system backends live. The rule: talk to the kernel or
the service itself (`/sys`, sockets, D-Bus), never to a CLI wrapper.

| Module | Speaks to | For | Feature |
| --- | --- | --- | --- |
| `power` | `/sys` | battery, power profile, temperature, fans, backlight levels | always on, no dependencies |
| `wifi` | iwd (`net.connman.iwd`) | radio, scan, networks, connect with a passphrase agent, forget | `iwd` |
| `bluetooth` | BlueZ (`org.bluez`) | radio, discovery, pair, connect, battery levels | `bluez` |
| `logind` | `org.freedesktop.login1` | setting brightness without root; shut down, restart, suspend, end the session | `logind` |
| `dconf` | `ca.desrt.dconf`, on the session bus | writing desktop settings: the dark-or-light preference | `dconf` |
| `auth` | `unix_chkpwd`, PAM's own helper | whether a password is the user's | always on, no dependencies |
| `audio` | PipeWire, over its PulseAudio socket | outputs and microphones (which, level, mute) and each application's volume | `audio` |
| `mpris` | every media player on the session bus (MPRIS) | what's playing and where in it; play, pause, skip, seek | `mpris` |
| `net` | `/sys`, `/proc` | online by cable or Wi-Fi, and the signal | always on, no dependencies |

The service backends are opt-in features, so a tool links only the clients
it uses: `enso-bar` takes `audio` to turn the volume on a scroll, and no D-Bus at all.
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
- **Lean per-tool dependencies.** `zbus` is only in the tools that talk to a
  service (`enso-notify`, `enso-ctl`, and — for logind — `enso-power` and
  `enso-lock`), `niri-ipc` only in those that talk to Niri, the JPEG decoder
  only in the two that show a wallpaper. `enso-wall` links no D-Bus, no
  text shaping it ever runs, and no IPC at all. CLI parsing is `lexopt`.
- **Passwords without PAM.** Linking PAM would be the suite's first C
  library. `enso-lock` instead asks `unix_chkpwd`, the setuid helper PAM
  itself uses to check a password when it is not root. The cost is that
  only the password counts: no fingerprint reader, no `pam_faillock`.
  `enso-auth` doesn't link PAM either: polkit's own helper runs it, so
  there the whole PAM stack applies.
- **Secrets are wiped** after use (`zeroize`): typed passwords and
  passphrases, PAM's questions, keyring secrets, tokens.
- **Files are written whole**: beside the old one, synced, then swapped in
  (`enso_core::disk`), so a crash never leaves half a setting.
- **A rebuild doesn't break a running session.** The lock watcher,
  enso-auth's prompts, and the file picker start themselves again through
  `/proc/self/exe`, so replacing the binaries under them changes nothing
  until they're restarted.
- **`unsafe` is forbidden** across the workspace, but for one module:
  `enso-boot`'s `firmware.rs`, where a boot menu has to take the
  firmware's word (its entry point, and a kernel's command line handed
  over as a pointer). Each use there says why it is sound.

## The tools up close

**`enso-bar`** — the left island is about where you are. At rest it shows one
dot per window on the current workspace, the focused one stretched into a
pill. Rest the pointer on it (anywhere from the screen corner) and it opens
out to every workspace: each a group of dots, one per window — a count, not
titles — with a soft capsule behind the current one. An empty workspace keeps
a single quiet dot. Click a dot to focus that window, or a group to switch to
that workspace.

The right island is the state of the machine at a glance, in icons enso
draws itself rather than takes from a font — thick round strokes, soft
corners, one hand for all of them:

- **Wi-Fi**, a dot and three arcs lit from the inside out with the signal
  (the rest stay, faintly); offline, all faint and struck through. A cable
  when wired.
- **The battery**, a pill holding its percentage, filled from the left as far
  as it is charged — in foam on power, gold below 30 %, love below 15 % — with
  a small lightning bolt before the figures while it charges. Where the fill
  runs under the figures they are cut out of it.
- **The bell**, which rings — a little swing that dies away — when something
  new arrives unattended, and keeps a dot in the accent until it is seen.

Every size is taken from the bar's `height`, so a taller or shorter bar gets
the same island, larger or smaller. The islands let a little of the
wallpaper through (`opacity = 0.9` under `[bar]`; 1.0 is solid); what sits
on them stays solid. And the island, the control center, the
notification history, and toasts all keep the same distance from the side
of the screen, so their right edges line up. Each is there only if the machine has it — a desktop shows no battery —
except the bell, which always is, so there is always an island to rest on.
The bell brightens and grows a dot when notifications went by unattended.

The island warms a shade under the pointer, and does three things:

- **Rest** on it for a moment and the control center opens beneath it,
  closing again once the pointer is on neither. The bar runs
  `enso-ctl --dropdown` for this — found next to its own binary, or on
  `PATH`. `control_center = false` under `[bar]` turns it off.
- **Click** it and the notification history opens in the control center's
  place (`enso-notify history`); click again to close it.

The control center and the history take turns: whichever opens sends the
other away, however each was opened. And there is only ever one control
center — `enso-ctl` keeps to one copy — so `Mod+C` toggles it: pressed while
one is open, from the key or from the bar, it closes it; resting on the bar
while one is open leaves it as it is.
- **Scroll** on it to turn the volume, 5 % a notch.

Without `enso-ctl` or `enso-notify` installed the island is simply a readout.

**A bar on every screen.** `enso-bar` on its own draws nothing: it keeps
one `enso-bar --output NAME` on each screen, starting one as a screen is
plugged in, ending it as it goes, and starting it again if it falls over.
Each shows its own screen's workspaces, and what it opens — the control
center, the calendar, what's playing — comes up on that screen (it hands
`$ENSO_OUTPUT` down). `outputs = ["eDP-1"]` under `[bar]` keeps it to
the screens named.

**The island answers.** Turning the volume — the keys, a scroll on the
island, a headset's buttons, an application — or the brightness, or
muting the microphone, or the keyboard's light, widens the island to the
left into the level: a speaker (a sun, a microphone, a keyboard), a track
filled as far as it goes, and the figure; a moment after the last change it folds back into its icons. The
bar follows the sound server for the volume, and the backlight itself for
the brightness (the kernel announces each new level), so it shows whatever
changed it. The keyboard's light shows when its own keys change it, which
many laptops handle in the firmware.

What's out of the ordinary shows on the island while it lasts, ahead of
the rest, and goes when it's over:

- **Sharing the screen**, in love, while anything records it (niri says);
  a click on it stops the sharing.
- **A microphone in use**, in love, while an application listens to it —
  struck through, quietly, when it listens but the microphone is muted.
- **Muted**: the speaker struck through.
- **Kept awake**: a cup (see `enso-ctl`).
- **The keyboard layout** (`US`, `RU`), when there's more than one.

With **Do not disturb** on, the bell is drawn quiet and struck through.

The clock in the middle knows **what's playing**. While anything plays its
pill turns to sea at dusk — the theme's iris in the deep water, rising
through three waves rolling across it at their own speeds to a rose one in
front — like a tide as playing starts, ebbing when it stops, with the time
in front of it all along. Every twenty seconds or so a set rolls in: a
swell sweeps across from the left, lifting the far waves first and the
nearest highest, a line of foam along its crest. The sea moves thirty times a second, not every frame: about one percent of a core
while it plays, nothing at rest. Rest the pointer on the clock and `enso-play` hangs a card
beneath it; click it to play or pause. The bar learns what plays from
`enso-play follow`, so it still speaks no D-Bus itself. `now_playing =
false` under `[bar]` turns all of it off.

With nothing playing, resting on the clock hangs **the month** there
instead: today in the accent, the arrows (or a scroll) for the months
around it, a click on its name to come back. It's this bar run again
(`enso-bar --dropdown`); `calendar = false` under `[bar]` turns it off.

Resting on a window's dot, the clock says **which window it is** — its
title, or its application — gliding to the width of the words and back.

**`enso-play`** — one small card for any media player on the session bus
(MPRIS): a YouTube tab in Firefox, Spotify, mpv. The cover on the left (the
card is warmed a touch by its colour), the title, and who it's by and in
what. Under them the timeline: what has played is a wave that rolls slowly
onwards while it plays and lies down flat when paused, what's left a quiet
straight line, a small upright thumb between them to drag. Then the times,
and previous, play or pause, and next — the ones the player can't do are
faint. With two players or more, dots at the foot choose between them;
otherwise the card follows whichever is playing.

Opened from the bar it never takes the keyboard and goes when the pointer
does. `enso-play` alone opens it on purpose, and again closes it; then
`Space` plays or pauses, `h`/`l` (or arrows) move five seconds, `n`/`p`
skip, `Tab` turns to the next player, `Esc` closes. For the media keys,
`enso-play play-pause`, `next`, and `previous` act on whichever player is
playing, and `enso-play status` says what that is.

Covers Firefox keeps on disk are read where they are; covers on the web
(Spotify's) are fetched once into `~/.cache/enso/covers`.

**`enso-ctl`** — one card. The first page is the glance and the quick
controls. Tiles first: the battery, with how long it lasts (or how long
until it's full — or until the limit, where the system stops charging
short of full to spare the battery; held there, it says *Kept at 80 %*); the power profile, with the CPU's temperature and fan
speed under it; then **Do not disturb** and **Keep awake**, two switches
lit in the accent while on, saying until when if they end by themselves.
Then a slider each for screen brightness and volume, and a row each for
Wi-Fi and Bluetooth, with their on/off switch. Tiles show only what the
machine has, so a desktop without power profiles shows the temperature
alone.

Keep awake holds off the idle lock (`enso-lock watch`) — for a talk, a
long read, a build to watch — and a cup on the bar says it's on. The
brightness slider ends in a moon, with enso-dusk installed: night light,
the other way from now until the next sunset or sunrise.

The volume row's speaker is its mute button and its track is the slider;
anywhere else on the row leads to the Sound page.

A row opens a page of its own:

- **Wi-Fi** lists the networks in range with their signal. The selected row
  says what choosing it does (`Connect`, `Disconnect`); a new secured network
  asks for its passphrase in a field right under it.
- **Bluetooth** lists paired devices and, while the page is open, looks for
  nearby ones. Choosing a new device pairs, trusts, and connects it; one that
  needs a PIN typed or numbers compared asks through `enso-auth`.
- **Sound** starts with where sound goes: the output's level, then every
  output there is — laptop speakers, a Bluetooth headset or speaker — with the
  one in use marked; choosing another moves what is playing over to it. Then
  one slider, with its own mute, for every application that is playing (all
  of a browser's tabs count as one). Then the microphone: an on/off switch,
  its level, and the microphones to choose between.
- **Theme**, from Settings, is the picker. Choosing one themes everything
  `theme set` does: see [Theming](#theming).
- **Displays**, from Settings: each screen niri drives — its resolution and
  refresh and its scale, stepped with the arrows (or `h`/`l`), variable
  refresh where it can, and on or off when there's more than one. niri
  applies each at once, and enso-ctl keeps each screen changed here, whole,
  in `~/.local/state/enso/outputs.kdl`, so niri starts with it too. niri
  reads the first section it finds for a screen, so its config includes that
  file ahead of its own outputs (sumi does); remove a screen's section to go
  back.

Beside the first page, a sideways swipe on the touchpad away — or `Tab`,
the page keys, or the dots at the card's foot — is **Settings**: the theme,
**Do not disturb** — stepped with its arrows from off to an hour, three
hours, until the morning, and on, saying until when — and the screen and
sleep: how long the machine sits
idle before the screen locks, how long the lock screen waits before turning
the screens off and before putting the machine to sleep, and whether it
locks just before sleeping. Each time steps with its arrows (or `h`/`l`).
What's chosen there is kept in `~/.local/state/enso/settings.toml`, laid
over `config.toml`'s `[notify]` and `[lock]` without touching it, and the
notification daemon and the lock watcher follow it at once.

Keys: `j`/`k` or arrows move; `Enter` or `l` opens or chooses; `h`, `Esc`, or
`Backspace` goes back; `h`/`l` adjust a slider or a time; `Tab` turns to the
page beside; `m` mutes; `Space` flips the switch of the row or page; `p`
cycles the power profile; `d` Do not disturb, `a` keep awake, `n` night
light; `q` closes.

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
enso-ctl dnd 1h                     # Do not disturb for an hour; on, off, toggle
enso-ctl awake 2h                   # keep awake for two hours; on, off, toggle
enso-ctl copy some text             # on the clipboard, kept there from the background
enso-ctl colour                     # pick a colour from the screen: its hex is copied
enso-ctl --page wifi                # open on a page: settings, wifi, bluetooth,
                                    # sound, displays, theme (an open one turns to it)
```

Raising the volume (or the microphone's level) unmutes it, as every
desktop's volume keys do.

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
one key binding toggles it. The control center's pages are there by name and
by what they're for — *Settings*, *Wi-Fi*, *Sound*, *Displays* (or
"resolution"), *Appearance* — and a terminal always opens a new window,
where a running application is gone to. `enso-launch --query =` opens with
a sum begun (sumi binds the calculator key to it). What you chose before ranks a little higher next
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

**Inside files**: a query starting with a quote (`"green tea`, or
`"/te+a/` for a pattern) searches the contents of the indexed files, shows
each match's line, and gives way the moment the query changes.
`enso-launch --find '"words'` does it from a terminal.

Beyond applications and files:

- **A running application is gone to.** Typing one that's open marks its
  chip with a dot, and `Enter` brings its most recent window forward
  (niri's windows, by their app id); `Shift+Enter` opens another.
  **Windows are found by their title** too — a page, a folder, a document.
- **Sums are worked out**: `12*7.5`, `2^10`, `sqrt(2)`, `80*15%`, listed
  first; `Enter` copies the answer, which stays on the clipboard after the
  launcher has gone.
- **An application's actions** — New Private Window, New Document — by
  their names.
- **The desktop's own commands**: lock, sleep, a theme by name, Wi-Fi or
  Bluetooth off, Do not disturb (or for an hour), keep awake, night light,
  pick a colour. Logging out, restarting, and shutting down are listed only
  once most of their name is typed, so a stray `Enter` never ends the
  session.

The field is the suite's own: arrows and `Home`/`End` move the caret,
`Ctrl+V` pastes, and `Ctrl+C` copies a file's path or a sum's answer.
Letters that matched are picked out in the accent; the selected row says
what `Enter` will do. If something can't start, the card stays and says
why.

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
  (`image-data`) or named as a PNG, JPEG, WebP, or GIF file are shown
  beside the text. Icon-theme
  names are not looked up: the suite carries no icon theme.
- **Level bars.** A notification with a `value` hint draws it as a bar, and
  one tagged `x-canonical-private-synchronous` replaces the last with the
  same tag — together, a volume or brightness pop-up that updates in place:
  `notify-send -h int:value:60 -h string:x-canonical-private-synchronous:volume Volume`.
- **Markup** in the body is understood and shown as plain text.
- **Reading time.** Nothing expires while the pointer is on the toasts.
- **Do not disturb** (`do_not_disturb`, the control center, or
  `enso-ctl dnd 1h` for a while): notifications go quietly to the
  history, and the bell counts them; critical ones — a battery about to
  run out — still show.
- **Low battery.** The daemon looks at the battery itself every half
  minute and warns at 10 % and, critically, at 5 %, with how long is left
  and a *Sleep now* button. Plugging in takes the warnings away.
- **One-time codes.** A message that carries one ("Your code is 482 913")
  gets a *Copy 482913* button.
- **Pictures that are files.** A toast whose picture is a file of yours,
  from a sender offering nothing to do with it — niri's "Screenshot
  captured" — opens the picture when clicked, and has *Open* and *Show in
  Files*.
- **For now only.** A notification marked `transient` shows as a toast
  and is never kept in the history or counted by the bell.
- **History.** `enso-notify history` — or a click on the bar's bell — opens
  a card where the toasts appear, listing the last 50 notifications, newest
  first. Five show at once and the rest are a scroll away: the list glides
  rather than steps, and instead of a scroll bar it fades toward whichever
  edge has more beyond it. Rows follow the card in one after another; click
  one to remove it and the rest close ranks, or `Clear` them all. `Esc`, a
  second `history`, or moving the pointer away closes it. While it is open
  new notifications arrive in the list rather than as toasts.

  A notification that offers something to open — a message, a finished
  download — is not over when its toast leaves: its row says `Open` under
  the pointer, and a left click does what clicking the toast would have
  (any other button only removes it). Its sender is told it is closed when
  it leaves the list, not before.

  The history is kept across restarts and logins (`history_keep`): the
  words and the time, in `~/.local/state/enso/notify/history.toml`, readable
  by you alone. Pictures and actions belong to the session that sent them
  and are not kept.

The daemon also keeps a two-line file, `~/.local/state/enso/notifications`
(the unattended count, and whether the history is open). That is all the bar
knows about it — the two share no process and neither needs the other.

**`enso-wall`** — a picture behind everything, on every screen, chosen by
theme. Each theme can have its own; the rest share a default; and with
neither, the theme's base colour is the wallpaper, so it works from the
first run.

```sh
enso-wall set ~/Pictures/quiet.jpg                        # the default
enso-wall set ~/Pictures/morning.png --theme rose-pine-dawn
enso-wall set ~/Pictures/night.jpg --current              # the active theme
enso-wall unset --theme rose-pine-dawn                    # back to the default
enso-wall list                                            # who gets what, and why
```

Pictures are PNG, JPEG, WebP, or GIF. `set` records the choice in
`~/.local/state/enso/wallpapers.toml`; the same slots can be declared under
`[wall]` in `config.toml`, and a recorded choice wins over a declared one. A
running `enso-wall` follows both `set` and `enso-ctl theme set` at once,
fading from the old picture to the new (420 ms). A picture that has gone
missing falls through to the default rather than leaving a hole.

It costs what a picture costs and no more. A frame is fitted to the screen's
real pixels (averaging on the way down, so a large photograph does not
shimmer), handed to the compositor, and let go: at rest the process holds no
pixels at all — about 5 MB — and wakes for nothing but a theme, a wallpaper,
or a monitor changing.

**`enso-power`** — a row of five: Lock, Sleep, Log out, Restart, Shut down.
Each has a key, a letter of its own name marked there in the choice's colour
— the **L** of Lock, the **p** of Slee*p* (the S being Shut down's), the **o**
of Log *o*ut, the **R** of Restart, the **S** of Shut down — so leaving is the
binding that opens the menu and one letter. Landing
on a choice sends a glint of light across its letter, with a small sparkle
at its shoulder, once. The three that end the session ask for the letter (or `Enter`)
a second time — the tile warms and a line says what a second press does —
and `Esc` takes the question back before it closes the menu. Arrows, `Tab`,
`j`/`k`, and `Ctrl+n`/`Ctrl+p` move; a click chooses; a click outside or a
second `enso-power` closes. `confirm = false` under `[power]` makes every
choice immediate, and `actions` picks and orders the tiles. Each choice takes the
first free letter of its name, in a fixed order of who chooses first, so the
letters never clash however the tiles are ordered.

`enso-power lock` (or `suspend`, `logout`, `restart`, `shutdown`) skips the
menu and acts, for key bindings. Shutting down, restarting, and sleeping go
through logind, which lets the person at the keyboard do them without root;
logging out asks Niri to quit over its socket; locking runs `enso-lock`
(`lock_command`), found next to `enso-power` or on `PATH`.

**`enso-lock`** — the ensō, a clock, the date, and a field that fills with
dots, over the theme's wallpaper blurred into colour and light (or the base
colour alone). The ensō paints itself in as the lock appears, then breathes
(see `enso-greet`). `Enter` checks the password; `Esc` or `Ctrl+u` clears the field; a
wrong one shakes it. After three misses each further one costs a wait,
doubling to half a minute.

The lock is Niri's own (`ext-session-lock`): once granted, nothing else is
shown or given input, and if `enso-lock` dies the session *stays* locked —
from a console (`Ctrl+Alt+F3`), `WAYLAND_DISPLAY=wayland-1 enso-lock` starts
another, which takes the lock over. It refuses to lock at all if it could not check a password afterwards.

A locked machine is one nobody is looking at, so it is made a quiet one:

- **Nothing is drawn** unless a key is pressed or the minute turns.
- **After 30 seconds alone the screens power off** (`screen_off_after`), and
  from then on not even the minute is followed: the process has nothing
  scheduled and sleeps until a key or the pointer, which brings the screens
  back.
- **The low-power profile is held** while locked and the previous one put
  back on unlock (`power_saver`). This is the same dial as `enso-ctl
  profile`, so it needs the same udev rule; without it, nothing happens.
- **Optionally, suspend** after a longer while (`suspend_after`, off by
  default).

Nothing in the session is stopped — music keeps playing, a build keeps
building, only slower — so unlocking returns to exactly what was left, with
no windows to wake. A laptop running low on battery says so at the foot of
the lock screen (20 % and under), so one left locked doesn't die unnoticed.

**Locking without being asked** is `enso-lock watch`: started with the
session, it draws nothing and runs the lock screen at the two moments nobody
presses the key.

- **When the machine is left alone** for `idle_after` seconds (300; `0`
  never). The compositor does the counting (`ext-idle-notify`), and does not
  count time a film or a call keeps the screen awake.
- **Just before it sleeps** (`before_sleep`) — a closed lid, the power menu,
  `systemctl suspend` — so it never wakes unlocked. logind is asked to wait
  for the lock screen, and does.

```kdl
spawn-at-startup "enso-lock" "watch"
```

All four — `idle_after`, `screen_off_after`, `suspend_after`, and
`before_sleep` — can be changed from the control center's Settings page as
well as in `config.toml`; the watcher picks up a change at once.

**Kept awake** (the control center's tile, or `enso-ctl awake 1h`), being
left alone doesn't lock the machine. When that ends — its time runs out, or
it's turned off — while nobody is there, the lock comes then, as it would
have.

Only one lock screen runs at a time: a second, from the key while the first
is up, steps aside.

**`enso-greet`** — the login screen. It is a greeter for
[greetd](https://sr.ht/~kennylevinsen/greetd/), which does everything a
login has to get right: the console, PAM, starting the session as the right
user. `enso-greet` only draws, and passes PAM's questions and the answers
between the screen and greetd's socket. It runs in a niri of its own,
started by greetd as its `greeter` user; once you are in, it asks greetd for
your session and quits that niri, and greetd starts the session in its place.

On screen: the terraces from the boot menu, quietened, the ensō, your
name and the password field, looking just like the lock. `enable` also
writes a Plymouth theme: while the machine starts, the ensō paints itself
on the same terraces and a light runs along it until the login screen is
ready, which opens with the ensō already whole. Tab or the arrow keys go through the others who can log in
(ordinary accounts with a login shell). A wrong password shakes the field;
whatever PAM says — an account locked after too many tries, say — is shown
as it says it. The session is your login shell running `niri-session`, so
your profile (`~/.bash_profile` and the like) sets its environment just as a
console login does.

Setting it up needs root once, and enso never runs anything as root:

```sh
enso-greet enable    # writes the files, prints the sudo lines to run
enso-greet preview   # see it now: password "enso", Esc leaves
enso-greet disable   # prints how to go back to the console login
```

It runs before anyone logs in, so its settings are system-wide:
`/etc/enso/greet.toml` (`default_user`, `command`), and the look from
`/etc/enso/config.toml` — `enable` writes the interface font there if it is
one only you have, and says how to copy it where the greeter can read it.

**The ensō** (`enso_ui::mark`) is the suite's mark: the circle from the
Neovim start screen as one brush stroke, twenty-six bristles running dry
towards its end. It is computed, not drawn from a file, so it is the same
pixels everywhere. Where it is shown it breathes — a slow swell and a faint
glow every nine seconds, with a light travelling the stroke every fourteen —
redrawn a few times a second, never every frame. With motion turned off it
is simply there.

**`enso-boot`** — the boot menu, a program for the firmware. Over a
picture of rice terraces stepping down to a river at dusk, the river
winding off to the mountains under the moon, all in voxels and the theme's
colours, it offers Arch and Windows. Nothing starts until Enter: it never
picks for you. Choosing Arch puts up the ensō on the base colour, the very
frame the login screen starts from, and with a quiet boot that is what
stays on screen until the password is asked for (a moment of black as the
graphics driver takes over is likely).

The picture is computed (`enso-scene`: a height field of voxels, one ray a
pixel and more at edges), drawn with the menu's words when enso-boot is
built for the firmware, and packed into it; at boot it is only unpacked and
shown. Arch starts as systemd-boot's entry for it says (`/loader/entries`),
so kernel options are changed in one place for both menus; Windows' own
boot manager is started from the same partition.

```sh
rustup target add x86_64-unknown-uefi
cargo build --release -p enso-boot --target x86_64-unknown-uefi
cargo run --release -p enso-boot -- install   # prints the steps, as root
```

systemd-boot stays installed underneath: Esc in the menu, or anything
going wrong, hands over to it.

**`enso-auth`** — when a program asks for something only an administrator
may do (mounting an internal disk, changing a system setting, `pkexec`),
polkit asks the session's *authentication agent* to have someone show who
they are; without one it just says no. When a Bluetooth device wants a PIN,
a passkey, or numbers compared, BlueZ asks its *pairing agent*; without one
such a device can't be paired. `enso-auth` is both, started with the session:

```kdl
spawn-at-startup "enso-auth"
```

Each question is one card over the dimmed screen, the same for all of them.
For polkit: what is being asked and by which program (by its application's
name where one owns it: *Files (enso-files)*), who answers (a choice,
`←`/`→` or `Tab`, when polkit would take any administrator), and a field that
fills with dots; a wrong password shakes it, Caps Lock is pointed out, and
anything PAM says — a second factor's question, "the account is locked" —
is shown as it comes. For Bluetooth: the number to compare or to type on the
device, large and spaced in threes, or a field for the PIN or passkey; a
device not yet trusted asking for a service (audio, input, the network) is
asked about by name. `Enter` answers yes, `Esc` no, and the mouse works too;
a stray click outside doesn't dismiss it. `enso-auth try password` (or
`confirm`, `pin`, `passkey`, `show`, `authorize`, `service`) shows each card
with nothing checked.

The agent itself never holds a password. Each card is a short-lived process
of its own, kept out of core dumps, which hands what's typed straight to
polkit's helper (`/run/polkit/agent-helper.socket`, or the setuid
`polkit-agent-helper-1`): the helper runs PAM as root and tells polkit the
outcome itself, so nothing in enso can answer "yes" for you. The helper is
only reached once `Enter` is pressed — a PAM conversation left unanswered
counts as a failed attempt, and `pam_faillock` locks the account after three.
Only polkit may ask for a password and only BlueZ for a pairing code: calls
from anyone else are refused. If either service restarts, the agent
registers again.

**`enso-files`** — a file manager, and every application's file chooser
once `enso-files portal enable` has run — which also lists it as *Files*
and makes it what folders open in, unless something else was chosen. The window is a list (or a grid
of thumbnails, or columns: `Ctrl+1/2/3`) with the places, bookmarks,
drives, and the trash down the side; each folder keeps its own view, sort,
and dotfiles once you change them, and Settings (`Ctrl+,`) sets the rest.
`?` shows the keys; `Ctrl+P` (or `:`) finds every command by name, with
its key, and the folders you go to most.

- **Moving**: arrows or `hjkl`, `Enter` to open, `Backspace` up, `Alt+←/→`
  back and forward, `Ctrl+L` to type a path, `g h`/`g d`/… to jump.
- **Choosing**: `Space`, `Shift`+arrows, a box dragged over empty space,
  `Ctrl+A`; what's chosen stays chosen across folders, and the status line
  says where.
- **Tabs and the split**: `Ctrl+T`, `Ctrl+W`, `Ctrl+Tab`, `Alt+1…9`;
  `F3` puts two side by side, `Tab` moves between them, `F5`/`F6` copy and
  move across.
- **Finding**: `/` filters the folder; `Ctrl+F` searches everything below
  it by name as you type, `Ctrl+Shift+F` inside files (`/like this/` is a
  regular expression). Both take query words — `type:image`, `ext:rs`,
  `size:>10M`, `modified:<7d`, `name:…`. Recent, in the sidebar, is what
  this and every other program opened lately.
- **Looking**: `i` opens a preview beside the list, `Shift+Space` a large
  one over it; `Ctrl+I` the properties, with permissions to tick and a
  folder's size counted all the way down.
- **Changing**: `F2` renames — several at once with find and replace, a
  numbering template, or `$EDITOR` (a setting) — `Ctrl+Shift+N` makes a
  folder, `Ctrl+C`/`X`/`V` copy, cut, and paste (with other file managers
  too), `Delete` trashes, `Ctrl+Z` undoes, `Ctrl+D` bookmarks, `F4` opens
  a terminal here.
- **Dragging**, both ways, with any program: what's dragged out goes as
  `text/uri-list` (and quoted paths, for terminals), with a pill beside the
  pointer saying what's held; what's dropped in lands in the folder under
  the pointer — copied, or moved when the drag is from this window.
- **Archives**: zip and tar (gz, xz, zst, bz2) open as folders; take things
  out, or make a new one.
- **Version control**, opt-in (Settings): the branch and its state in the
  status line, a branch menu with remote branches, fetch, pull, push,
  clone, the identity to commit as. Signed in to **GitHub** or **GitLab**
  (gitlab.com or your own) with your own OAuth app's device flow, the
  token goes in the keyring; clone lists your repositories, publish makes
  one, and git can ask enso-files for the way in.
- **Drives** in the sidebar (udisks2), mounted with a click, and **Open
  with…** for every application that takes the type.

As the file picker, behind the desktop portal, it answers every "Open…"
and "Save as…" — Firefox, Chromium, Electron, Flatpaks — in the window of
the program that asked, and remembers each program's folder.

**Sharing the screen** goes through the same portal, to
[xdg-desktop-portal-gnome](https://gitlab.gnome.org/GNOME/xdg-desktop-portal-gnome):
niri speaks GNOME's screen-casting interface, and that portal is what
turns it into the one browsers, Discord, and OBS ask for. With it
installed, calls can share a screen or a window. The portal links the
system's portal files again each time it starts (`enso-files portal
refresh`, run from its drop-in), so installing it later needs nothing
more than the portal starting again; `enso-files portal status` says
whether it's there. While anything shares, the bar shows it, and a click
there stops it.

**`enso-keyring`** — where programs keep passwords and tokens: the Secret
Service (`org.freedesktop.secrets`), which gh, git (with
`credential.helper = /usr/lib/git-core/git-credential-libsecret`), VS Code,
Chromium, and anything built on libsecret use. `enso-keyring enable` makes
it the session's keyring, started by the session bus the first time a
program wants a secret.

It opens with your login password, checked against the system's own record
(`unix_chkpwd`, as `enso-lock` checks it) every time — the password is the
gate, and nothing else: it's never kept, and never used to seal anything.
What seals the keyring is this machine: a random master key, sealed by
systemd's credential service with the host key and the TPM and scoped to
your account, so it opens only here, only for you, and not from a copy of
the disk. Everything else — labels, attributes, secrets — is sealed by the
master key (XChaCha20-Poly1305) in `~/.local/share/enso/keyring.toml`,
readable by you alone. Attributes are also kept hashed, as gnome-keyring
keeps them, so a program can find its secret while the keyring is locked
and ask for it to be opened. The card says which program is asking.

So changing your password — even resetting it as root, having forgotten it
— strands nothing: the next unlock takes the new one. The other side of
that: something already running as you could have systemd unseal the key
without the password, as it could read an unlocked keyring anyway. Bound
to no TPM registers, it survives firmware and Secure Boot updates; only
clearing the TPM loses it, and then the card says so and starts a new one,
keeping the old file beside it. `enso-keyring lock` locks it;
`enso-keyring status` says how it stands.

**`enso-clip`** — clipboard history. Started with the session, it keeps
what's copied — text, and pictures — for as long as the session lasts:
the last 50, in `$XDG_RUNTIME_DIR/enso-clip` (memory, readable by you
alone, never written to the disk). It watches the clipboard through
`ext-data-control`, the protocol clipboard managers use, and:

- **Keeps the newest on the clipboard** after the program that copied it
  quits, so what was copied from a window since closed still pastes.
- **Never reads a secret**: what a password manager marks so
  (`x-kde-passwordManagerHint`) isn't kept, or even looked at.

`enso-clip pick` (Mod+Ctrl+V in sumi) opens the history as a card like
the launcher's: newest first, pictures as thumbnails, a field to narrow it
down. `Enter` puts the chosen one on the clipboard, ready to paste;
`Delete` forgets it. `enso-clip list` prints it; `enso-clip clear`
forgets everything.

```kdl
spawn-at-startup "enso-clip"
binds { Mod+Ctrl+V { spawn "enso-clip" "pick"; } }
```

**`enso-dusk`** — night light. Started with the session, it holds every
screen's gamma (`wlr-gamma-control`) and warms it from sunset to sunrise,
changing over half an hour centred on each (`fade`), to `warmth` kelvin
(3500). When the sun sets is worked out here, from nothing but the
timezone: the timezone database knows where each zone's city is
(`zone1970.tab`), which is near enough — `latitude` and `longitude` under
`[dusk]` win over it. In between it sleeps until the light next turns.

Turned on or off by hand — the control center's moon, `n` there, the
launcher's *Night light*, `enso-dusk toggle` — it stays that way until the
next sunset or sunrise, and then the sun has its say again. `enso-dusk on`,
`off`, and `sun` are for good; `enso-dusk status` says how it stands, and
today's sunrise and sunset.

Given `day_theme` and `night_theme` (`rose-pine-dawn` and
`rose-pine-moon`, say), it switches the whole desktop's theme at sunrise
and sunset too — unless a third theme was chosen by hand, which it leaves
alone.

```kdl
spawn-at-startup "enso-dusk"
```

**`enso-view`** — pictures, one at a time, in a window of their own: as
large as fits (never enlarged to fit), on the theme's base colour, with a
quiet line under it — the name, which of how many, its size, the zoom — that
is there while you move and gone when you look. Opened on a picture it steps
through the others in its folder, in the order a file manager lists them,
reading the next ahead. A photo is turned the way the camera was held
(EXIF), here and everywhere enso shows pictures.

`Right`/`Space` and `Left`/`Backspace` step, `Home`/`End` go to the ends;
`+`/`-` or the wheel zoom about the pointer, `0` fits, `1` or a
double-click shows every pixel, and a drag moves about; `r`/`R` turn it;
`Ctrl+C` copies the picture itself; `w` makes it the wallpaper
(`enso-wall set`); `Delete` moves it to the trash and `Ctrl+Z` puts it
back; `e` shows it in Files.

`enso-view enable` makes it what PNG, JPEG, WebP, and GIF open in (those
nothing else was chosen for), with a desktop entry that stays out of the
launcher; `enso-view disable` gives them back.

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

### The rest of the desktop

The same switch — from the command line or the panel's Theme page — takes
the palette to every other program that can be told:

```
$ enso-ctl theme set rose-pine-dawn
Rosé Pine Dawn (rose-pine-dawn)
  enso       running tools recoloured
  niri       colours updated
  alacritty  colours updated
  neovim     colours updated
  gtk        light colour scheme set
```

The rule is that enso never edits a config file you write by hand. For each
program it writes a file of its own, in `~/.local/state/enso/`, and one line
of yours brings that file in — after which the program follows every switch
by itself. Until the line is there, `theme set` says which line and where.
Programs that are not installed are passed over without a word;
`enso-ctl theme apply` applies the active theme again and lists them all,
which is the thing to run after installing one, or after adding a line.

| Program | What enso writes | What you add, once | Follows a switch |
| --- | --- | --- | --- |
| Niri | `niri.kdl`: ring, border, shadow, backdrop colours | `include optional=true "~/.local/state/enso/niri.kdl"` in `config.kdl`, after any colours of your own | at once |
| Alacritty | `alacritty.toml`: Rosé Pine's own mapping to the terminal colours, and the accent for the cursor, as enso's carets have it | `import = ["~/.local/state/enso/alacritty.toml"]` under `[general]` in `alacritty.toml` | at once |
| Neovim | `nvim.lua`: the palette, handed to [`rose-pine/neovim`](https://github.com/rose-pine/neovim) | `dofile(vim.fn.expand("~/.local/state/enso/nvim.lua"))` in `init.lua`, after the plugin is loaded | at once, in every running Neovim |
| GTK, and the system | the dark-or-light preference (dconf's `color-scheme`); `gtk-application-prefer-dark-theme` for GTK 3 | nothing | at once — GTK 3 programs when next started |
| Fonts | `fonts.conf`: the configured `font` and `mono_font` first for `sans-serif`, `system-ui`, and `monospace`; `font-name` and `monospace-font-name` in dconf; GTK 3's `gtk-font-name` | `<include ignore_missing="yes">~/.local/state/enso/fonts.conf</include>` in `~/.config/fontconfig/fonts.conf` | at once — fontconfig programs when next started |
| The cursor | `cursor-theme` and `cursor-size` in dconf; GTK 3's two cursor keys; `~/.local/share/icons/default` inheriting the chosen theme; and a `cursor` section in `niri.kdl` | nothing — choose one (see [The cursor](#the-cursor)) | at once |
| GTK's palette | `gtk-4.0.css`: the colours libadwaita is built from; `gtk-3.0.css`: the same, and GTK 3's stock widgets recoloured | `@import url("file:///home/you/.local/state/enso/gtk-4.0.css");` in `~/.config/gtk-4.0/gtk.css`, and likewise for `gtk-3.0` | when a program next starts |
| Firefox | `firefox.css`: tabs, toolbars, address bar, menus | the same kind of `@import` in a profile's `chrome/userChrome.css`, with `toolkit.legacyUserProfileCustomizations.stylesheets` on in `about:config` | when Firefox next starts |
| Qt | a colour scheme, `colors/enso.conf`, for qt5ct and qt6ct | choose the custom colour scheme `enso` in the tool's Appearance tab | within a few seconds |
| Flatpak | four read-only entries in the user's global override | nothing | when a program next starts |

Some particulars:

- **Alacritty** keeps what its own config says over what it imports, so
  colours set there stay yours. It watches only imports that existed when it
  started: terminals opened before the first `theme set` need a restart.
- **Neovim** needs the plugin for colours — without it the file only sets
  `background`. Options you gave `require("rose-pine").setup` are kept; enso
  adds the palette and the variant. Any theme works, not only the three
  built-ins: the plugin derives every highlight from the same fifteen roles.
- **The colour scheme** is the one setting every toolkit asks the desktop
  portal for, so it is also how Qt 6, Firefox, and Electron programs learn
  that the desktop went dark or light. It is written through dconf's own
  service (`xdg-desktop-portal-gtk` passes it on); without dconf there is
  nothing to keep it, and the step is skipped.
- **GTK 3** does not ask the portal. enso sets the one key in
  `~/.config/gtk-3.0/settings.ini` — the exception to the rule above, since
  that file has no way to include another — and leaves the rest of it alone.
  GTK 3's stock theme has its colours built in, so `gtk-3.0.css` restyles
  the stock widgets themselves: a file chooser or a settings dialog then
  belongs to the desktop. It also defines every role as `@enso_base`,
  `@enso_accent`, … for a `gtk.css` of your own.
- **Firefox** already follows dark and light. The stylesheet colours its own
  interface; pages are left to the web.
- **Qt**'s palette needs qt5ct or qt6ct as the platform theme
  (`QT_QPA_PLATFORMTHEME=qt6ct`). Without one, Qt 6 still follows dark and
  light.
- **Flatpak** programs cannot see `~/.config/gtk-4.0` or enso's stylesheets
  from inside their sandbox. enso lets them, read-only, by adding
  `xdg-config/gtk-3.0`, `xdg-config/gtk-4.0`, and the two stylesheets to
  `filesystems` in `~/.local/share/flatpak/overrides/global` — what
  `flatpak override --user --filesystem=…` would write. Entries you already
  have, or have denied with a `!`, are left as they are.

To have enso leave a program alone, drop it from the list:

```toml
[ctl]
theme_targets = ["niri", "alacritty", "neovim", "gtk", "fonts", "cursor", "firefox", "qt", "flatpak"]
```

A new target is one file in `crates/enso-ctl/src/theme/` — a function from
the theme to an outcome — and one more `Place`.

### Fonts

enso draws with the families named under `[appearance]`, and takes them to
the rest of the desktop with the theme:

```toml
[appearance]
font = "Google Sans Flex"        # the interface
mono_font = "Google Sans Code"   # the terminal, and code
```

GTK is told through dconf (`font-name`, `monospace-font-name`, sized from
`font_size`), and everything that asks fontconfig for `sans-serif` or
`monospace` — Alacritty, Firefox, web pages — through a generated
`fonts.conf` that puts these first, with Symbols Nerd Font behind them.
Preferences of your own after the include line come after enso's: the place
for fallbacks. A family left generic (the default) is the system's to choose,
and one that is not installed falls back to its kind, with a word on stderr.

### The cursor

One XCursor theme, at one size, for every program. Choose it in
`config.toml`, or try one without editing anything:

```toml
[appearance]
cursor = "Bibata-Modern-Ice"   # a directory in ~/.local/share/icons, /usr/share/icons, …
cursor_size = 24
```

```
$ enso-ctl cursor
  Adwaita            /usr/share/icons
● Bibata-Modern-Ice  ~/.local/share/icons  24 px
$ enso-ctl cursor set bibata-modern-ice 32
Bibata-Modern-Ice, 32 px
  niri       colours updated
  cursor     Bibata-Modern-Ice, 32 px
$ enso-ctl cursor size 24
```

`cursor set` records the choice in `~/.local/state/enso/cursor`, which wins
over `config.toml` as `theme set` does over `theme`. enso installs no
cursors: put the theme's directory in `~/.local/share/icons` first. One
that is chosen but missing is reported, and Niri is not pointed at it.

Programs learn the cursor three ways, and all three are told:

- **Niri** draws the pointer for its own surfaces and for every program that
  asks for a cursor by shape — enso's tools, Alacritty, Firefox, GTK 4 — and
  hands `XCURSOR_THEME` and `XCURSOR_SIZE` to whatever it starts. It reads
  the theme from `niri.kdl`, the file it already includes. Niri merges the
  `cursor` section across files, so a `hide-when-typing` of yours stays; an
  `xcursor-theme` of yours included after `niri.kdl` wins over enso's.
- **GTK** reads dconf (GTK 4 by way of the portal) and changes cursor while
  running; GTK 3 off Wayland reads `settings.ini`, where enso sets its two
  cursor keys and nothing else.
- **Everything else** that draws a cursor without being told falls back on
  the theme called `default`. enso writes
  `~/.local/share/icons/default/index.theme` to inherit the chosen theme —
  unless that file is yours, in which case it says what line to put there.

## Design language

All of it lives in `enso-core` as tokens, so tools never hard-code a look:

- `style` — radii (14 px cards, 12 px panels, pills), spacing scale, type
  scale, shadows, and the icon vocabulary.
- `motion` — easing curves, named transitions (`SELECT` 180 ms, `TOGGLE`
  200 ms, `REVEAL` 220 ms, `DISMISS` 250 ms, and `SCENE` 420 ms for a
  whole screen changing), and `Tween`, a value that glides
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
quickest way to see a visual change without restarting anything. A dozen
key screens are also held to reference pictures in `crates/*/snapshots/`:
a test fails when more than a sliver changes, and writes the changes in
magenta beside the snapshot. `ENSO_BLESS=1 cargo test` makes the new
pictures the references, for when a change is meant.

Tests that could raise a real prompt (enso-auth, enso-keyring) run without
a compositor: `env -u WAYLAND_DISPLAY cargo test --workspace`.

**Releasing** is pushing a tag: `git tag v0.2.0 && git push origin v0.2.0`.
The release workflow builds every tool, packs them with their SHA-256, and
publishes the release, its notes written from the commits since the last
one.

## Roadmap

- **enso-ctl** — forgetting networks and devices from the panel (the
  command line can); more theme targets (other terminals, Kvantum).
- **enso-launch** — Git's global excludes file and `.git/info/exclude`.
- **enso-notify** — keeping pictures in the saved history; styled (bold,
  italic) body text.
- **enso-wall** — a picture per output.
- **enso-view** — animated GIFs; a slideshow.
- **enso-lock** — authentication beyond the password (fingerprint), which
  means PAM.
- **enso-boot** — a picture for other screen sizes than 16:9 at 1080p
  (others are scaled, not redrawn); entries beyond Arch and Windows.
- **enso-greet** — a session chooser, once there is a second desktop to
  choose; remembering who logged in last.
- **enso-power** — a tile in the control center.
- **enso-files** — the plan's last phase: saved searches, restoring the
  last session's tabs, checksums, tags, and finding duplicates.
- **Releases** — builds for aarch64.

## License

MIT — see [LICENSE](LICENSE).
