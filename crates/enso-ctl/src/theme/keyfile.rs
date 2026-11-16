//! The little of the desktop key-file format (`[Group]`, `key=value`) that
//! theming needs: reading one key, and setting one while leaving every other
//! line of a file somebody else owns exactly as it was.
//!
//! GTK's `settings.ini`, Flatpak's overrides, and qt5ct's `.conf` are all
//! this shape.

/// The value of `key` in `[group]`, if it is set.
pub fn get<'a>(text: &'a str, group: &str, key: &str) -> Option<&'a str> {
    let mut inside = false;
    let mut found = None;
    for line in text.lines() {
        if let Some(name) = header(line) {
            inside = name == group;
        } else if inside
            && let Some((name, value)) = pair(line)
            && name == key
        {
            // As the readers of these files do: the last one wins.
            found = Some(value);
        }
    }
    found
}

/// `text` with `key` of `[group]` set to `value`. The group is added at the
/// end if the file has none, the key at the end of its group.
pub fn set(text: &str, group: &str, key: &str, value: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let entry = format!("{key}={value}");

    let mut inside = false;
    // The line that sets the key, and the last line of the group with
    // anything on it.
    let (mut at, mut group_end) = (None, None);
    for (index, line) in lines.iter().enumerate() {
        if let Some(name) = header(line) {
            inside = name == group;
            if inside {
                group_end = Some(index);
            }
        } else if inside && !line.trim().is_empty() {
            group_end = Some(index);
            if pair(line).is_some_and(|(name, _)| name == key) {
                at = Some(index);
            }
        }
    }

    let mut out: Vec<String> = lines.iter().map(|line| (*line).to_owned()).collect();
    match (at, group_end) {
        (Some(index), _) => out[index] = entry,
        (None, Some(index)) => out.insert(index + 1, entry),
        (None, None) => {
            if out.last().is_some_and(|line| !line.trim().is_empty()) {
                out.push(String::new());
            }
            out.push(format!("[{group}]"));
            out.push(entry);
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

/// The name in a `[Group]` line.
fn header(line: &str) -> Option<&str> {
    line.trim().strip_prefix('[')?.strip_suffix(']')
}

/// The two sides of a `key=value` line; comments and blanks are neither.
fn pair(line: &str) -> Option<(&str, &str)> {
    let line = line.trim();
    if line.starts_with(['#', ';']) {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((key.trim_end(), value.trim_start()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const INI: &str = "\
# Mine.
[Settings]
gtk-font-name = Inter 11
gtk-application-prefer-dark-theme=false

[Other]
gtk-application-prefer-dark-theme=maybe
";

    #[test]
    fn reads_a_key_of_a_group() {
        let key = "gtk-application-prefer-dark-theme";
        assert_eq!(get(INI, "Settings", key), Some("false"));
        assert_eq!(get(INI, "Other", key), Some("maybe"));
        assert_eq!(get(INI, "Settings", "gtk-font-name"), Some("Inter 11"));
        assert_eq!(get(INI, "Settings", "gtk-theme-name"), None);
        assert_eq!(get(INI, "Missing", key), None);
        assert_eq!(get("# a=b\n[G]\n; a=c\n", "G", "a"), None);
    }

    #[test]
    fn replaces_a_key_in_place() {
        let out = set(INI, "Settings", "gtk-application-prefer-dark-theme", "true");
        assert_eq!(
            out,
            INI.replacen(
                "gtk-application-prefer-dark-theme=false",
                "gtk-application-prefer-dark-theme=true",
                1
            )
        );
        // Setting what is already there changes nothing.
        assert_eq!(
            set(
                INI,
                "Settings",
                "gtk-application-prefer-dark-theme",
                "false"
            ),
            INI
        );
    }

    #[test]
    fn adds_a_key_at_the_end_of_its_group() {
        let out = set(INI, "Settings", "gtk-theme-name", "Adwaita");
        assert_eq!(
            out,
            INI.replacen("theme=false\n", "theme=false\ngtk-theme-name=Adwaita\n", 1)
        );
        assert_eq!(
            set("[Settings]\n", "Settings", "a", "1"),
            "[Settings]\na=1\n"
        );
    }

    #[test]
    fn adds_a_group_to_a_file_without_it() {
        assert_eq!(set("", "Settings", "a", "1"), "[Settings]\na=1\n");
        assert_eq!(
            set("[Other]\nb=2", "Settings", "a", "1"),
            "[Other]\nb=2\n\n[Settings]\na=1\n"
        );
        assert_eq!(
            set("[Other]\nb=2\n\n", "Settings", "a", "1"),
            "[Other]\nb=2\n\n[Settings]\na=1\n"
        );
    }
}
