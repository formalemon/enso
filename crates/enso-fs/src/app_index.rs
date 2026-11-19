//! The application index: the launcher's list of applications, saved from
//! one run to the next so that opening it reads one small file instead of
//! every desktop entry on the system.
//!
//! The format is plain text and private to the launcher: a header line, then
//! one application per line with tab-separated fields. It is only ever a
//! cache — anything unexpected makes [`read`] return `None` and the launcher
//! falls back to scanning.

use std::{fs, io, path::Path};

use enso_core::disk;

use crate::apps::{App, Shortcut};

/// First line of the file. Bump the number when the format changes; older
/// files are then ignored and rewritten.
const HEADER: &str = "enso-launch index 3";
/// Separates fields within a line.
const FIELD: char = '\t';
/// Ends each item of a list field.
const ITEM: char = '\x1f';
/// Separates an action's name from its command's arguments, and those
/// arguments from each other, within one item.
const PART: char = '\x1e';

/// The saved index, if there is a usable one.
pub fn read(path: &Path) -> Option<Vec<App>> {
    decode(&fs::read_to_string(path).ok()?)
}

/// Save the index, atomically: a launcher starting at the same moment sees
/// either the old file or the new one, never half of it.
pub fn write(path: &Path, apps: &[App]) -> io::Result<()> {
    disk::write_atomic(path, encode(apps).as_bytes(), disk::SHARED)
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
        out.push(FIELD);
        out.push(if app.fresh { '1' } else { '0' });
        out.push(FIELD);
        escape(app.wm_class.as_deref().unwrap_or_default(), &mut out);
        out.push(FIELD);
        for action in &app.actions {
            escape(&action.name, &mut out);
            for arg in &action.command {
                out.push(PART);
                escape(arg, &mut out);
            }
            out.push(ITEM);
        }
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
                terminal: flag(fields.next()?)?,
                fresh: flag(fields.next()?)?,
                wm_class: Some(unescape(fields.next()?)).filter(|class| !class.is_empty()),
                actions: fields
                    .next()?
                    .split_terminator(ITEM)
                    .map(|item| {
                        let mut parts = item.split(PART).map(unescape);
                        Shortcut {
                            name: parts.next().unwrap_or_default(),
                            command: parts.collect(),
                        }
                    })
                    .collect(),
            };
            fields.next().is_none().then_some(app)
        })
        .collect()
}

fn flag(field: &str) -> Option<bool> {
    match field {
        "1" => Some(true),
        "0" => Some(false),
        _ => None,
    }
}

/// Write `text` with the characters the format gives meaning to escaped.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            FIELD => out.push_str("\\t"),
            ITEM => out.push_str("\\i"),
            PART => out.push_str("\\p"),
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
            Some('p') => out.push(PART),
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
            fresh: false,
            wm_class: None,
            actions: Vec::new(),
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
                fresh: true,
                wm_class: Some("Tricky\x1eclass".into()),
                actions: vec![
                    Shortcut {
                        name: "New\x1fwindow".into(),
                        command: vec!["tricky".into(), String::new(), "--new\x1e".into()],
                    },
                    Shortcut::default(),
                ],
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
        assert_eq!(decode(&good.replace("index 3", "index 2")), None);
        assert_eq!(decode(""), None);
        assert_eq!(decode("[Desktop Entry]\nName=Files\n"), None);
        // A record cut short, or with a field too many.
        assert_eq!(decode(&good[..good.len() - 4]), None);
        assert_eq!(decode(&good.replace("\t0\t0\t\t\n", "\t0\t0\t\t\textra\n")), None);
        assert_eq!(decode(&good.replace("\t0\t0\t\t\n", "\tyes\t0\t\t\n")), None);
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
