//! The application index: the launcher's list of applications, saved from
//! one run to the next so that opening it reads one small file instead of
//! every desktop entry on the system.
//!
//! The format is plain text and private to the launcher: a header line, then
//! one application per line with tab-separated fields. It is only ever a
//! cache — anything unexpected makes [`read`] return `None` and the launcher
//! falls back to scanning.

use std::{fs, io, path::Path};

use crate::apps::App;

/// First line of the file. Bump the number when the format changes; older
/// files are then ignored and rewritten.
const HEADER: &str = "enso-launch index 1";
/// Separates fields within a line.
const FIELD: char = '\t';
/// Ends each item of a list field.
const ITEM: char = '\x1f';

/// The saved index, if there is a usable one.
pub fn read(path: &Path) -> Option<Vec<App>> {
    decode(&fs::read_to_string(path).ok()?)
}

/// Save the index, atomically: a launcher starting at the same moment sees
/// either the old file or the new one, never half of it.
pub fn write(path: &Path, apps: &[App]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("index path has no directory"))?;
    fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".apps.{}.tmp", std::process::id()));
    fs::write(&tmp, encode(apps))?;
    fs::rename(&tmp, path).inspect_err(|_| {
        // Best effort: a leftover temporary file is harmless.
        let _ = fs::remove_file(&tmp);
    })
}

fn encode(apps: &[App]) -> String {
    let mut out = String::with_capacity(64 + apps.len() * 128);
    out.push_str(HEADER);
    out.push('\n');
    for app in apps {
        escape(&app.id, &mut out);
        out.push(FIELD);
        escape(&app.name, &mut out);
        out.push(FIELD);
        escape(app.description.as_deref().unwrap_or_default(), &mut out);
        out.push(FIELD);
        for list in [&app.keywords, &app.command] {
            for item in list {
                escape(item, &mut out);
                out.push(ITEM);
            }
            out.push(FIELD);
        }
        out.push(if app.terminal { '1' } else { '0' });
        out.push('\n');
    }
    out
}

fn decode(text: &str) -> Option<Vec<App>> {
    // Not `lines()`: that would also swallow a carriage return inside a field.
    let mut lines = text.split('\n');
    if lines.next()? != HEADER {
        return None;
    }
    lines
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut fields = line.split(FIELD);
            let list = |field: &str| field.split_terminator(ITEM).map(unescape).collect();
            let app = App {
                id: unescape(fields.next()?),
                name: unescape(fields.next()?),
                description: Some(unescape(fields.next()?)).filter(|text| !text.is_empty()),
                keywords: list(fields.next()?),
                command: list(fields.next()?),
                terminal: match fields.next()? {
                    "1" => true,
                    "0" => false,
                    _ => return None,
                },
            };
            fields.next().is_none().then_some(app)
        })
        .collect()
}

/// Write `text` with the characters the format gives meaning to escaped.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            FIELD => out.push_str("\\t"),
            ITEM => out.push_str("\\i"),
            c => out.push(c),
        }
    }
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push(FIELD),
            Some('i') => out.push(ITEM),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> App {
        App {
            id: name.to_lowercase(),
            name: name.to_owned(),
            description: None,
            keywords: Vec::new(),
            command: vec![name.to_lowercase()],
            terminal: false,
        }
    }

    #[test]
    fn round_trips_every_field() {
        let apps = vec![
            app("Files"),
            App {
                id: "org.example.Tricky".into(),
                name: "Tab\there, back\\slash, new\nline, unit\x1fsep, cr\r".into(),
                description: Some("Résumé — 履歴書".into()),
                keywords: vec!["a;b".into(), "c".into()],
                // An empty argument and an empty list must stay distinct.
                command: vec![
                    "sh".into(),
                    "-c".into(),
                    "echo \"hi\"\t".into(),
                    String::new(),
                ],
                terminal: true,
            },
            App {
                command: Vec::new(),
                ..app("Empty")
            },
        ];
        assert_eq!(decode(&encode(&apps)), Some(apps));
        assert_eq!(decode(&encode(&[])), Some(Vec::new()));
    }

    #[test]
    fn rejects_anything_unexpected() {
        let good = encode(&[app("Files"), app("Notes")]);
        assert!(decode(&good).is_some());
        // Another version, an empty or foreign file.
        assert_eq!(decode(&good.replace("index 1", "index 2")), None);
        assert_eq!(decode(""), None);
        assert_eq!(decode("[Desktop Entry]\nName=Files\n"), None);
        // A record cut short, or with a field too many.
        assert_eq!(decode(&good[..good.len() - 4]), None);
        assert_eq!(decode(&good.replace("\t0\n", "\t0\textra\n")), None);
        assert_eq!(decode(&good.replace("\t0\n", "\tyes\n")), None);
    }

    #[test]
    fn writes_and_reads_a_file() {
        let dir = std::env::temp_dir().join(format!("enso-fs-index-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        // The directory is created on demand.
        let path = dir.join("nested/apps");
        assert_eq!(read(&path), None);

        let apps = vec![app("Files"), app("Notes")];
        write(&path, &apps).unwrap();
        assert_eq!(read(&path), Some(apps));
        // Nothing but the index is left behind.
        assert_eq!(fs::read_dir(dir.join("nested")).unwrap().count(), 1);
        fs::remove_dir_all(dir).unwrap();
    }
}
