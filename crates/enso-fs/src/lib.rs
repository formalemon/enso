//! Files without a window: what the enso tools that deal in files share.
//!
//! - [`list`] — a directory's entries in two passes: names and kinds at
//!   once, sizes and dates from a pool of threads after.
//! - [`sort`] — natural ordering of a listing, by name, size, date, or
//!   extension.
//! - [`format`] — sizes and dates the way a person reads them.
//! - [`places`] — home, the XDG user folders, and free space.
//! - [`uri`] — `file://` URIs, URI lists, and the portal's byte paths.
//! - [`apps`] — installed applications, from freedesktop desktop entries,
//!   and launching them.
//! - [`open`] — opening a file with the application the desktop associates
//!   with it, from the shared MIME database and `mimeapps.list`.
//! - [`search`] — an index of every file under some roots, walked in
//!   parallel, and a matcher fast enough to run over it per keystroke.
//! - [`ignores`] — what a file search leaves out: `.gitignore` rules and
//!   build output.
//! - [`fuzzy`] — the fuzzy matcher the launcher and the file manager share.
//!
//! Nothing here draws, and nothing shells out: the freedesktop databases
//! and specifications are read and written directly.

#![cfg_attr(test, allow(clippy::unwrap_used))]

mod app_index;
pub mod apps;
pub mod format;
pub mod fuzzy;
pub mod ignores;
pub mod list;
pub mod open;
pub mod places;
pub mod search;
pub mod sort;
pub mod uri;

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
