//! Paths as `file://` URIs and back, and paths as the desktop portal sends
//! them.
//!
//! A URI may only hold a few characters as they are; every other byte is
//! written `%XX`. Paths on Linux are bytes, not text, so the conversion is
//! byte for byte both ways and a name that isn't UTF-8 survives the trip.

use std::{
    ffi::OsStr,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
};

/// `path` (which should be absolute) as a `file://` URI.
pub fn from_path(path: &Path) -> String {
    let bytes = path.as_os_str().as_bytes();
    let mut uri = String::with_capacity(7 + bytes.len() * 3 / 2);
    uri.push_str("file://");
    for &byte in bytes {
        if is_unreserved(byte) || byte == b'/' {
            uri.push(char::from(byte));
        } else {
            uri.push('%');
            uri.push(char::from(HEX[usize::from(byte >> 4)]));
            uri.push(char::from(HEX[usize::from(byte & 15)]));
        }
    }
    uri
}

const HEX: &[u8; 16] = b"0123456789ABCDEF";

/// RFC 3986's unreserved characters, which need no escaping anywhere.
fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

/// The path a `file://` URI names, if it names a local one: no host, or
/// `localhost`. Malformed escapes make it `None` rather than a guess.
pub fn to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    // A query or fragment is not part of the path.
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);
    let mut bytes = Vec::with_capacity(rest.len());
    let mut input = rest.bytes();
    while let Some(byte) = input.next() {
        if byte == b'%' {
            let high = enso_core::hex::digit(input.next()?)?;
            let low = enso_core::hex::digit(input.next()?)?;
            bytes.push(high << 4 | low);
        } else {
            bytes.push(byte);
        }
    }
    Some(PathBuf::from(OsStr::from_bytes(&bytes)))
}

/// A path as the portal sends it (`ay`): the bytes, usually with a
/// terminating NUL.
pub fn from_portal_bytes(bytes: &[u8]) -> PathBuf {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    PathBuf::from(OsStr::from_bytes(bytes))
}

/// A path as the portal wants it back: the bytes and a terminating NUL.
pub fn to_portal_bytes(path: &Path) -> Vec<u8> {
    let mut bytes = path.as_os_str().as_bytes().to_vec();
    bytes.push(0);
    bytes
}

/// Paths from a `text/uri-list`, as dropped or pasted: one URI per line,
/// `#` lines are comments, anything not a local file is skipped.
pub fn parse_list(list: &str) -> Vec<PathBuf> {
    list.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(to_path)
        .collect()
}

/// `paths` as a `text/uri-list`: CRLF-terminated lines, as the spec asks.
pub fn format_list<'a>(paths: impl IntoIterator<Item = &'a Path>) -> String {
    paths
        .into_iter()
        .map(|path| from_path(path) + "\r\n")
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_paths_stay_readable() {
        assert_eq!(
            from_path(Path::new("/home/me/notes-1_final~.txt")),
            "file:///home/me/notes-1_final~.txt"
        );
    }

    #[test]
    fn everything_else_is_escaped_and_comes_back() {
        let path = Path::new("/home/me/a b/é #1?.png");
        let uri = from_path(path);
        assert_eq!(uri, "file:///home/me/a%20b/%C3%A9%20%231%3F.png");
        assert_eq!(to_path(&uri).as_deref(), Some(path));

        let raw = PathBuf::from(OsStr::from_bytes(b"/tmp/caf\xe9"));
        assert_eq!(from_path(&raw), "file:///tmp/caf%E9");
        assert_eq!(to_path(&from_path(&raw)), Some(raw));
    }

    #[test]
    fn only_local_files_are_paths() {
        assert_eq!(
            to_path("file://localhost/etc/hosts").as_deref(),
            Some(Path::new("/etc/hosts"))
        );
        assert_eq!(
            to_path("file:///etc/hosts#top").as_deref(),
            Some(Path::new("/etc/hosts"))
        );
        assert_eq!(to_path("file://server/share/x"), None);
        assert_eq!(to_path("https://example.com/x"), None);
        assert_eq!(to_path("file:///bad%2"), None);
        assert_eq!(to_path("file:///bad%zz"), None);
    }

    #[test]
    fn portal_paths_end_in_nul() {
        assert_eq!(from_portal_bytes(b"/home/me\0"), Path::new("/home/me"));
        assert_eq!(from_portal_bytes(b"/home/me"), Path::new("/home/me"));
        assert_eq!(to_portal_bytes(Path::new("/x")), b"/x\0");
    }

    #[test]
    fn uri_lists_both_ways() {
        let paths = [Path::new("/a b"), Path::new("/c")];
        let list = format_list(paths);
        assert_eq!(list, "file:///a%20b\r\nfile:///c\r\n");
        assert_eq!(
            parse_list(&format!("# copied\n{list}https://x.org/\n\n")),
            paths.map(Path::to_owned)
        );
    }
}
