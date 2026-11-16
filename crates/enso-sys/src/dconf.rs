//! Desktop settings through dconf: the store behind GSettings.
//!
//! The dark-or-light preference every toolkit asks about — GTK 4, Qt 6,
//! Firefox, Electron, through the settings portal — is one key in the
//! user's dconf database. Reading that database is a matter of mapping a
//! file; writing it is the job of `dconf-service`, which takes a change over
//! the session bus, writes it, and tells every running program.
//!
//! A change travels as one byte array: a GVariant of type `a{smv}` — path to
//! new value — in GVariant's own serialisation, which is not the D-Bus one.
//! Only strings are written here, so only that much of the format is.

use crate::bus::{Result, Service};

const WRITER: &str = "ca.desrt.dconf.Writer";
/// The writer of the `user` database, the one the default profile names.
const USER: &str = "/ca/desrt/dconf/Writer/user";

/// Set string keys, all in one change. Each is a full path and its value:
/// `("/org/gnome/desktop/interface/color-scheme", "prefer-dark")`.
///
/// `dconf-service` is started on demand; where it is not installed the
/// error [`is_unavailable`](crate::Error::is_unavailable).
pub fn set_strings(entries: &[(&str, &str)]) -> Result<()> {
    // The reply is a tag naming the change, of use only to a program that
    // also watches for its own writes.
    let _tag: String =
        Service::session("ca.desrt.dconf")?.call(USER, WRITER, "Change", &(changeset(entries),))?;
    Ok(())
}

/// Entries of a changeset start on a multiple of this: a variant's alignment.
const ALIGN: usize = 8;

/// `entries` as a serialised `a{smv}`.
fn changeset(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut array = Vec::new();
    let mut ends = Vec::with_capacity(entries.len());
    for (path, value) in entries {
        pad(&mut array);
        array.extend(entry(path, value));
        ends.push(array.len());
    }
    // Entries differ in size, so the array ends with where each one ends.
    let width = offset_width(array.len(), ends.len());
    for end in ends {
        push_offset(&mut array, end, width);
    }
    array
}

/// One `{smv}`: the key, then — aligned — the value, then where the key ends.
fn entry(path: &str, value: &str) -> Vec<u8> {
    let mut entry = Vec::with_capacity(path.len() + value.len() + 16);
    entry.extend_from_slice(path.as_bytes());
    entry.push(0);
    let key_end = entry.len();
    pad(&mut entry);
    // The string, then what makes it a variant: a separator and its type.
    entry.extend_from_slice(value.as_bytes());
    entry.extend_from_slice(&[0, 0, b's']);
    // A maybe that holds something of no fixed size ends with one zero.
    entry.push(0);
    let width = offset_width(entry.len(), 1);
    push_offset(&mut entry, key_end, width);
    entry
}

fn pad(bytes: &mut Vec<u8>) {
    bytes.resize(bytes.len().next_multiple_of(ALIGN), 0);
}

/// Bytes per offset: the fewest that can address the container, offsets
/// included.
fn offset_width(body: usize, offsets: usize) -> usize {
    if body + offsets <= usize::from(u8::MAX) {
        1
    } else if body + 2 * offsets <= usize::from(u16::MAX) {
        2
    } else {
        4
    }
}

/// GVariant data is in the byte order of the machine that wrote it.
fn push_offset(bytes: &mut Vec<u8>, offset: usize, width: usize) {
    match width {
        1 => bytes.push(offset as u8),
        2 => bytes.extend_from_slice(&(offset as u16).to_ne_bytes()),
        _ => bytes.extend_from_slice(&(offset as u32).to_ne_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// What GLib itself serialises the same changesets to
    /// (`g_variant_builder_add (&b, "{smv}", path, g_variant_new_string (…))`).
    #[test]
    #[cfg(target_endian = "little")]
    fn changesets_match_glib() {
        assert_eq!(
            hex(&changeset(&[(
                "/org/gnome/desktop/interface/color-scheme",
                "prefer-dark"
            )])),
            "2f6f72672f676e6f6d652f6465736b746f702f696e746572666163652f636f6c6f722d736368656d65\
             000000000000007072656665722d6461726b000073002a40"
        );
        // Two entries: the second starts on a multiple of eight.
        assert_eq!(
            hex(&changeset(&[
                ("/a/b", "x"),
                ("/org/gnome/desktop/interface/gtk-theme", "Adwaita-dark"),
            ])),
            "2f612f620000000078000073000500002f6f72672f676e6f6d652f6465736b746f702f696e74657266\
             6163652f67746b2d7468656d650000416477616974612d6461726b00007300270e49"
        );
        assert!(changeset(&[]).is_empty());
    }

    /// Past 255 bytes the array's offsets take two bytes each; the entries,
    /// still short, keep one.
    #[test]
    #[cfg(target_endian = "little")]
    fn long_changesets_use_wider_offsets() {
        let keys: Vec<String> = (0..6)
            .map(|i| format!("/org/gnome/desktop/interface/key-number-{i}"))
            .collect();
        let entries: Vec<(&str, &str)> = keys
            .iter()
            .map(|key| (key.as_str(), "a-reasonably-long-value"))
            .collect();
        let bytes = changeset(&entries);
        assert_eq!(bytes.len(), 6 * 80 - 4 + 6 * 2);
        assert_eq!(hex(&bytes[..76]), {
            let mut first = hex(b"/org/gnome/desktop/interface/key-number-0");
            first.push_str("00000000000000");
            first.push_str(&hex(b"a-reasonably-long-value"));
            first.push_str("000073002a");
            first
        });
        assert_eq!(hex(&bytes[bytes.len() - 12..]), "4c009c00ec003c018c01dc01");
    }
}
