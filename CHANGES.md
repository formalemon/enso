# Changes

What changed that you would notice, newest first, a line or two each. The
README describes the suite as it stood before 2026-11-18 and is behind in
places; this file is current. Add to it with each change worth mentioning.

## 2026-11-18

### Across the suite

- **Snapshots that catch regressions.** Twelve key screens are held to
  reference pictures in `crates/*/snapshots/`; a test fails when more than
  a sliver changes, and writes the changes in magenta beside the snapshot.
  `ENSO_BLESS=1 cargo test` makes the new pictures the references.
- **The terminal is a setting:** `[appearance] terminal`, else `$TERMINAL`,
  else alacritty, run as `<terminal> -e <command>`. `[launch] terminal` is
  gone in its favour.
- **Pictures in every format.** Notifications, the wallpaper, and the lock
  screen's backdrop now take PNG, JPEG, WebP, and GIF, through the decoder
  enso-files' thumbnails use (`enso_fs::picture`).
- **One way to lay out a dialog.** Every dialog is built with enso-ui's
  `Form`: one title size, one field shape (a pill), cards as tall as what
  they say. The password field (dots, a shake when refused, the Caps Lock
  note) is one widget, used by the lock screen, enso-auth, enso-keyring, and
  the control center's Wi-Fi passphrase.
- **Files are written whole.** Settings, history, indexes, thumbnails, the
  keyring, and theme files are written beside the old file, synced to disk,
  then swapped in (`enso_core::disk`); writing through a symbolic link keeps
  the link.
- **Secrets are wiped the same way everywhere** (`zeroize`): typed
  passwords and passphrases, PAM's questions, keyring secrets, the GitHub
  token.
- **A rebuild no longer stops locking.** The lock watcher, enso-auth's
  prompts, and enso-files' picker start themselves through
  `/proc/self/exe`, so idle and sleep locking and polkit prompts keep
  working after `cargo build` replaces the binary under them.

### enso-launch

- **Looks inside files:** a query starting with a quote (`"green tea`, or
  `"/te+a/` for a pattern) searches the contents of the indexed files, shows
  each match's line, and gives way the moment the query changes.
  `enso-launch --find '"words'` does it from a terminal.

### enso-files

- One overlay at a time — a dialog, a form, the palette, a menu — by
  construction; keys and clicks go to it alone.
- The `?` keys card shows the essentials and points to the command palette
  (`Ctrl+P`), which has everything with its keys.
- Dialog titles and fields match the rest of the suite; renaming many at
  once shows its token hint at the top and its summary above the buttons.
- **GitHub:** sign in with your own OAuth app (device flow; the token goes
  in the keyring), clone from a list of your repositories, publish a
  repository, and let git ask enso-files for GitHub's way in.
- **Git remotes:** a branch menu with remote branches; fetch, pull, push,
  clone.
- **Drag and drop**, both ways, with other programs.
- **Archives:** look inside zip and tar (gz, xz, zst, bz2) as folders, take
  out, and make new ones.
- Each folder keeps its own view (list, grid, columns; sort).
- **Properties**, with permissions to tick and folder sizes counted down.
- **Rename many at once** (a setting): find and replace, patterns, case,
  counters, or edit the names in `$EDITOR`.
- **The command palette** (`Ctrl+P`), and folders ranked by how often and
  how lately you go there.
- **Search** by name below a folder or inside files (`Ctrl+Shift+F`),
  recent files, bookmarks, and query words (`type:image size:>10M
  modified:<7d`).
- **Tabs** and a **split view**.
- Columns view, a preview pane and a quick look, thumbnails (the
  freedesktop cache; PNG, JPEG, WebP, GIF made here), a grid, drag-to-choose.
- Open with…, drives and the trash in the sidebar (udisks2), "Show in
  folder" for other programs, settings in the window, opt-in version
  control (branch, identity, init, new branch).
- Copy, move, rename, trash, delete, with undo; folders update as they
  change.
- **The file picker for every app**, behind the desktop portal:
  `enso-files portal enable` sets it up; each app's folder is remembered.

### enso-keyring (new)

- The desktop's Secret Service keyring: gh, git (libsecret), browsers and
  anything built on libsecret keep passwords and tokens in it. It is sealed
  to this machine and account by systemd credentials (with the TPM) and
  opened by your login password, which is checked, never kept.
  `enso-keyring enable` makes it the session's keyring.

### enso-auth (new)

- Asks for your password when the system needs it (the polkit agent), and
  for Bluetooth pairing codes (BlueZ's agent), in one prompt card.
