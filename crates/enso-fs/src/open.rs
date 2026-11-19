//! Opening a file with the application the desktop associates with it,
//! worked out from the freedesktop databases directly: the shared MIME
//! database for the type, `mimeapps.list` and `mimeinfo.cache` for the
//! application, and its desktop entry for the command.
//!
//! When nothing is associated — common on a minimal system — a text file
//! opens in `$VISUAL`/`$EDITOR` and a directory in a terminal there.

use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

use crate::apps::{self, spawn_detached};

/// An application offered for opening a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    /// Its desktop file id, `org.gnome.Loupe.desktop`.
    pub id: String,
    pub name: String,
    /// The one opening it would use.
    pub default: bool,
}

/// How a file is going to be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub argv: Vec<String>,
    /// Directory to start in, when that is the point (a terminal in a folder).
    pub cwd: Option<PathBuf>,
}

/// Open `path` and return. `terminal` runs handlers that need one.
pub fn open(path: &Path, terminal: &str) -> io::Result<()> {
    let plan = Database::from_env()
        .plan(path, terminal)
        .ok_or_else(|| io::Error::other("no application is set up to open it"))?;
    spawn_detached(&plan.argv, plan.cwd.as_deref())
}

/// Open a web address (or any URI) with what handles its scheme: the
/// browser, for `https`.
pub fn uri(uri: &str) -> io::Result<()> {
    let database = Database::from_env();
    let scheme = uri.split(':').next().unwrap_or_default();
    let argv = database
        .handlers(&format!("x-scheme-handler/{scheme}"))
        .into_iter()
        .find_map(|id| database.command(&id, uri).map(|(argv, _)| argv))
        .ok_or_else(|| {
            io::Error::other(format!(
                "no application is set up to open {scheme} addresses"
            ))
        })?;
    spawn_detached(&argv, None)
}

/// Put desktop entry `id` (`text`) among the user's applications, and make
/// it what opens each of `mimes` the user hasn't chosen anything for. A
/// tool sets itself up this way (`enso-files portal enable`); what it
/// writes is its own to take back with [`abandon`]. Returns the types taken.
pub fn adopt(id: &str, text: &str, mimes: &[&str]) -> io::Result<Vec<String>> {
    let database = Database::from_env();
    let dir = database
        .app_dirs
        .first()
        .ok_or_else(|| io::Error::other("no applications directory"))?;
    fs::create_dir_all(dir)?;
    fs::write(dir.join(id), text)?;
    let file = database.user_mimeapps()?;
    let mut list = fs::read_to_string(&file).unwrap_or_default();
    let mut taken = Vec::new();
    for mime in mimes {
        if chosen(&list, mime).is_none() {
            list = set_in_group(&list, "[Default Applications]", mime, id);
            taken.push((*mime).to_owned());
        }
    }
    if !taken.is_empty() {
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&file, list)?;
    }
    Ok(taken)
}

/// Undo [`adopt`]: the entry goes, and so does each default naming it.
pub fn abandon(id: &str) -> io::Result<()> {
    let database = Database::from_env();
    if let Some(dir) = database.app_dirs.first() {
        match fs::remove_file(dir.join(id)) {
            Err(err) if err.kind() != io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
    }
    let file = database.user_mimeapps()?;
    let Ok(list) = fs::read_to_string(&file) else {
        return Ok(());
    };
    let kept = without_default(&list, id);
    if kept != list {
        fs::write(&file, kept)?;
    }
    Ok(())
}

/// What the `[Default Applications]` group of `list` says opens `mime`.
fn chosen<'a>(list: &'a str, mime: &str) -> Option<&'a str> {
    let mut in_group = false;
    for line in list.lines().map(str::trim) {
        if line.starts_with('[') {
            in_group = line == "[Default Applications]";
        } else if in_group
            && let Some((key, value)) = line.split_once('=')
            && key.trim() == mime
        {
            return Some(value.trim());
        }
    }
    None
}

/// `list` without the defaults that are `id` alone.
fn without_default(list: &str, id: &str) -> String {
    let mut in_group = false;
    let mut out = String::with_capacity(list.len());
    for line in list.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_group = trimmed == "[Default Applications]";
        } else if in_group
            && trimmed
                .split_once('=')
                .is_some_and(|(_, value)| value.trim().trim_end_matches(';') == id)
        {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// Where the MIME and application databases live.
#[derive(Clone, Debug, Default)]
pub struct Database {
    /// Directories holding `mimeapps.list`, most specific first.
    config_dirs: Vec<PathBuf>,
    /// `applications` directories, most specific first.
    app_dirs: Vec<PathBuf>,
    /// `mime` directories of the shared MIME database.
    mime_dirs: Vec<PathBuf>,
    /// `$VISUAL` or `$EDITOR`, for text nothing else claims.
    editor: Option<String>,
}

impl Database {
    pub fn from_env() -> Self {
        let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let config_home = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .unwrap_or_else(|| home.join(".config"));
        let config_dirs = env::var("XDG_CONFIG_DIRS")
            .ok()
            .filter(|dirs| !dirs.is_empty())
            .unwrap_or_else(|| "/etc/xdg".to_owned());
        let app_dirs = apps::application_dirs();
        Self {
            config_dirs: std::iter::once(config_home)
                .chain(config_dirs.split(':').map(PathBuf::from))
                .collect(),
            mime_dirs: app_dirs
                .iter()
                .filter_map(|dir| dir.parent().map(|data| data.join("mime")))
                .collect(),
            app_dirs,
            editor: ["VISUAL", "EDITOR"]
                .iter()
                .find_map(|var| env::var(var).ok().filter(|value| !value.trim().is_empty())),
        }
    }

    /// Decide how to open `path`, or `None` if nothing can.
    pub fn plan(&self, path: &Path, terminal: &str) -> Option<Plan> {
        let file = path.to_str()?;
        let is_dir = path.is_dir();
        let types = self.types(path, is_dir);
        for mime in &types {
            for id in self.handlers(mime) {
                if let Some((mut argv, in_terminal)) = self.command(&id, file) {
                    if in_terminal {
                        argv.splice(0..0, [terminal.to_owned(), "-e".to_owned()]);
                    }
                    return Some(Plan { argv, cwd: None });
                }
            }
        }
        if is_dir {
            // A terminal in the folder beats nothing happening.
            return Some(Plan {
                argv: vec![terminal.to_owned()],
                cwd: Some(path.to_owned()),
            });
        }
        if types.iter().any(|mime| mime == "text/plain")
            && let Some(editor) = &self.editor
        {
            let mut argv = vec![terminal.to_owned(), "-e".to_owned()];
            argv.extend(editor.split_whitespace().map(str::to_owned));
            argv.push(file.to_owned());
            return Some(Plan { argv, cwd: None });
        }
        None
    }

    /// The applications that can open `path`: the one [`plan`](Self::plan)
    /// would use first, then every other that declares its type (or a type
    /// it is a kind of).
    pub fn choices(&self, path: &Path) -> Vec<Choice> {
        let mut seen = std::collections::HashSet::new();
        let mut choices: Vec<Choice> = Vec::new();
        for mime in self.types(path, path.is_dir()) {
            for id in self.handlers(&mime) {
                if !seen.insert(id.clone()) {
                    continue;
                }
                if let Some(name) = self.name(&id) {
                    let default = choices.is_empty();
                    choices.push(Choice { id, name, default });
                }
            }
        }
        choices
    }

    /// Open `path` with the application `id`.
    pub fn open_with(&self, id: &str, path: &Path, terminal: &str) -> io::Result<()> {
        let file = path
            .to_str()
            .ok_or_else(|| io::Error::other("the name isn't UTF-8"))?;
        let (mut argv, in_terminal) = self
            .command(id, file)
            .ok_or_else(|| io::Error::other(format!("{id} can't be started")))?;
        if in_terminal {
            argv.splice(0..0, [terminal.to_owned(), "-e".to_owned()]);
        }
        spawn_detached(&argv, None)
    }

    /// Make `id` what opens files of `path`'s type, from now on: a line in
    /// the user's `mimeapps.list`, the one other programs read too. Returns
    /// the type.
    pub fn set_default(&self, path: &Path, id: &str) -> io::Result<String> {
        let mime = self
            .types(path, path.is_dir())
            .into_iter()
            .next()
            .ok_or_else(|| io::Error::other("its type isn't known"))?;
        let file = self
            .config_dirs
            .first()
            .ok_or_else(|| io::Error::other("no config directory"))?
            .join("mimeapps.list");
        let text = fs::read_to_string(&file).unwrap_or_default();
        let text = set_in_group(&text, "[Default Applications]", &mime, id);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&file, text)?;
        Ok(mime)
    }

    /// The user's own `mimeapps.list`, the one choices are written to.
    fn user_mimeapps(&self) -> io::Result<PathBuf> {
        Ok(self
            .config_dirs
            .first()
            .ok_or_else(|| io::Error::other("no config directory"))?
            .join("mimeapps.list"))
    }

    /// The name desktop entry `id` gives itself.
    fn name(&self, id: &str) -> Option<String> {
        let text = self.entry(id)?;
        let mut in_entry = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
            } else if in_entry
                && let Some((key, value)) = line.split_once('=')
                && key.trim_end() == "Name"
            {
                return Some(value.trim().to_owned());
            }
        }
        None
    }

    /// The text of desktop entry `id`.
    fn entry(&self, id: &str) -> Option<String> {
        self.app_dirs.iter().find_map(|dir| {
            // `a-b.desktop` may live at `a/b.desktop`.
            let nested = id.replacen('-', "/", 1);
            fs::read_to_string(dir.join(id))
                .or_else(|_| fs::read_to_string(dir.join(nested)))
                .ok()
        })
    }

    /// The file's MIME type followed by the broader types it is a kind of.
    fn types(&self, path: &Path, is_dir: bool) -> Vec<String> {
        if is_dir {
            return vec!["inode/directory".to_owned()];
        }
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let mut types = Vec::new();
        if let Some(mime) = self.type_by_name(name) {
            types.push(mime);
        }
        // Walk up the "is a kind of" chain: text/x-rust → text/plain.
        let subclasses = self.read_all(&self.mime_dirs, "subclasses");
        let mut at = 0;
        while at < types.len() && types.len() < 16 {
            for line in subclasses.lines() {
                if let Some((child, parent)) = line.split_once(' ')
                    && child == types[at]
                    && !types.iter().any(|known| known == parent)
                {
                    types.push(parent.to_owned());
                }
            }
            at += 1;
        }
        let texty = types.iter().any(|mime| mime.starts_with("text/"));
        if !types.iter().any(|mime| mime == "text/plain")
            && (texty || (types.is_empty() && looks_like_text(path)))
        {
            types.push("text/plain".to_owned());
        }
        if types.is_empty() {
            types.push("application/octet-stream".to_owned());
        }
        types
    }

    /// The type the shared MIME database gives a file name: the heaviest
    /// glob that matches, the longest among equals.
    fn type_by_name(&self, name: &str) -> Option<String> {
        let globs = self.read_all(&self.mime_dirs, "globs2");
        let lower = name.to_lowercase();
        let mut best: Option<(u32, usize, &str)> = None;
        for line in globs.lines().filter(|line| !line.starts_with('#')) {
            let mut fields = line.split(':');
            let (Some(weight), Some(mime), Some(pattern)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let case_sensitive = fields.next().is_some_and(|flags| flags.contains("cs"));
            let matched = match pattern.strip_prefix("*.") {
                // The common shape, without a general glob match.
                Some(ext) if !ext.contains(['*', '?', '[']) => {
                    let subject = if case_sensitive { name } else { &lower };
                    subject.len() > ext.len() + 1
                        && subject.ends_with(ext)
                        && subject.as_bytes()[subject.len() - ext.len() - 1] == b'.'
                }
                _ if pattern.contains(['*', '?', '[']) => false,
                _ if case_sensitive => pattern == name,
                _ => pattern.to_lowercase() == lower,
            };
            let rank = (weight.parse().unwrap_or(50), pattern.len(), mime);
            if matched && best.is_none_or(|(w, len, _)| (rank.0, rank.1) > (w, len)) {
                best = Some(rank);
            }
        }
        best.map(|(_, _, mime)| mime.to_owned())
    }

    /// Desktop ids that open `mime`: the user's and system's chosen
    /// defaults first, then every application that lists the type.
    fn handlers(&self, mime: &str) -> Vec<String> {
        let mut ids = Vec::new();
        let lists = self
            .config_dirs
            .iter()
            .chain(&self.app_dirs)
            .map(|dir| (dir.join("mimeapps.list"), "[Default Applications]"));
        let caches = self
            .app_dirs
            .iter()
            .map(|dir| (dir.join("mimeinfo.cache"), "[MIME Cache]"));
        for (file, group) in lists.chain(caches) {
            let Ok(text) = fs::read_to_string(file) else {
                continue;
            };
            let mut in_group = false;
            for line in text.lines().map(str::trim) {
                if line.starts_with('[') {
                    in_group = line == group;
                } else if in_group
                    && let Some((key, value)) = line.split_once('=')
                    && key.trim() == mime
                {
                    ids.extend(
                        value
                            .split(';')
                            .map(str::trim)
                            .filter(|id| !id.is_empty())
                            .map(str::to_owned),
                    );
                }
            }
        }
        ids
    }

    /// The command line of desktop entry `id` for `file`, and whether it
    /// wants a terminal.
    fn command(&self, id: &str, file: &str) -> Option<(Vec<String>, bool)> {
        let text = self.entry(id)?;
        let (mut exec, mut terminal, mut in_entry) = (None, false, false);
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_entry = line == "[Desktop Entry]";
            } else if in_entry && let Some((key, value)) = line.split_once('=') {
                match key.trim_end() {
                    "Exec" => exec = Some(value.trim_start().to_owned()),
                    "Terminal" => terminal = value.trim() == "true",
                    "Hidden" if value.trim() == "true" => return None,
                    _ => {}
                }
            }
        }
        let argv = apps::parse_exec(&exec?, Some(file));
        (!argv.is_empty()).then_some((argv, terminal))
    }

    /// The same-named file from every directory, concatenated.
    fn read_all(&self, dirs: &[PathBuf], name: &str) -> String {
        dirs.iter()
            .filter_map(|dir| fs::read_to_string(dir.join(name)).ok())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// `text` (an INI-style file) with `key=value` set in `group`, every other
/// line kept as it was; the group is added if missing.
fn set_in_group(text: &str, group: &str, key: &str, value: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let (mut in_group, mut found_group, mut set) = (false, false, false);
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_group && !set {
                out.push(format!("{key}={value}"));
                set = true;
            }
            in_group = trimmed == group;
            found_group |= in_group;
        } else if in_group
            && trimmed
                .split_once('=')
                .is_some_and(|(k, _)| k.trim() == key)
        {
            if !set {
                out.push(format!("{key}={value}"));
                set = true;
            }
            continue;
        }
        out.push(line.to_owned());
    }
    if in_group && !set {
        out.push(format!("{key}={value}"));
    } else if !found_group {
        if out.last().is_some_and(|line| !line.is_empty()) {
            out.push(String::new());
        }
        out.push(group.to_owned());
        out.push(format!("{key}={value}"));
    }
    out.join("\n") + "\n"
}

/// No type by name: call it text if its first kilobyte reads as text.
fn looks_like_text(path: &Path) -> bool {
    use std::io::Read;
    let mut buffer = [0; 1024];
    let Ok(read) = fs::File::open(path).and_then(|mut file| file.read(&mut buffer)) else {
        return false;
    };
    let head = &buffer[..read];
    // A multi-byte character may be cut at the end; that is still text.
    let valid = match std::str::from_utf8(head) {
        Ok(_) => true,
        Err(err) => err.error_len().is_none(),
    };
    valid && !head.contains(&0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopting_leaves_the_users_choices_alone() {
        let list = "[Default Applications]\nimage/png=org.gnome.Loupe.desktop\ninode/directory=enso-files.desktop\n\n[Added Associations]\ntext/plain=enso-files.desktop\n";
        assert_eq!(chosen(list, "image/png"), Some("org.gnome.Loupe.desktop"));
        assert_eq!(chosen(list, "image/jpeg"), None);
        let kept = without_default(list, "enso-files.desktop");
        assert!(!kept.contains("inode/directory"));
        // Only the defaults, not other groups, and not others' choices.
        assert!(kept.contains("text/plain=enso-files.desktop"));
        assert!(kept.contains("image/png=org.gnome.Loupe.desktop"));
    }

    #[test]
    fn defaults_are_set_without_touching_the_rest() {
        let list = "[Added Associations]\nimage/png=loupe.desktop;\n\n[Default Applications]\nimage/png=old.desktop\ntext/plain=nvim.desktop\n";
        let set = set_in_group(list, "[Default Applications]", "image/png", "imv.desktop");
        assert_eq!(
            set,
            "[Added Associations]\nimage/png=loupe.desktop;\n\n[Default Applications]\nimage/png=imv.desktop\ntext/plain=nvim.desktop\n"
        );
        let added = set_in_group(
            "[Added Associations]\na=b\n",
            "[Default Applications]",
            "image/png",
            "imv.desktop",
        );
        assert_eq!(
            added,
            "[Added Associations]\na=b\n\n[Default Applications]\nimage/png=imv.desktop\n"
        );
        assert_eq!(
            set_in_group("", "[Default Applications]", "x", "y.desktop"),
            "[Default Applications]\nx=y.desktop\n"
        );
    }

    /// A sandbox with a small MIME database and two applications.
    fn sandbox(name: &str) -> (PathBuf, Database) {
        let root = env::temp_dir().join(format!("enso-fs-open-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (config, data) = (root.join("config"), root.join("share"));
        fs::create_dir_all(&config).unwrap();
        fs::create_dir_all(data.join("applications")).unwrap();
        fs::create_dir_all(data.join("mime")).unwrap();
        fs::write(
            data.join("mime/globs2"),
            "# comment\n50:text/markdown:*.md\n50:text/rust:*.rs\n50:image/png:*.png\n\
             50:text/x-makefile:makefile\n60:application/x-compressed-tar:*.tar.gz\n\
             50:application/gzip:*.gz\n",
        )
        .unwrap();
        fs::write(
            data.join("mime/subclasses"),
            "text/markdown text/plain\ntext/rust text/x-csrc\ntext/x-csrc text/plain\n",
        )
        .unwrap();
        let entry = |exec: &str, extra: &str| {
            format!("[Desktop Entry]\nType=Application\nName=X\nExec={exec}\n{extra}\n")
        };
        fs::write(
            data.join("applications/viewer.desktop"),
            entry("viewer --open %f", ""),
        )
        .unwrap();
        fs::write(
            data.join("applications/edit.desktop"),
            entry("edit %F", "Terminal=true"),
        )
        .unwrap();
        fs::write(
            data.join("applications/mimeinfo.cache"),
            "[MIME Cache]\nimage/png=viewer.desktop;\ntext/plain=missing.desktop;edit.desktop;\n",
        )
        .unwrap();
        let database = Database {
            config_dirs: vec![config],
            app_dirs: vec![data.join("applications")],
            mime_dirs: vec![data.join("mime")],
            editor: None,
        };
        (root, database)
    }

    #[test]
    fn choices_name_every_handler_and_defaults_stick() {
        let (root, db) = sandbox("choices");
        let md = root.join("notes.md");
        fs::write(&md, "").unwrap();
        let names = |db: &Database| -> Vec<(String, bool)> {
            db.choices(&md)
                .into_iter()
                .map(|choice| (choice.id, choice.default))
                .collect()
        };
        // Only the text/plain handler that exists.
        assert_eq!(names(&db), [("edit.desktop".to_owned(), true)]);
        assert_eq!(
            db.set_default(&md, "viewer.desktop").unwrap(),
            "text/markdown"
        );
        assert_eq!(
            names(&db),
            [
                ("viewer.desktop".to_owned(), true),
                ("edit.desktop".to_owned(), false)
            ]
        );
        assert_eq!(argv(db.plan(&md, "term"))[0], "viewer");
        fs::remove_dir_all(root).unwrap();
    }

    fn argv(plan: Option<Plan>) -> Vec<String> {
        plan.unwrap().argv
    }

    #[test]
    fn types_come_from_the_name_and_widen() {
        let (root, db) = sandbox("types");
        let types = |name: &str| db.types(&root.join(name), false);
        assert_eq!(types("notes.md"), ["text/markdown", "text/plain"]);
        assert_eq!(types("main.rs"), ["text/rust", "text/x-csrc", "text/plain"]);
        assert_eq!(types("PHOTO.PNG"), ["image/png"]);
        assert_eq!(types("Makefile"), ["text/x-makefile", "text/plain"]);
        // The heavier, longer glob wins over the plain extension.
        assert_eq!(types("src.tar.gz"), ["application/x-compressed-tar"]);
        assert_eq!(types("gone.bin"), ["application/octet-stream"]);
        assert_eq!(db.types(&root, true), ["inode/directory"]);

        // No extension: the contents decide between text and not.
        fs::write(root.join("README"), "plain words — with a dash").unwrap();
        fs::write(root.join("blob"), [0u8, 159, 146, 150]).unwrap();
        assert_eq!(types("README"), ["text/plain"]);
        assert_eq!(types("blob"), ["application/octet-stream"]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn picks_the_associated_application() {
        let (root, db) = sandbox("plan");
        let png = root.join("my cat.png");
        let md = root.join("notes.md");
        fs::write(&png, "").unwrap();
        fs::write(&md, "").unwrap();
        let (png_arg, md_arg) = (png.to_str().unwrap(), md.to_str().unwrap());

        // The path is one argument, spaces and all.
        assert_eq!(argv(db.plan(&png, "term")), ["viewer", "--open", png_arg]);
        // Markdown has no handler of its own: the text/plain one opens it,
        // skipping an id whose entry is gone, inside a terminal as it asks.
        assert_eq!(argv(db.plan(&md, "term")), ["term", "-e", "edit", md_arg]);

        // A chosen default beats the cache.
        fs::write(
            db.config_dirs[0].join("mimeapps.list"),
            "[Added Associations]\ntext/markdown=edit.desktop;\n\n\
             [Default Applications]\ntext/markdown=viewer.desktop\n",
        )
        .unwrap();
        assert_eq!(argv(db.plan(&md, "term")), ["viewer", "--open", md_arg]);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn falls_back_when_nothing_is_associated() {
        let (root, mut db) = sandbox("fallback");
        fs::remove_file(db.app_dirs[0].join("mimeinfo.cache")).unwrap();
        let md = root.join("notes.md");
        let bin = root.join("thing.bin");
        fs::write(&md, "").unwrap();
        fs::write(&bin, [0u8; 8]).unwrap();

        // A folder opens a terminal there.
        assert_eq!(
            db.plan(&root, "term"),
            Some(Plan {
                argv: vec!["term".to_owned()],
                cwd: Some(root.clone()),
            })
        );
        // Text needs an editor to be named.
        assert_eq!(db.plan(&md, "term"), None);
        db.editor = Some("nvim -p".to_owned());
        assert_eq!(
            argv(db.plan(&md, "term")),
            ["term", "-e", "nvim", "-p", md.to_str().unwrap()]
        );
        // Nothing sensible opens an unknown binary.
        assert_eq!(db.plan(&bin, "term"), None);
        fs::remove_dir_all(root).unwrap();
    }
}
