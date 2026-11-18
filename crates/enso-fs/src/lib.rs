//! Files without a window: what the enso tools that deal in files share.
//!
//! - [`git`] — the repository a folder is in, its branch, identities,
//!   through the `git` program.
//! - [`list`] — a directory's entries in two passes: names and kinds at
//!   once, sizes and dates from a pool of threads after.
//! - [`sort`] — natural ordering of a listing, by name, size, date, or
//!   extension.
//! - [`format`] — sizes and dates the way a person reads them.
//! - [`ops`] — copying, moving, renaming, deleting, with progress,
//!   conflicts, and undo.
//! - [`places`] — home, the XDG user folders, and free space.
//! - [`watch`] — following a folder as it changes, stopped by dropping.
//! - [`thumbs`] — thumbnails through the freedesktop cache, PNG and JPEG
//!   made here.
//! - [`trash`] — the freedesktop trash: trashing, listing, restoring,
//!   emptying, on the home filesystem and others.
//! - [`uri`] — `file://` URIs, URI lists, and the portal's byte paths.
//! - [`apps`] — installed applications, from freedesktop desktop entries,
//!   and launching them.
//! - [`mime`] — file types by name from the shared MIME database, loaded
//!   once for lookups by the thousand.
//! - [`open`] — opening a file with the application the desktop associates
//!   with it, from the shared MIME database and `mimeapps.list`.
//! - [`search`] — an index of every file under some roots, walked in
//!   parallel, and a matcher fast enough to run over it per keystroke.
//! - [`ignores`] — what a file search leaves out: `.gitignore` rules and
//!   build output.
//! - [`fuzzy`] — the fuzzy matcher the launcher and the file manager share.
//!
//! Nothing here draws, and nothing shells out: the freedesktop databases
//! and specifications are read and written directly. The one exception is
//! [`git`], which runs the `git` program itself (never through a shell).

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod app_index;
pub mod apps;
pub mod bookmarks;
pub mod format;
pub mod fuzzy;
pub mod git;
pub mod ignores;
pub mod list;
pub mod mime;
pub mod open;
pub mod ops;
pub mod places;
pub mod recent;
pub mod search;
pub mod sort;
pub mod thumbs;
pub mod trash;
pub mod uri;
pub mod watch;

/// Say something went wrong in the background, where there is no caller to
/// return it to, prefixed with the running program's name.
pub(crate) fn warn(message: &str) {
    let program = std::env::args_os()
        .next()
        .map(std::path::PathBuf::from)
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "enso".to_owned());
    eprintln!("{program}: {message}");
}
