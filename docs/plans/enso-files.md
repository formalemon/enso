# enso-files — plan

A file picker for every application on the desktop, and a file manager that
comes free with it. Pure Rust, drawn by `enso-ui`, coloured by the active
theme, quick enough that opening a folder never feels like waiting, and as
comfortable with the mouse as with the keyboard.

Status: **plan**. Branch `enso-files`. Nothing below is built yet; the numbers
in [Measured baseline](#measured-baseline) were taken on this machine on
2026-11-18.

---

## 1. Why, and what "done" looks like

Today every "Upload…" or "Save as…" in Firefox opens GTK 3's file chooser,
drawn by `xdg-desktop-portal-gtk`. enso now themes it, but it is still
GTK 3: no multi-select across folders, no previews, no fuzzy filter, no
keyboard model beyond arrows, and it looks like a guest on the desktop.

`enso-files` replaces it **behind the same portal**, so Firefox, Chromium,
Electron apps, Flatpaks — anything that asks the portal for a file — get the
enso picker without knowing it. The same program, opened on its own, is a
file manager.

Done means:

1. Every portal file request (open one, open many, open a folder, save one,
   save many) is answered by enso-files, in the active theme, with the
   parent application dimmed behind it.
2. The picker can do everything GTK's could, plus: select files from several
   folders in one go, filter as you type, preview before choosing, and do
   all of it without touching the mouse — or without touching the keyboard.
3. As a file manager it covers what a person actually does in a week: browse,
   open, open-with, copy, move, rename (one or many), trash and restore,
   new folder, search, preview, compress and extract, see what's on a USB
   stick, and drag things to and from other windows.
4. It is never the slow part: a folder of 50,000 files lists in under
   50 ms, scrolling and selection run at the display's full 144 Hz, and the
   picker is on screen within 60 ms of the request.

## 2. Ground rules (from this repo and from you)

- **Pure Rust, direct interfaces.** No GTK, no Qt, no GIO, no shelling out
  to CLI wrappers (`gio`, `udisksctl`, `xdg-mime`, `file`). The freedesktop
  specs (MIME, trash, thumbnails, recent files, bookmarks, desktop entries)
  are implemented directly.
- **No new pacman/AUR packages.** New Rust crates are fine if pure Rust and
  worth their weight; each one is listed in [§11](#11-dependencies).
- **Standalone.** `enso-files` runs with no config file, no other enso tool
  running, and no portal set up — the portal is how it gets *used*, not
  something it needs to *work*.
- **One family.** Colours from the palette, radii/spacing/type from
  `enso_core::style`, every animation a named `Transition`, icons drawn as
  `Figure`s by `enso_ui::figure`, live theme switching like every other tool.
- **Keyboard-first, mouse-complete.** Every action has a key; every action
  is also reachable by pointer (click, right-click menu, drag, or the
  command palette). Neither input is the second-class one.

## 3. Measured baseline

Measured with a release-build probe against the real `enso-ui`, on this
machine (12 threads, NVMe, eDP-1 at 1920×1080 @ 144 Hz, scale 1).

| What | Time | What it means for the design |
| --- | --- | --- |
| `Context::load` + `TextEngine::new` | ~0.1 ms | Fonts load lazily; a fresh process is cheap. |
| First text measure (loads fonts) | ~1.4 ms | Paid once per process. |
| Shaping one short file name | ~7 µs | 2,000 names = 14 ms. **Shape only visible rows, cache them.** |
| List 50,000 names + types (`read_dir`) | 11 ms | Names alone can paint at once. |
| `stat` 50,000 entries, one thread, warm | 32 ms | Metadata arrives *after* the first paint, on a pool. |
| `stat` 2,418 entries in `/usr/lib`, cold | 25 ms | Cold disks are the slow case; never block on them. |
| Full 1888×1024 frame, 32 rows × 3 texts, scale 1 | 1.6 ms | A full redraw fits in 144 Hz's 6.9 ms with room to spare. |
| Same at scale 2 | 5.5 ms | HiDPI would want damage tracking; not needed here first. |

Conclusions:

- **One process per window is fine.** Start-up is dominated by Wayland
  round-trips, not by fonts. A tiny resident service (no Wayland, no fonts)
  answers D-Bus and spawns a window process per request.
- **Two-phase listing.** Paint names and kinds from `getdents` immediately;
  fill sizes and dates from a `statx` pool; re-sort only if the sort key
  needs them, keeping the cursor on the same file.
- **Shaped-text cache.** The README already lists "a shaped-text cache" on
  enso-ui's roadmap; enso-files is the tool that needs it.

## 4. Architecture

### 4.1 Processes

```
                 ┌──────────────────────────── session bus ─────────────────────────────┐
 Firefox ──► xdg-desktop-portal ──► org.freedesktop.impl.portal.FileChooser ──┐
 Firefox "Show in folder" ───────► org.freedesktop.FileManager1 ──────────────┤
                                                                               ▼
                                                    enso-files --service   (resident, ~2 MB,
                                                    D-Bus only, no Wayland, D-Bus-activated)
                                                         │ spawns, one per request/window
                                                         ▼
                        enso-files pick --request <json on fd 3>      enso-files [PATH…]
                        (xdg-toplevel, parented to the caller          (xdg-toplevel, the
                         through xdg-foreign; answer on fd 3)           file manager)
```

- **`enso-files [PATH…]`** — the file manager window. A second invocation
  opens a new window (tabs live inside one window; new windows are new
  processes).
- **`enso-files pick [--multiple] [--directory] [--save NAME] [--filter …]`**
  — the picker as a command: prints chosen paths to stdout, one per line
  (`-0` for NUL). Useful from scripts and from Neovim as it is from the portal.
- **`enso-files --service`** — implements the portal back-end and
  `FileManager1` on the session bus. It never draws: per request it spawns
  `enso-files pick` with the request on a pipe and relays the answer. A
  portal `Request.Close` kills the child. If the child crashes, the request
  answers "cancelled" (response 1) and the caller carries on.

Why not one long-lived GUI process: crash isolation (a bad thumbnail can't
take down every open dialog), zero idle memory for pixel buffers and
caches, and the measured start-up cost doesn't justify the complexity. If
start-up ever measures above 60 ms to first frame, the fallback is to keep
one warm, hidden picker process around — the pipe protocol already allows it.

### 4.2 Crates

| Crate | Change | Holds |
| --- | --- | --- |
| `enso-fs` | **new**, headless, no Wayland | Everything about files that isn't drawing (below). Unit-testable in temp dirs. |
| `enso-ui` | **extended** | An xdg-toplevel window role, clipboard and drag-and-drop, cursor shapes, more keys, click counting, a shaped-text cache, and a `widget` module. |
| `enso-sys` | small | `udisks2` client (feature `udisks`), used only if the service exists. |
| `enso-files` | **new** binary | The picker, the manager, the service. |
| `enso-launch` | refactor | Moves its MIME/open-with, desktop-entry, file-walk, ignore, and fuzzy code into `enso-fs`, unchanged in behaviour; tests move with it. One implementation, so "Open with" in the manager and Enter in the launcher always agree. |
| `enso-core` | small | `Figure`s for file kinds and file actions; a `[files]` config section. |

### 4.3 `enso-fs` modules

| Module | Responsibility | Source |
| --- | --- | --- |
| `list` | Two-phase directory listing (`getdents` names+`d_type`, then a `statx` pool), symlink resolution, permission errors per entry not per folder. | new |
| `sort` | Natural sort (`file2` < `file10`, case-folded, locale-light), by name/size/modified/created/kind/extension, folders first, stable. Precomputed sort keys. | new |
| `mime` | Shared MIME database: `globs2`, `magic` (sniffing for extension-less files), `subclasses`, `aliases`, `generic-icons`, human descriptions from `packages/*.xml` ("PNG image"). | moved from `enso-launch/open.rs`, extended |
| `apps` | Desktop entries, `mimeapps.list` read **and write** (set default, add association), `Exec` field expansion (`%f %F %u %U`). | moved from `enso-launch/apps.rs` |
| `open` | Plan how to open a path (default app, terminal fallbacks). | moved |
| `walk` / `ignores` / `fuzzy` | Parallel recursive walk, gitignore-aware; fuzzy scorer. | moved from `enso-launch` |
| `ops` | Copy, move, link, rename, mkdir, delete, trash, restore, with progress, pause/cancel, conflict policy, and an undo journal. `copy_file_range`/reflink first, rename across a single filesystem, preserve mode and mtime, sparse-aware. | new |
| `trash` | freedesktop Trash spec: `$XDG_DATA_HOME/Trash` and per-mount `$topdir/.Trash-$uid`, `.trashinfo` read/write, restore, empty, `directorysizes`. | new |
| `thumbs` | freedesktop thumbnail spec: `~/.cache/thumbnails/{normal,large,x-large,xx-large}`, MD5 of URI names, `Thumb::URI`/`Thumb::MTime` PNG text chunks, `fail/enso-files/` markers. Readers for every format in [§6.8](#68-thumbnails). | new |
| `recent` | `~/.local/share/recently-used.xbel` read, and write when a pick returns files — so GTK apps' Recent and ours agree. | new |
| `places` | XDG user dirs (`user-dirs.dirs`), GTK bookmarks (`~/.config/gtk-3.0/bookmarks`) read/write, mounts from `/proc/self/mountinfo` + `statvfs` free space, removable block devices from `/sys/block`. | new |
| `watch` | inotify on the open folder(s); coalesced (16 ms) change batches. | builds on `enso_ui::watch` |
| `search` | Name search (fuzzy over a walk), content search (literal and regex, `memchr`-fast, binary-file skip), and query syntax (`type:image size:>10M modified:<7d`). | new |
| `archive` | List / extract / create zip, tar, tar.gz, tar.bz2, tar.xz, tar.zst (decode). | new, later phase |
| `uri` | `file://` URIs both ways, percent-encoding, the portal's NUL-terminated byte-array paths. | new |

### 4.4 `enso-ui` additions

The current runtime is one layer-shell surface per process. enso-files
needs a normal window. `smithay-client-toolkit` 0.21 (already a dependency)
has every piece; niri 26.04 advertises every protocol needed except
`xdg_wm_dialog_v1` (checked against the binary):

| Addition | Protocol | Niri? |
| --- | --- | --- |
| Window role: `SurfaceSpec` gains `Role::Window { app_id, title, min_size, parent }` beside the layer roles; resizable, maximise/fullscreen states, configure-driven size | `xdg_shell` | yes |
| Parent from another process (`wayland:<handle>` in the portal request) | `zxdg_importer_v2` | yes |
| Server-side decorations off: we draw our own title row, niri draws the border/focus ring | `zxdg_decoration_manager_v1` | yes |
| Modal hint | `xdg_wm_dialog_v1` | **no** — niri floats windows that have a parent anyway; we also ship a window rule ([§9](#9-system-integration)) |
| Clipboard: copy/paste text and files (`text/uri-list`, `x-special/gnome-copied-files` so Nautilus-style cut/paste interoperates) | `wl_data_device` | yes |
| Middle-click paste of paths | `zwp_primary_selection_v1` | yes |
| Drag and drop, both directions | `wl_data_device` DnD | yes |
| Pointer shapes (resize edges, text I-beam, grab, not-allowed, copy) | `wp_cursor_shape_v1` | yes |
| Focus-stealing-safe activation (FileManager1 `ShowItems` raising a window) | `xdg_activation_v1` | yes |
| Crisp text at fractional scales | `wp_fractional_scale_v1` + `wp_viewporter` | yes (also on enso-ui's roadmap) |
| Input methods in text fields (compose, CJK) | `zwp_text_input_v3` | yes (later phase) |

Toolkit-level, independent of protocols:

- **Keys**: `F1`–`F12`, `Insert`, `Menu`, `Space` as a named key, keypad
  Enter; `Mods` already has what's needed.
- **Pointer**: click counting (double/triple, configurable interval and
  slop), back/forward buttons (`BTN_SIDE`/`BTN_EXTRA`), wheel vs touchpad
  distinction (`axis_source`, `value120`) so wheel scrolls by rows and
  touchpads scroll by pixels with kinetic follow-through.
- **Shaped-text cache**: keyed by (string, size, weight, font, max width);
  LRU, a few thousand entries; invalidated on font or theme change.
- **Damage**: `Canvas` records the rectangles drawn since the last frame
  when the app opts in, and the runtime damages only those. Not needed at
  scale 1 ([§3](#3-measured-baseline)); worth having for scale 2 and for
  battery.
- **`widget` module** — reusable by every enso tool, styled only from tokens:
  `TextField` (caret, selection, word motions, clipboard, undo, IME later),
  `Button`, `IconButton`, `Toggle`, `Chip`, `Dropdown`, `Menu` (with
  submenus, mnemonics, separators, disabled items), `Tooltip`, `Scroll`
  (Tween-driven smooth offset + thin auto-hiding scrollbar with drag),
  `Splitter`, `ProgressPill`, `Dialog` (card over a scrim, focus-trapped).
  Each widget is a plain struct with `event(&mut self, …) -> Response` and
  `draw(&self, canvas, rect, now)`; no retained widget tree.

### 4.5 Inside `enso-files`

- **`state`** — pure data, no drawing: windows → tabs → panes; each pane has
  a location, a listing, a view (list/grid/columns), a sort, a filter, a
  cursor, a selection, a scroll position, and history (back/forward). Fully
  unit-testable.
- **`action`** — one `enum Action` for every command (`Open`, `Rename`,
  `ToggleHidden`, `SelectRange`, `NewTab`, …). Keys, mouse, context menus,
  the command palette, and the help overlay all go through it. The keymap
  is a table from chord to `Action`, overridable in config.
- **`jobs`** — worker threads (listing, stat pool, thumbnails, search,
  file operations, recursive sizes) report through `Proxy` messages tagged
  with a generation number, so results for a folder you've already left are
  dropped.
- **`view`** — layout and drawing. Drawing records hit regions
  (`Vec<(Rect, Target)>`), so pointer handling asks "what's under (x, y)"
  rather than redoing layout.
- **`portal`** — the request/response pipe protocol and option mapping.
- **`service`** — the D-Bus service (`zbus`, already in the workspace).

## 5. Design

### 5.1 The manager window

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│  ‹  ›   ( ⌂ ren › dev › enso › crates )                    ⌕ filter…    ☰  ▦  ◫  ⋯  │  title row: history, path pills,
├────────────┬─────────────────────────────────────────────────────────┬───────────────┤  filter, view switch, preview, more
│ ⌂ Home     │  Name                         Size        Modified       │               │
│ ↓ Downloads│ ▸ enso-bar                     —          2 h ago        │   [preview]   │
│ ▤ Documents│ ▸ enso-core                    —          yesterday      │               │
│ ▧ Pictures │▌▸ enso-ui                      —          yesterday      │  enso-ui      │  cursor: pill tint + accent bar
│ ◷ Recent   │ ✓ Cargo.toml                 1.2 kB       Mon            │  folder, 16   │  selected: check + soft tint
│ ★ dev      │ ✓ README.md                  8.4 kB       Nov 12         │  items, 214kB │
│ ★ sumi     │   rustfmt.toml                 74 B       Oct 30         │               │
│ ─────────  │                                                          │               │
│ ⛁ nvme 403G│                                                          │               │
│ ⏏ USB 14G  │                                                          │               │
│ 🗑 Trash   │                                                          │               │
├────────────┴─────────────────────────────────────────────────────────┴───────────────┤
│  2 selected · 9.6 kB                                         16 items · 403 GB free   │  status row
└──────────────────────────────────────────────────────────────────────────────────────┘
```

### 5.2 The picker

The same window, with a bottom bar and nothing that can damage files (no
delete, no move — rename and new folder stay, they're part of saving).

```
┌──────────────────────────────────────────────────────────────────────────────┐
│  Upload a file — file.io                                                    │  title from the request
│  ‹  ›   ( ⌂ ren › Pictures )                         ⌕ filter…    ☰  ▦  ◫   │
├────────────┬─────────────────────────────────────────────────────────────────┤
│  …sidebar… │  …listing, same as the manager…                                 │
├────────────┴─────────────────────────────────────────────────────────────────┤
│ ( samurai_bebop.png ✕ )( girl_smoking.png ✕ )( notes.txt ✕ )   from 2 folders │  the basket: picks from
│  Images ▾      ☐ Open read-only            [ Cancel ]   ( Upload 3 files )   │  anywhere, removable chips
└──────────────────────────────────────────────────────────────────────────────┘
```

Save mode replaces the basket with a name field (stem pre-selected, so
typing replaces `untitled` but keeps `.pdf`), and the filter dropdown sets
the extension as you change it.

### 5.3 Visual rules

- **Surfaces**: window background `base`; sidebar and preview on `surface`;
  menus, tooltips, dialogs on `overlay` with `Shadow::FLOATING` and
  `radius::CARD`; rows and buttons `radius::CONTROL`; path segments and
  chips are pills.
- **Rows**: 32 px (comfortable) or 26 px (compact, config); generous
  `space::LG` gutters. Cursor row: `accent` tinted at `opacity::TINT`, with
  a 3 px pill bar on the leading edge that **glides** between rows
  (`Transition::SELECT`). Selected rows: a soft `iris` tint and a check in
  place of the kind icon; cursor + selected stack. Hover: a whisper of
  `highlight_low` (`Transition::HOVER`).
- **Focus**: a 2 px `stroke::FOCUS` ring in `accent` around the focused
  region (sidebar / listing / field), so Tab-cycling is always visible.
- **Kinds by colour**, from the palette, consistent everywhere (icons,
  grid placeholders, search chips): folders `accent`, images `iris`,
  video `love`, audio `rose`, documents `pine`, code/text `foam`, archives
  `gold`, executables `love`, everything else `subtle`.
- **Type**: names in `BODY`, secondary columns `SMALL` in `subtle`, headings
  `HEADING`; the path's last segment `medium` weight.
- **Motion**: folder change is a 180 ms cross-fade + 8 px slide in the
  direction of travel (into a child slides left, back slides right); view
  switch `REFLOW`; preview pane opens with `REVEAL`; the basket's chips pop
  in with `OutBack`; deleted rows collapse with `REFLOW` while their
  neighbours close the gap; toasts for finished operations glide like
  enso-notify's. `[motion]` config (including reduced motion) applies.
- **Empty and error states** are drawn, not blank: "Nothing here yet",
  "No matches for 'xyz' — Esc clears the filter", "You can't read this
  folder" with the owner shown.

### 5.4 New figures

Drawn in `enso_ui::figure` on the 24-unit grid with the one pen:

- Places: `Home`, `Download`, `Document`, `Image`, `Music`, `Video`,
  `Desktop`, `Recent` (clock), `Star`, `Trash`, `TrashFull`, `Drive`,
  `Usb`, `Network`, `Eject`.
- Kinds: `FileImage`, `FileVideo`, `FileAudio`, `FileText`, `FileCode`,
  `FileArchive`, `FilePdf`, `FileFont`, `FileExec`, `Symlink` (badge),
  `FolderOpen`.
- Actions: `ArrowUp`, `ArrowLeft`, `ArrowRight`, `Plus`, `FolderPlus`,
  `Copy`, `Cut`, `Paste`, `Pencil`, `Link`, `Terminal`, `Refresh`,
  `Filter`, `Sort`, `List`, `Grid`, `Columns`, `Preview` (eye),
  `Hidden` (eye-off), `Info`, `More` (three dots), `Tag`, `Undo`,
  `Archive`, `Extract`, `Upload`, `Save`.

## 6. Features

Phases (P1…P6) are defined in [§10](#10-phases). Everything here has a home
in a phase; nothing is "someday".

### 6.1 Navigation

- Path bar of pills; click a segment to go there, right-click a segment for
  its siblings (a quick menu of folders beside it), drop files on it to
  move/copy there. **P1**
- Edit the location as text: `Ctrl+L` turns the pills into a field with
  `~` expansion and Tab completion (fuzzy, folders first). Pasting a path or
  `file://` URI navigates; pasting a path to a file selects it. **P1**
- Back / forward / up with history per tab; mouse back/forward buttons.
  Leaving a folder and coming back restores its cursor and scroll. **P1**
- Sidebar places: Home, XDG user dirs that exist, Recent, bookmarks, mounts,
  Trash; collapsible (`Ctrl+B`), resizable by its splitter. **P1** (mounts,
  trash: **P2**)
- Jump keys: `g h` home, `g d` downloads, `g p` pictures, `g c` ~/.config,
  `g t` trash, `g r` recent, `g /` root, `g g` top, `G` bottom, `~` home;
  user-defined jumps in config. **P1**
- Recent folders list (frecency), shown in the location field's
  completions and the command palette. **P4**
- Follow symlinks into their target, or "go to the real folder"
  (`Ctrl+Shift+Enter`). **P2**

### 6.2 Views

- **List**: sortable, resizable, reorderable columns: name, size, modified,
  created, accessed, kind, extension, permissions, owner. Folders show item
  counts in the size column (counted lazily). **P1**
- **Grid**: thumbnails, three sizes plus `Ctrl+wheel` / `Ctrl+=`/`Ctrl+-`
  zoom; names wrap to two lines then ellipsize in the middle so extensions
  survive (`a_very_long…name.png`). **P3**
- **Columns** (Miller, as in Finder/ranger/yazi): parent | current | preview;
  `h`/`l` move between them. **P3**
- Hidden files toggle (`Ctrl+H` and `.`), remembered per folder or globally
  (config). **P1**
- Sort menu and per-column header click; folders first (toggle); natural
  numeric order. **P1**
- **Per-folder view memory**: view, sort, zoom, hidden — remembered for folders
  you changed, default elsewhere. **P4**
- Compact / comfortable density. **P1**

### 6.3 Selection

This is the centre of the picker, and of the request.

- Cursor and selection are separate: the cursor is where you are; the
  selection is what an action acts on. With nothing selected, actions act
  on the cursor.
- Keyboard: `Space` toggles the cursor item and moves down; `Shift+↑/↓`,
  `Shift+PgUp/PgDn/Home/End` extend; `v` starts a visual range (yazi), `V`
  selects the folder; `Ctrl+A` all, `Ctrl+Shift+A` / `Esc` none; `*`
  inverts; `+`/`-` select/deselect by glob ("`*.png`"). **P1**
- Mouse: click selects; `Ctrl+click` toggles; `Shift+click` range;
  rubber-band by dragging in empty space (with auto-scroll at the edges,
  `Ctrl`-drag adds to the selection); a check circle appears on hover in
  grid view for touch-style toggling. **P1** (rubber band **P3**)
- **Selection across folders — the basket**: the selection survives
  navigation. A bar shows "5 selected in 3 folders" with removable chips;
  click a chip to reveal that file. In the picker, the basket *is* the
  answer. In the manager, it's what copy/move/trash/compress act on.
  **P1**
- Select by kind ("all images here"), by date ("modified today") from the
  command palette. **P4**

### 6.4 Search and filter

- **Filter as you type**: `/` (or click the filter field) — fuzzy over the
  current folder, matches highlighted in names, instant, never leaves the
  folder; `Esc` clears. **P1**
- **Type-ahead find** (Nautilus/Finder habit): config option
  `type_to_find = true` makes plain letters jump instead of acting as
  commands; off by default to keep `hjkl` and friends. **P1**
- **Recursive search** (`Ctrl+F` or `//`): fuzzy name search below the
  current folder, streaming results as the walk finds them, gitignore-aware
  (the launcher's walker), results show their folder. Uses the launcher's
  saved index for `~` when it is fresh. **P4**
- **Content search** (`Ctrl+Shift+F`): literal or regex inside files, with
  the matching line shown as the result's subtitle; skips binary files. **P4**
- **Query words**: `type:image`, `ext:rs`, `size:>10M`, `modified:<7d`,
  `name:` — usable in the filter and in search. **P4**
- **Saved searches** as sidebar entries. **P6**

### 6.5 Opening

- `Enter` / double-click: open folders in place, files with their default
  application (MIME database + `mimeapps.list`, shared with the launcher). **P2**
- `Ctrl+Enter` / middle-click: folder in a new tab (with tabs, **P4**).
- **Open with…** submenu: every application that declares the type, the
  default first; "Other application…" with a fuzzy app list (the launcher's);
  "Always use for .png" writes `mimeapps.list`. **P2**
- Open a terminal here (`F4`, Dolphin's key; config `terminal`, default
  `alacritty`). **P2**
- Open several selected files at once — grouped by application, one launch
  per application with `%F`/`%U`. **P2**
- Run executables only on explicit choice ("Run" vs "Open"), never by
  double-click by default. **P2**

### 6.6 File operations

All operations run on worker threads, never block the window, show a
progress pill in the status row (expandable into a list with per-job
pause/cancel), and finish with a toast.

- Copy, cut, paste (`Ctrl+C`/`X`/`V`) — through the Wayland clipboard in the
  formats Nautilus, Dolphin, and terminals understand, so copy in
  enso-files → paste in Nautilus works, and copy a path in a terminal →
  `Ctrl+V` here goes there. `Ctrl+Shift+C` copies paths as text. **P2**
- Paste as symlink (`Ctrl+Shift+V`). **P2**
- Move to trash (`Delete`), delete permanently (`Shift+Delete`, confirmed,
  with a count and total size). **P2**
- Rename in place (`F2` / click the name of the cursor item after a pause):
  stem selected, `Tab` jumps to the extension, invalid names flagged as you
  type, collisions refused. **P2**
- New folder (`Ctrl+Shift+N`), new empty file (`Ctrl+Alt+N`), from
  templates in `~/Templates`. **P2**
- Duplicate (`Ctrl+Shift+D` → "name (copy).ext"). **P2**
- **Conflicts**: a dialog with both files side by side (size, date,
  thumbnails): Replace / Skip / Keep both / Merge folders, "for all
  remaining". **P2**
- **Undo / redo** (`Ctrl+Z` / `Ctrl+Shift+Z`) for rename, move, copy (undo
  = delete the copies), new, trash (undo = restore). Journal survives until
  the window closes. **P2**
- Fast paths: same-filesystem moves are renames; copies use
  `copy_file_range` (reflinks on btrfs/XFS), sparse files stay sparse,
  modes and mtimes preserved, xattrs best-effort. **P2**
- Permissions errors are per-file and listed at the end, not a modal per
  failure. **P2**
- **Bulk rename** (`F2` with a selection, or `Ctrl+F2`): two ways —
  (a) in-app: find/replace with regex and capture groups, case changes,
  numbering `{n:03}`, date tokens from mtime, live before→after preview,
  conflict detection; (b) in `$EDITOR`: one name per line, saved and
  applied (yazi/vidir style), cycles (a→b, b→a) handled through temporary
  names. **P4**
- Compress (`.zip`, `.tar.gz`, `.tar.xz`), extract here / to a folder,
  browse an archive like a folder (read-only) with preview. **P5**

### 6.7 Preview

- **Preview pane** (`i` for inspect, or the eye button): text and code (first 64 kB,
  monospace, syntax-coloured from the palette for common languages),
  images (fitted, with dimensions), folders (their contents and total
  size), fonts (a sample rendered with cosmic-text in that font), PDFs
  (first page — see risks), archives (their listing), audio/video
  (container metadata: duration, codecs, dimensions, cover art when
  embedded), and for everything else the kind, size, dates, permissions,
  and MIME type. **P3**
- **Peek** (`Shift+Space`, Finder's Quick Look): a large preview card over a
  scrim; `←/→` or `j/k` walk files while it's open; `Esc` closes. **P3**
- Preview loads asynchronously and is cancelled when the cursor moves on;
  holding `↓` through a folder never queues work. **P3**

### 6.8 Thumbnails

- Formats: PNG, JPEG (already decoded in `enso-ui`'s `picture`), WebP,
  GIF (first frame), BMP, ICO, QOI, SVG (rendered with resvg, which already
  builds on tiny-skia), and anything that already has a freedesktop
  thumbnail in `~/.cache/thumbnails` from another program (video, PDF). **P3**
- **Shared cache** per the freedesktop thumbnail spec, so thumbnails made
  once serve every program that reads the spec; size-appropriate
  (128/256/512/1024) for the zoom level and scale. **P3**
- Decode on a pool sized to cores − 2, visible items first, then a little
  ahead in the scroll direction; cancelled when scrolled past; EXIF
  rotation honoured; never decode anything over a size cap (config) into
  memory. **P3**
- An in-memory LRU of ready-to-blit premultiplied thumbnails keyed by
  (path, mtime, size). **P3**

### 6.9 Places, bookmarks, recents, trash, drives

- Bookmarks: `Ctrl+D` bookmarks the folder; drag a folder onto the sidebar;
  reorder by drag; rename; stored in GTK's bookmarks file so GTK dialogs and
  Nautilus see the same list. **P4**
- Recent: files opened or picked through enso-files, plus every other
  program's `recently-used.xbel` entries; grouped by day. **P4**
- Trash: browse with original location and deletion date; restore
  (`Ctrl+Z` or "Restore"), delete forever, empty (confirmed); shows the full
  icon when non-empty. **P2**
- Drives: mounted filesystems with free space bars (`statvfs`), hiding
  system mounts (`/boot`, `/proc`, tmpfs…). Mounting and ejecting removable
  drives goes through `udisks2` over D-Bus: mount, unmount, eject, power
  off. **P5**
- Free space in the status row for the current filesystem. **P1**

### 6.10 Tabs and panes

- Tabs (`Ctrl+T`, `Ctrl+W`, `Ctrl+Tab`, `Alt+1…9`, middle-click to close,
  drag to reorder, drop files on a tab to move/copy there). **P4**
- **Split view** (`F3`, Dolphin's key): two panes side by side, `Tab`
  switches focus, `F5` copies and `F6` moves the selection to the other
  pane (Midnight Commander habits). **P4**
- Session restore: windows, tabs, and locations on next launch (config). **P6**

### 6.11 Information and permissions

- Properties (`Alt+Enter` or `Ctrl+I`): kind, MIME, size (recursive for
  folders, computed live), location, dates, owner/group, permissions as
  three readable rows (Owner/Group/Others × read/write/run) and as octal,
  editable; symlink target; filesystem and free space. Multiple selection:
  totals and common permissions. **P4**
- "Make executable" one-key toggle (`Ctrl+Shift+X`). **P4**
- Checksums (SHA-256, MD5) on request, computed on a worker. **P6**

### 6.12 Command palette and help

- `Ctrl+P` / `Ctrl+Shift+P` / `:`: a launcher-style fuzzy list of every
  `Action`, with its key shown — the discovery path for mouse users and for
  rarely-used commands. Also takes "go to" (folders by frecency) and "select
  by…" queries. **P4**
- `?` or `F1`: a key map overlay, grouped, generated from the live keymap
  (so remapped keys show correctly). **P1**

### 6.13 Drag and drop

- Drag files out to any application (Firefox upload fields, editors,
  terminals get paths) as `text/uri-list`. **P5**
- Drop files in from any application: on the listing (into this folder), on
  a folder row, on a path segment, on a sidebar place, on a tab. `Ctrl`
  copies, `Shift` moves, default by filesystem (same fs → move, else copy),
  as GTK and Dolphin do. Spring-loaded folders open while you hover. **P5**
- Inside the picker, dropping files from elsewhere adds them to the basket. **P5**

### 6.14 Live updates

- The open folder (and each visible pane) updates as files appear, change,
  and disappear, without losing the cursor or the scroll position; new
  files fade in with `REVEAL`. **P2**
- Recursive folder sizes and item counts update as they finish. **P4**

### 6.15 The portal, precisely

Interface `org.freedesktop.impl.portal.FileChooser`, every method and option current xdg-desktop-portal (1.22) passes:

| Method | Options honoured | Answer |
| --- | --- | --- |
| `OpenFile` | `handle_token`, `accept_label`, `modal`, `multiple`, `directory`, `filters`, `current_filter`, `choices`, `current_folder` | `uris`, `choices`, `current_filter` |
| `SaveFile` | `accept_label`, `modal`, `filters`, `current_filter`, `choices`, `current_name`, `current_folder`, `current_file` | `uris`, `choices`, `current_filter` |
| `SaveFiles` | `accept_label`, `modal`, `choices`, `current_folder`, `files` | `uris` (folder + each name) |

- `parent_window` `wayland:<handle>` → `zxdg_importer_v2.import_toplevel`
  → `set_parent_of`: the picker floats over its caller, centred on it.
  `x11:` handles (Xwayland apps) are ignored gracefully.
- Filters (`a(sa(us))` — globs and MIME types) → the filter dropdown;
  "All files" always added unless the request makes it impossible.
- `choices` → checkboxes (boolean) and dropdowns (string) in the bottom bar.
- `Request.Close` → the child process ends; no answer is sent.
- Picked files are added to `recently-used.xbel`.
- The window opens in the last folder used by **that application**
  (keyed by app id from the request), else `current_folder`, else
  Downloads for open, Documents for save. Config can pin either.

Also on the service: **`org.freedesktop.FileManager1`** —
`ShowFolders`, `ShowItems` (open the folder with the items selected — this
is Firefox's "Show in folder" and Chromium's "Show in folder"),
`ShowItemProperties`. **P2**

### 6.16 The command-line picker

```sh
enso-files pick                       # one file → stdout
enso-files pick --multiple -0         # many, NUL-separated
enso-files pick --directory           # a folder
enso-files pick --save report.pdf     # a path to save to
enso-files pick --filter 'Images:*.png,*.jpg,image/webp'
```

Exit status 0 with answers, 1 when cancelled. For scripts, Neovim, and
`xdg-desktop-portal-termfilechooser`-style setups. **P1**

### 6.17 Settings

The file manager's own settings, in the window — not in enso-ctl, which
is the desktop's. **P2**

- Opened with `Ctrl+,`, a gear at the end of the title row, or the
  command palette; a card over the window (`radius::CARD`, scrim behind),
  sections down the left, keyboard-navigable like everything else.
- Sections: **View** (default view, density, dotfiles, sort, folders
  first), **Behaviour** (type-to-find, single click, confirm trash, vim
  keys, terminal), **Picker** (where it opens for open and for save),
  **Version control** ([§6.18](#618-version-control-git): on/off, the
  global identity, this repository's identity).
- **Layers**: `config.toml`'s `[files]` is the hand-written default; what
  is changed in the window is written to `~/.local/state/enso/files.toml`
  and wins over it, the way `enso-ctl theme set` overrides the configured
  theme. A setting coming from `config.toml` says so, and "Reset" drops
  the override.
- Changes apply at once to every open enso-files window (they watch the
  state file).

### 6.18 Version control (Git)

Opt-in, and quiet in the window: the branch where you are, and the few
things a person reaches for — growing, later, into GitHub (first) and
GitLab accounts with their remote repositories, so a folder like
`~/dev/enso` or `~/dev/sumi` can be cloned, published, and kept in step
from the file manager. **Basics P2; remotes and accounts P5.**

**How:** the `git` binary does the work, behind the scenes (`git -C
<folder> …` with `--porcelain` and `-z` output, parsed; never a shell).
This is the one deliberate exception to "no CLI wrappers": Git's own
tool is the reference implementation of its formats and protocols, and
reimplementing pushes and credential helpers would only be worse. If
`git` isn't installed, version control says so in Settings and stays off.

**Asked once.** The first time the file manager opens (never inside the
picker — a stranger's upload dialog is no place for questions), a small
card asks whether to turn on version control. **No** turns it off
completely: `git` is never run, no branch is shown, no menu item appears.
The answer is kept in the settings layer, and Settings → Version control
turns it back on (or off) at any time.

**Who you are.** Yes leads to the identity step: if a global identity
exists (`git config --global user.name`/`user.email`), it is shown and
used by default. If not, the card offers to set one — name and email,
written with `git config --global` — or to skip and set identities per
repository instead. Each repository can have its own (`git config
user.name` inside it), shown and edited in Settings → Version control
while inside it, and in the folder's properties; the default is to
inherit the global one.

**Basics (P2):**
- **The branch**: inside a repository, a small pill in the status row
  (`⎇ main`, the short commit when detached), with `↑2 ↓1` when it is
  ahead of or behind its upstream. Read from `git status --porcelain=v2
  --branch`, off the drawing thread, refreshed when `.git/HEAD` or the
  index changes.
- **Make a repository**: "Initialise Git repository here" — the folder's
  context menu and the command palette — runs `git init` (honouring
  `init.defaultBranch`), then offers the identity step if there is no
  identity to use.
- **Switch branch**: clicking the pill opens a menu of local branches
  (current one ticked) and "New branch…".
- Nothing else in the window: no per-file badges, no staging, no commit
  history. The pill is the only thing always on screen.

**Remotes and accounts (P5):**
- **Remote branches**: the branch menu lists the remote's branches too
  (after `git fetch`, run on request or when the menu opens if the last
  fetch is old), and checking one out makes a tracking branch.
- **Accounts**: Settings → Version control → "Connect GitHub" (and
  GitLab). GitHub's device flow — the window shows a short code, you
  approve it in the browser — so no password ever passes through
  enso-files; GitLab via its OAuth flow, including self-hosted instances.
  Tokens live in the desktop's Secret Service (gnome-keyring here),
  reached over D-Bus, never in a file. Git is then given them through a
  credential helper (`enso-files --git-credential`), so `git push` from
  the terminal benefits as well.
- **With an account**: clone one of your repositories into a folder
  (a searchable list of them, fuzzy like everything else); publish a
  folder that is a repository but has no remote — create it on GitHub
  (private by default), add it as `origin`, push; push and pull the
  current branch from the pill's menu; "Open on GitHub" for the folder.
- **Quiet still**: all of it lives in the pill's menu, the folder's
  context menu, the command palette, and Settings; nothing new is
  permanently on screen.

## 7. Keys and the mouse

### 7.1 Keyboard (defaults; every one remappable)

| Keys | Action |
| --- | --- |
| `↑` `↓`, `k` `j`, `Ctrl+p` `Ctrl+n` | cursor up / down (suite-wide navigation keys) |
| `←` `h`, `Backspace`, `Alt+↑` | parent folder |
| `→` `l`, `Enter` | open folder / open file (picker: choose) |
| `Alt+←` `Alt+→`, `H` `L` | back / forward |
| `PgUp` `PgDn` | page up / down |
| `Home` `End`, `g g` `G` | first / last |
| `Space` | toggle selection, move down |
| `Shift+↑/↓/PgUp/PgDn/Home/End` | extend selection |
| `v` / `V` | visual range / select all |
| `Ctrl+A`, `Esc` | select all / clear (then: close filter → clear → cancel picker) |
| `/`, `Ctrl+F`, `Ctrl+Shift+F` | filter here / search below / search contents |
| `n` `N` | next / previous match |
| `Ctrl+L`, `:` | edit location / command palette |
| `Ctrl+P` | command palette |
| `g` + letter | jump (home, downloads, pictures, config, trash, recent, root) |
| `Ctrl+H`, `.` | hidden files |
| `Ctrl+1` `Ctrl+2` `Ctrl+3` | list / grid / columns |
| `Ctrl+=` `Ctrl+-` `Ctrl+0` | zoom in / out / reset |
| `i`, `Shift+Space` | preview pane / peek |
| `s` + key | sort by name / size / modified / kind / ext; again reverses |
| `F2`, `Ctrl+F2` | rename / bulk rename |
| `Ctrl+C` `Ctrl+X` `Ctrl+V`, `Ctrl+Shift+V` | copy / cut / paste, paste as link |
| `y y`, `d d`, `p p` *(when `vim_keys = true`)* | yank / cut / paste, yazi-style |
| `Ctrl+Shift+C` | copy path(s) as text |
| `Delete`, `Shift+Delete` | trash / delete permanently |
| `Ctrl+Shift+N`, `Ctrl+Alt+N` | new folder / new file |
| `Ctrl+D`, `Ctrl+Shift+D` | bookmark this folder (as in GTK and browsers) / duplicate |
| `Ctrl+Z`, `Ctrl+Shift+Z` | undo / redo |
| `Alt+Enter`, `Ctrl+I` | properties |
| `F3`, `Tab`, `F5`, `F6` | split view / switch pane / copy / move to other pane |
| `F4` | terminal here |
| `Ctrl+T` `Ctrl+W` `Ctrl+Tab` `Alt+1…9` | tabs |
| `Ctrl+B` | sidebar |
| `Ctrl+R` | reload |
| `?`, `F1` | key map |
| `Ctrl+,` | settings |
| `Ctrl+Q` | close window (picker: cancel) |

Picker-only: `Enter` chooses (with a selection: the selection; without: the
cursor, entering folders unless `--directory`); `Ctrl+Enter` chooses the
current folder in folder mode; `Tab` moves between listing, basket, name
field, filter dropdown, and buttons.

### 7.2 Mouse and touchpad

| Gesture | Action |
| --- | --- |
| Click | select (replace) and move cursor |
| `Ctrl`+click / `Shift`+click | toggle / range |
| Double-click | open (picker: choose a file, enter a folder) |
| Drag on empty space | rubber-band select (`Ctrl` adds) |
| Drag items | drag and drop (move/copy/link by modifiers) |
| Right-click | context menu for the item(s), or for the folder on empty space |
| Middle-click | open folder in new tab; on a tab: close it |
| Back / forward buttons | history |
| Wheel / two-finger scroll | scroll (rows for wheels, pixels + inertia for touchpads) |
| `Ctrl`+wheel / pinch | zoom |
| Click column header / drag its edge | sort / resize; drag the header to reorder |
| Drag a splitter | resize sidebar / preview / panes |
| Hover a truncated name | tooltip with the full name (after 500 ms) |
| Hover a folder while dragging | it opens after 700 ms (spring-loaded) |
| Click a path pill / right-click it | go there / siblings menu |

Every context-menu entry shows its key, so the mouse teaches the keyboard.

## 8. Performance

### 8.1 Budgets

| Moment | Budget | How it's checked |
| --- | --- | --- |
| Portal request → first frame of the picker | ≤ 60 ms | `enso-files --bench open` timestamps; probe via D-Bus |
| Folder change, ≤ 5,000 entries, warm | ≤ 8 ms to a complete frame | bench |
| Folder change, 50,000 entries, warm | ≤ 25 ms to first frame with names; metadata ≤ 60 ms | bench on a generated folder |
| Any frame while scrolling, scale 1 | ≤ 3 ms (144 Hz has 6.9) | frame timer in debug overlay (`Ctrl+Alt+F12`) |
| Filter keystroke, 50,000 entries | ≤ 4 ms | bench |
| Recursive name search, `~` | first results ≤ 30 ms, streaming | bench, compared with `enso-launch --find` |
| Idle CPU | 0 — no timers, no polling | `top`; inotify and frame callbacks only |
| Idle memory, service | ≤ 4 MB RSS | `ps` |
| Memory, window on a 50,000-entry folder | ≤ 80 MB RSS | `ps` |

### 8.2 Techniques

- **Listing**: `getdents64` via `rustix` for names and `d_type`; the first
  frame needs nothing else. `statx` on a pool (cores − 2 threads) with
  only the fields wanted (`STATX_SIZE|STATX_MTIME|STATX_MODE|…`), results
  merged in batches per frame, never per entry.
- **Names stored once**: an arena of bytes + offsets per listing (as the
  launcher's index is one string) — no `PathBuf` per entry, no per-entry
  allocation. Names that aren't UTF-8 are kept as bytes and shown with
  replacement marks, never dropped.
- **Sorting**: keys precomputed (case-folded, digit runs parsed) once per
  listing; sort indices, not entries; folders-first is a key bit.
- **Filtering**: the fuzzy scorer over the arena, parallel above ~20,000
  entries, results as an index list.
- **Drawing**: only visible rows are laid out; shaped text is cached; icons
  are cached per (figure, size, colour) as tiny pixmaps; thumbnails are
  pre-fitted and premultiplied so drawing is one blit.
- **No work for the past**: every job carries a generation; results for a
  folder, filter, or cursor position you've left are dropped on arrival and
  their workers check a cancel flag between items.
- **Frames on demand**: `Paint::Idle` whenever nothing moves; animations
  run on frame callbacks; batched worker results request at most one frame.
- **Benchmarks in the binary**: `enso-files --bench <list|filter|search|open> PATH`
  prints timings, like `enso-launch --find`, so regressions are visible
  from a terminal.

## 9. System integration

enso-files sets itself up; no dotfiles repository is involved, and every
enso tool stays swappable. `enso-files portal enable` writes, all in the
user's own directories:

1. **D-Bus activation**: `~/.local/share/dbus-1/services/org.freedesktop.impl.portal.desktop.enso.service`,
   `Exec=<this binary> --service`.
2. **A portals directory**, `~/.local/share/enso/portals`: links to every
   system `.portal` file, enso's own `enso.portal`, and the routing —
   the desktop's `<desktop>-portals.conf` (the user's own if they have one,
   else the system's) with `org.freedesktop.impl.portal.FileChooser=enso`.
   With `XDG_DESKTOP_PORTAL_DIR` set, the portal reads both from there.
3. **A drop-in**, `~/.config/systemd/user/xdg-desktop-portal.service.d/enso.conf`,
   setting `XDG_DESKTOP_PORTAL_DIR`.

Then it reloads the session bus and systemd and restarts the portal.
`enso-files portal disable` removes all three; `portal status` says which
it is. Running `enable` again refreshes the links after packages change.

Also:

4. **Default folder handler**: `inode/directory=enso-files.desktop` in
   `mimeapps.list`, and a desktop entry — offered by enso-files itself
   (P2), never assumed.
5. **Floating**: a picker started by an application has a parent window,
   and niri floats it; pickers carry their own app id, `enso-files-picker`,
   for anyone who wants a window rule for the rest.
6. **Firefox** already routes through the portal; nothing to change.

## 10. Phases

Each phase ends usable, tested, committed, and measured against §8.

**P0 — Foundations.**
`enso-ui`: window role, xdg-foreign parenting, decorations off, cursor
shapes, more keys, click counting, wheel/touchpad scroll, shaped-text cache,
`widget::{TextField, Button, Toggle, Dropdown, Menu, Tooltip, Scroll,
Splitter, Dialog}`. `enso-fs`: move launcher code over (launcher behaviour
and tests unchanged), add `list`, `sort`, `uri`, `places` (user dirs +
statvfs). `enso-files --bench list`.
*Done when*: an empty enso window opens as a tiled niri window, resizes,
themes live; the launcher passes its tests on `enso-fs`; listing 50k
entries benches within budget.

**P1 — The picker.**
List view, path pills + location field, sidebar (places, user dirs),
history, hidden toggle, sorting, filter-as-you-type, selection (keyboard,
click, Ctrl/Shift-click), the basket, filters and choices, open/save/
save-many, the help overlay, `enso-files pick`, `--service` with
`impl.portal.FileChooser`, install wiring. Rename and new folder in save
mode.
*Done when*: Firefox's upload and download dialogs are enso-files; picking
three files from two folders uploads all three; saving with a filter fixes
the extension; everything works with keyboard only and with mouse only;
the GTK fallback line restores the old picker.

**P2 — The manager.**
Open / open-with / set default, terminal here, copy/cut/paste (interop),
trash + trash view + restore, delete, rename, new, duplicate, conflicts,
undo/redo, progress pill and toasts, live updates, context menus,
FileManager1, symlinks, run-vs-open, mounts in the sidebar (read-only),
the settings panel and its layer ([§6.17](#617-settings)), and version
control basics ([§6.18](#618-version-control-git)): asked once, the
identity step, the branch pill, switching branches, initialising a
repository, per-repository identities.
*Done when*: a week of normal use needs no other file manager; "Show in
folder" from Firefox's downloads lands on the file, selected; answering
"no" to version control leaves no trace of Git anywhere in the window.

**P3 — Looking.**
Grid view and zoom, thumbnails (formats in §6.8, freedesktop cache),
preview pane, peek, Miller columns, rubber-band selection, the new figures
complete, motion polish (folder slide, row collapse, chip pops).
*Done when*: `~/Pictures` in grid scrolls at 144 Hz with thumbnails
appearing in scroll order and none decoded twice across runs.

**P4 — Power.**
Recursive and content search, query words, command palette, bulk rename
(in-app and `$EDITOR`), bookmarks (GTK file), recents (xbel), tabs, split
view with F5/F6, properties and permissions, per-folder view memory,
recursive sizes, frecent folders, select-by.

**P5 — Reach.**
Drag and drop both ways, archives (browse/extract/compress), removable
drives via udisks2, IME in text fields, fractional scaling, and Git's
remotes and accounts: remote branches, GitHub and GitLab sign-in, clone,
publish, push, pull ([§6.18](#618-version-control-git)).

**P6 — Nice to have, still real.**
Saved searches, session restore, checksums, emblems/tags (xattr
`user.xdg.tags`), duplicate finder.

## 11. Dependencies

Already in the workspace: `smithay-client-toolkit` (+ `wayland-protocols`
with staging/unstable), `calloop`, `kbvm`, `tiny-skia` (with `png`),
`zune-jpeg`, `cosmic-text`, `zbus`, `rustix`, `memchr`, `flate2`
(miniz_oxide), `serde`, `toml`, `lexopt`, `jiff`.

New, all pure Rust, each behind the feature that needs it:

| Crate | For | Phase |
| --- | --- | --- |
| `image-webp` | WebP thumbnails | P3 |
| `gif` | GIF first frame | P3 |
| `resvg` | SVG thumbnails (shares tiny-skia) | P3 |
| `md-5` | thumbnail cache file names (spec-mandated MD5) | P3 |
| `regex` | content search, bulk rename | P4 |
| `zip`, `tar`, `lzma-rs`, `ruzstd` | archives (xz and zstd decoding in pure Rust) | P5 |

No new system packages. Optional system services are used when present
(udisks2) and their absence is a drawn state, not an error.

## 12. Configuration

A `[files]` table, every key optional, mirrored into `config.example.toml`
(the test that keeps them in step applies):

```toml
[files]
# "list", "grid", or "columns".
view = "list"
# "comfortable" or "compact" rows.
density = "comfortable"
# Show dotfiles.
hidden = false
# "name", "size", "modified", "kind", or "extension"; and the direction.
sort = "name"
descending = false
folders_first = true
# Plain letters jump to matching names instead of acting as commands.
type_to_find = false
# yazi-style yy / dd / pp alongside Ctrl+C / X / V.
vim_keys = false
# Single click opens, as in some file managers.
single_click = false
# Thumbnails: on, and the largest file decoded for one.
thumbnails = true
thumbnail_max_mb = 64
# Ask before moving to the trash (deleting permanently always asks).
confirm_trash = false
# Used for "terminal here" and terminal applications.
terminal = "alacritty"
# Where the picker opens when the application hasn't been seen before.
open_in = "~/Downloads"
save_in = "~/Documents"
# Extra jump keys after `g`.
jumps = { w = "~/Pictures/Wallpapers", e = "~/dev/enso" }
# Remap or add keys: chord = action.
# keys = { "ctrl+e" = "terminal-here", "x" = "cut" }
```

## 13. Decisions

Settled on 2026-11-18:

1. **Portal registration** — user-level, no root, done by enso-files
   itself (`enso-files portal enable`/`disable`), never by a dotfiles
   repository: enso tools stay swappable ([§9](#9-system-integration)).
2. **udisks2** — installed on the system, so removable drives are mounted,
   unmounted, and ejected through it (P5). Its absence elsewhere is still a
   drawn state, not an error.
3. **Space** toggles selection; `Shift+Space` peeks.
4. **Name** — `enso-files`.
5. **Settings** live in the window, for the file manager only, as an
   override layer over `config.toml` ([§6.17](#617-settings)).
6. **Git** — opt-in: asked once, "no" means off entirely; a global
   identity by default, per-repository identities. Runs the `git` binary
   behind the scenes (the one exception to "no CLI wrappers"). Basics —
   the branch, initialising, switching — in P2; remote branches and
   GitHub (first) and GitLab accounts — clone, publish, push, pull — in
   P5 ([§6.18](#618-version-control-git)).

## 14. Risks

- **xdg-dialog missing in niri**: no compositor-enforced modality. Parenting
  via xdg-foreign still floats and centres the picker; the caller isn't
  blocked from input, which GTK on niri already lives with.
- **Video and PDF thumbnails** need decoders that don't exist in pure Rust
  at reasonable size; enso-files reads such thumbnails when another program
  has made them, and shows a coloured kind tile otherwise. A pure-Rust PDF
  first-page renderer (`hayro`) is worth evaluating in P3.
- **Network filesystems** (sshfs, NFS) work as plain mounts; GVFS-style
  `smb://` locations are out of scope without GVFS.
- **Text field quality** — selection, word motions, clipboard, IME — is
  most of what makes save dialogs feel right; it gets its own tests and a
  P0 slot, not a corner of P1.
- **Clipboard interop** formats differ between file managers; tested
  against Nautilus's and Dolphin's formats with recorded offers, not live
  apps.
- **Large-directory sorts by metadata** (size/date) can't show final order
  before `statx` finishes; the list shows name order first and re-sorts
  once, keeping the cursor on its file — one visible jump, animated with
  `REFLOW`, never several.

## 15. Testing

- `enso-fs`: unit tests in temp dirs for listing (odd names, non-UTF-8,
  permission-denied entries, symlink loops), sorting, trash round-trips
  (including cross-device), ops with injected failures, conflict policies,
  undo journal, MIME detection, thumbnail cache naming, xbel and bookmarks
  round-trips.
- `enso-files`: state-machine tests driving `Action`s and `KeyEvent`s
  (selection semantics, basket across folders, Esc ladder, picker answers
  for each portal option); headless render snapshots of every view, both
  themes (dark and Dawn), picker and manager, written to
  `target/snapshots/` like the other tools.
- Portal: the request → child → answer pipe tested without D-Bus; the D-Bus
  layer tested on a private bus when `dbus-daemon` is available, skipped
  otherwise.
- Benchmarks: `--bench` budgets checked in CI-style tests on generated
  folders (lenient bounds, to catch order-of-magnitude regressions).
- `cargo clippy --workspace --all-targets --all-features` and
  `cargo fmt --all` clean, as always.

## 16. Borrowed with thanks

- **Nautilus**: path pills, type-ahead, Recent, the clipboard format.
- **Finder**: Quick Look (peek), column view, "Show in folder" as a first-class act.
- **Dolphin**: split view, F3/F4, places, drop menus by modifier.
- **yazi / ranger / lf**: `hjkl`, Miller columns, async everything,
  bulk rename in `$EDITOR`, visual mode, previews that never block.
- **nnn**: speed as a feature; selection that crosses folders.
- **Thunar**: the in-app bulk renamer's live preview.
- **Midnight Commander**: F5/F6 between panes.
- **fzf / xdg-desktop-portal-termfilechooser**: a picker you can call from a script.
