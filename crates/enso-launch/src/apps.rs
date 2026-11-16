//! Installed applications, read from freedesktop `.desktop` entries.

use std::{
    collections::HashSet,
    env, fs, io,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// A launchable application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct App {
    /// Desktop file id, e.g. `org.mozilla.firefox`.
    pub id: String,
    pub name: String,
    /// Generic name or comment, shown as a quiet second label.
    pub description: Option<String>,
    /// Extra search terms.
    pub keywords: Vec<String>,
    /// Command line with field codes removed.
    pub command: Vec<String>,
    /// Must run inside a terminal emulator.
    pub terminal: bool,
}

/// Every visible application, sorted by name. Earlier directories win, so a
/// user's entry in `~/.local/share/applications` overrides the system one.
pub fn load() -> Vec<App> {
    load_from(&application_dirs(), &current_desktops())
}

/// Hand `ready` the applications as early as possible, then make sure they
/// are right.
///
/// The index saved at `index` by the previous run comes first — one small
/// file rather than every desktop entry. The real scan follows, and only if
/// something was installed, removed, or edited since is `ready` called a
/// second time and the index rewritten. So the index is never trusted for
/// longer than one scan takes, and needs no invalidation rules.
pub fn load_indexed(index: &Path, ready: impl FnMut(Vec<App>)) {
    load_indexed_from(index, &application_dirs(), &current_desktops(), ready);
}

fn load_indexed_from(
    index: &Path,
    dirs: &[PathBuf],
    desktops: &[String],
    mut ready: impl FnMut(Vec<App>),
) {
    let saved = crate::index::read(index);
    if let Some(apps) = &saved {
        ready(apps.clone());
    }
    let apps = load_from(dirs, desktops);
    if saved.as_ref() != Some(&apps) {
        // Without an index the next launch scans first; nothing else is lost.
        if let Err(err) = crate::index::write(index, &apps) {
            eprintln!("enso-launch: cannot save the application index: {err}");
        }
        ready(apps);
    }
}

fn load_from(dirs: &[PathBuf], desktops: &[String]) -> Vec<App> {
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for dir in dirs {
        let mut files = Vec::new();
        collect(dir, 0, &mut files);
        files.sort();
        for path in files {
            let id = desktop_id(dir, &path);
            // The first entry with an id wins, even if it hides the app.
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&path)
                && let Some(app) = parse(&id, &text, desktops)
            {
                apps.push(app);
            }
        }
    }
    apps.sort_by_cached_key(|app| app.name.to_lowercase());
    apps
}

/// `applications` directories in priority order.
pub fn application_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    let data_dirs = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());

    std::iter::once(data_home)
        .chain(data_dirs.split(':').map(PathBuf::from))
        .map(|dir| dir.join("applications"))
        .collect()
}

fn current_desktops() -> Vec<String> {
    env::var("XDG_CURRENT_DESKTOP")
        .map(|value| {
            value
                .split(':')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn collect(dir: &Path, depth: usize, files: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for path in entries.filter_map(|entry| entry.ok().map(|e| e.path())) {
        if path.is_dir() {
            if depth < 2 {
                collect(&path, depth + 1, files);
            }
        } else if path.extension().is_some_and(|ext| ext == "desktop") {
            files.push(path);
        }
    }
}

/// Desktop file id: the path below `applications/` with `/` turned into `-`.
fn desktop_id(dir: &Path, path: &Path) -> String {
    let relative = path.strip_prefix(dir).unwrap_or(path).with_extension("");
    relative.to_string_lossy().replace('/', "-")
}

/// Parse a desktop entry; `None` if it is not an application to show.
fn parse(id: &str, text: &str, desktops: &[String]) -> Option<App> {
    let mut in_entry = false;
    let (mut name, mut generic, mut comment, mut exec) = (None, None, None, None);
    let (mut keywords, mut terminal) = (Vec::new(), false);
    let list = |value: &str| -> Vec<String> {
        value
            .split(';')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect()
    };

    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim_end(), value.trim_start());
        match key {
            "Type" if value != "Application" => return None,
            "NoDisplay" | "Hidden" if value == "true" => return None,
            "OnlyShowIn" if !list(value).iter().any(|d| desktops.contains(d)) => return None,
            "NotShowIn" if list(value).iter().any(|d| desktops.contains(d)) => return None,
            "Name" => name = Some(unescape(value)),
            "GenericName" => generic = Some(unescape(value)),
            "Comment" => comment = Some(unescape(value)),
            "Keywords" => keywords = list(value),
            "Exec" => exec = Some(value.to_owned()),
            "Terminal" => terminal = value == "true",
            _ => {}
        }
    }

    let command = parse_exec(&exec?, None);
    if command.is_empty() {
        return None;
    }
    Some(App {
        id: id.to_owned(),
        name: name?,
        description: generic.or(comment).filter(|text| !text.is_empty()),
        keywords,
        command,
        terminal,
    })
}

/// Undo desktop-entry string escapes (`\s`, `\n`, `\t`, `\\`).
fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// Split an `Exec` value into arguments: honour double quotes and backslash
/// escapes, and turn `%%` into `%`. Field codes that stand for a file or URL
/// (`%f`, `%F`, `%u`, `%U`) become `file` when there is one — appended if the
/// entry names no place for it — and every other field code is dropped.
pub fn parse_exec(exec: &str, file: Option<&str>) -> Vec<String> {
    let mut args = Vec::new();
    let mut placed = false;
    let mut current = String::new();
    let mut started = false;
    let mut quoted = false;
    let mut chars = exec.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                started = true;
            }
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                    started = true;
                }
            }
            '%' => {
                match (chars.next(), file) {
                    (Some('%'), _) => {
                        current.push('%');
                        started = true;
                    }
                    (Some('f' | 'F' | 'u' | 'U'), Some(file)) if !placed => {
                        current.push_str(file);
                        started = true;
                        placed = true;
                    }
                    // Any other field code expands to nothing.
                    _ => {}
                }
            }
            c if c.is_whitespace() && !quoted => {
                if started {
                    args.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                current.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(current);
    }
    if let Some(file) = file
        && !placed
        && !args.is_empty()
    {
        args.push(file.to_owned());
    }
    args
}

/// Start a program detached from the launcher, optionally in a directory.
pub fn spawn_detached(argv: &[String], cwd: Option<&Path>) -> io::Result<()> {
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| io::Error::other("empty command"))?;
    let mut command = Command::new(program);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        // Its own process group, so it outlives the launcher.
        .process_group(0)
        .spawn()
        .map(drop)
}

impl App {
    /// Start the application detached from the launcher.
    pub fn launch(&self, terminal: &str) -> io::Result<()> {
        let mut argv: Vec<String> = Vec::new();
        if self.terminal {
            argv.extend([terminal.to_owned(), "-e".to_owned()]);
        }
        argv.extend(self.command.iter().cloned());
        spawn_detached(&argv, None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIREFOX: &str = "\
[Desktop Entry]
Type=Application
Name=Firefox
Name[de]=Feuerfuchs
GenericName=Web Browser
Keywords=Internet;WWW;Browser;
Exec=/usr/lib/firefox/firefox %u
Terminal=false

[Desktop Action new-window]
Name=New Window
Exec=/usr/lib/firefox/firefox --new-window %u
";

    #[test]
    fn parses_the_main_entry_only() {
        let app = parse("firefox", FIREFOX, &[]).unwrap();
        assert_eq!(app.name, "Firefox");
        assert_eq!(app.description.as_deref(), Some("Web Browser"));
        assert_eq!(app.keywords, ["Internet", "WWW", "Browser"]);
        assert_eq!(app.command, ["/usr/lib/firefox/firefox"]);
        assert!(!app.terminal);
    }

    #[test]
    fn skips_hidden_and_foreign_desktop_entries() {
        let entry =
            |extra: &str| format!("[Desktop Entry]\nType=Application\nName=X\nExec=x\n{extra}\n");
        let niri = ["niri".to_owned()];
        assert!(parse("x", &entry(""), &niri).is_some());
        assert!(parse("x", &entry("NoDisplay=true"), &niri).is_none());
        assert!(parse("x", &entry("Hidden=true"), &niri).is_none());
        assert!(parse("x", &entry("OnlyShowIn=GNOME;KDE;"), &niri).is_none());
        assert!(parse("x", &entry("OnlyShowIn=GNOME;niri;"), &niri).is_some());
        assert!(parse("x", &entry("NotShowIn=niri;"), &niri).is_none());
        assert!(parse("x", "[Desktop Entry]\nType=Link\nName=X\nURL=x\n", &niri).is_none());
        assert!(parse("x", "[Desktop Entry]\nType=Application\nName=X\n", &niri).is_none());
    }

    #[test]
    fn splits_exec_lines() {
        assert_eq!(parse_exec("foo --bar %U", None), ["foo", "--bar"]);
        assert_eq!(
            parse_exec(r#"sh -c "echo hello  world" %f"#, None),
            ["sh", "-c", "echo hello  world"]
        );
        assert_eq!(
            parse_exec(r#""/opt/My App/run" --flag="a b""#, None),
            ["/opt/My App/run", "--flag=a b"]
        );
        assert_eq!(parse_exec("printf 100%%", None), ["printf", "100%"]);
        assert_eq!(
            parse_exec(r#"env VAR="" app"#, None),
            ["env", "VAR=", "app"]
        );
        assert!(parse_exec("   ", None).is_empty());
    }

    #[test]
    fn user_entries_override_system_ones() {
        let root = env::temp_dir().join(format!("enso-launch-apps-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (user, system) = (root.join("user"), root.join("system"));
        fs::create_dir_all(user.join("nested")).unwrap();
        fs::create_dir_all(&system).unwrap();

        let entry =
            |name: &str| format!("[Desktop Entry]\nType=Application\nName={name}\nExec=run\n");
        fs::write(system.join("app.desktop"), entry("System App")).unwrap();
        fs::write(user.join("app.desktop"), entry("User App")).unwrap();
        fs::write(system.join("zed.desktop"), entry("Zed")).unwrap();
        fs::write(user.join("nested/tool.desktop"), entry("alpha tool")).unwrap();
        // A user entry can hide a system application.
        fs::write(system.join("gone.desktop"), entry("Gone")).unwrap();
        fs::write(
            user.join("gone.desktop"),
            format!("{}Hidden=true\n", entry("Gone")),
        )
        .unwrap();

        let apps = load_from(&[user, system], &[]);
        let names: Vec<&str> = apps.iter().map(|app| app.name.as_str()).collect();
        assert_eq!(names, ["alpha tool", "User App", "Zed"]);
        assert_eq!(apps[0].id, "nested-tool");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_index_answers_first_and_is_corrected_when_stale() {
        let root = env::temp_dir().join(format!("enso-launch-indexed-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dirs = [root.join("applications")];
        let index = root.join("cache/apps");
        fs::create_dir_all(&dirs[0]).unwrap();
        let entry =
            |name: &str| format!("[Desktop Entry]\nType=Application\nName={name}\nExec=run\n");
        fs::write(dirs[0].join("one.desktop"), entry("One")).unwrap();

        let deliveries = || {
            let mut seen: Vec<Vec<String>> = Vec::new();
            load_indexed_from(&index, &dirs, &[], |apps| {
                seen.push(apps.into_iter().map(|app| app.name).collect());
            });
            seen
        };

        // No index yet: one delivery, from the scan, which also saves it.
        assert_eq!(deliveries(), [["One"]]);
        assert!(index.exists());
        // An up-to-date index: one delivery, and the file is left alone.
        let saved = fs::metadata(&index).unwrap().modified().unwrap();
        assert_eq!(deliveries(), [["One"]]);
        assert_eq!(fs::metadata(&index).unwrap().modified().unwrap(), saved);

        // Something was installed: the index answers, then the scan corrects it…
        fs::write(dirs[0].join("two.desktop"), entry("Two")).unwrap();
        assert_eq!(deliveries(), [vec!["One"], vec!["One", "Two"]]);
        // …so the next launch is right from the start.
        assert_eq!(deliveries(), [["One", "Two"]]);

        // An entry edited in place is caught the same way.
        fs::write(dirs[0].join("one.desktop"), entry("Uno")).unwrap();
        assert_eq!(deliveries(), [vec!["One", "Two"], vec!["Two", "Uno"]]);

        // A damaged index is ignored and replaced.
        fs::write(&index, "not an index").unwrap();
        assert_eq!(deliveries(), [["Two", "Uno"]]);
        assert_eq!(deliveries(), [["Two", "Uno"]]);
        fs::remove_dir_all(root).unwrap();
    }
}
