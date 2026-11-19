//! Recently used files, in the freedesktop file every program shares
//! (`~/.local/share/recently-used.xbel`): what GTK programs, the GTK file
//! chooser, Firefox, and now enso-files have opened or picked.
//!
//! The file is XML, but of a fixed and simple shape, so it's read by
//! looking for its `<bookmark>` elements rather than with an XML parser,
//! and written by changing only what's needed: an entry already there gets
//! new times; a new one is added in the shape GTK writes.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

use enso_core::disk;
use jiff::Timestamp;

use crate::uri;

/// A file used lately.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Recent {
    pub path: PathBuf,
    /// When it was last used.
    pub when: Timestamp,
}

/// Where the file is.
pub fn file() -> PathBuf {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| crate::places::home().join(".local/share"));
    data.join("recently-used.xbel")
}

/// The local files in it, newest first, at most `limit`.
pub fn read(path: &Path, limit: usize) -> Vec<Recent> {
    let text = fs::read_to_string(path).unwrap_or_default();
    let mut recent = parse(&text);
    recent.sort_by_key(|recent| std::cmp::Reverse(recent.when));
    recent.truncate(limit);
    recent
}

fn parse(text: &str) -> Vec<Recent> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("<bookmark ") {
        rest = &rest[at..];
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end..];
        let Some(path) = attribute(tag, "href").and_then(|href| uri::to_path(&unescape(href)))
        else {
            continue;
        };
        let time = |name| attribute(tag, name).and_then(|value| value.parse::<Timestamp>().ok());
        let Some(when) = [time("visited"), time("modified"), time("added")]
            .into_iter()
            .flatten()
            .max()
        else {
            continue;
        };
        found.push(Recent { path, when });
    }
    found
}

/// The value of `name="…"` in a start tag.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let key = format!(" {name}=\"");
    let start = tag.find(&key)? + key.len();
    let len = tag[start..].find('"')?;
    Some(&tag[start..start + len])
}

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// Note that `path` (of type `mime`) was just used by this program.
pub fn add(file: &Path, path: &Path, mime: &str) -> io::Result<()> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err),
    };
    let now = Timestamp::now().to_string();
    let text = added(&text, &uri::from_path(path), mime, &now);
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    // Private, as GTK's is.
    disk::write_atomic(file, text.as_bytes(), disk::PRIVATE)
}

/// `text` with `uri` marked as used at `now`.
fn added(text: &str, uri: &str, mime: &str, now: &str) -> String {
    let start = format!("<bookmark href=\"{uri}\"");
    if let Some(at) = text.find(&start) {
        // Already there: only its times change.
        let end = at + text[at..].find('>').unwrap_or(0);
        let mut tag = text[at..end].to_owned();
        for name in ["modified", "visited"] {
            let key = format!(" {name}=\"");
            if let Some(value_at) = tag.find(&key).map(|i| i + key.len())
                && let Some(len) = tag[value_at..].find('"')
            {
                tag.replace_range(value_at..value_at + len, now);
            }
        }
        return format!("{}{tag}{}", &text[..at], &text[end..]);
    }
    let mime = mime.replace('&', "&amp;").replace('"', "&quot;");
    let entry = format!(
        "  <bookmark href=\"{uri}\" added=\"{now}\" modified=\"{now}\" visited=\"{now}\">
    <info>
      <metadata owner=\"http://freedesktop.org\">
        <mime:mime-type type=\"{mime}\"/>
        <bookmark:applications>
          <bookmark:application name=\"enso-files\" exec=\"&apos;enso-files %u&apos;\" modified=\"{now}\" count=\"1\"/>
        </bookmark:applications>
      </metadata>
    </info>
  </bookmark>
"
    );
    match text.rfind("</xbel>") {
        Some(end) => format!("{}{entry}{}", &text[..end], &text[end..]),
        None => format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<xbel version=\"1.0\"
      xmlns:bookmark=\"http://www.freedesktop.org/standards/desktop-bookmarks\"
      xmlns:mime=\"http://www.freedesktop.org/standards/shared-mime-info\"
>
{entry}</xbel>
"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XBEL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<xbel version="1.0"
      xmlns:bookmark="http://www.freedesktop.org/standards/desktop-bookmarks"
      xmlns:mime="http://www.freedesktop.org/standards/shared-mime-info"
>
  <bookmark href="file:///home/kai/a%20b.png" added="2026-11-15T12:59:20Z" modified="2026-11-16T16:39:22Z" visited="2026-11-15T12:59:20Z">
    <info/>
  </bookmark>
  <bookmark href="https://example.com/" added="2026-11-17T00:00:00Z" modified="2026-11-17T00:00:00Z" visited="2026-11-17T00:00:00Z">
  </bookmark>
  <bookmark href="file:///tmp/x.tar.gz" added="2026-11-15T18:01:56Z" modified="2026-11-15T18:01:56Z" visited="2026-11-15T18:01:56Z">
  </bookmark>
</xbel>
"#;

    #[test]
    fn local_files_newest_first() {
        let mut recent = parse(XBEL);
        recent.sort_by_key(|recent| std::cmp::Reverse(recent.when));
        let paths: Vec<_> = recent.iter().map(|r| r.path.to_str().unwrap()).collect();
        assert_eq!(paths, ["/home/kai/a b.png", "/tmp/x.tar.gz"]);
    }

    #[test]
    fn using_a_file_again_only_moves_its_time() {
        let now = "2026-11-18T10:00:00Z";
        let again = added(XBEL, "file:///tmp/x.tar.gz", "application/gzip", now);
        assert_eq!(again.len(), XBEL.len(), "nothing added");
        assert!(again.contains(&format!(
            r#"href="file:///tmp/x.tar.gz" added="2026-11-15T18:01:56Z" modified="{now}" visited="{now}""#
        )));
        let fresh = added(XBEL, "file:///home/kai/new.txt", "text/plain", now);
        assert!(fresh.ends_with("  </bookmark>\n</xbel>\n"));
        assert_eq!(parse(&fresh).len(), 3);
        assert_eq!(parse(&fresh)[2].path, Path::new("/home/kai/new.txt"));
        // From nothing: a whole file.
        let first = added("", "file:///a", "text/plain", now);
        assert!(first.starts_with("<?xml") && first.ends_with("</xbel>\n"));
        assert_eq!(parse(&first).len(), 1);
    }
}
