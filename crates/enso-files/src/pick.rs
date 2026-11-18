//! Picking: what an application asks for, and what it gets back.
//!
//! A [`Request`] says whether to open one file, many, or a folder, or where
//! to save; which kinds of file to offer ([`Filter`]s, as globs and MIME
//! types); which extra questions to ask ([`Choice`]s); and where to start.
//! It comes from the command line (`enso-files pick`) or from the desktop
//! portal, as TOML; the [`Answer`] goes back the same way.

use std::path::{Path, PathBuf};

use enso_fs::{ignores::glob, mime::MimeDb};
use serde::{Deserialize, Serialize};

/// What is being picked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Mode {
    /// Files (or, with `directory`, folders) to open.
    Open { multiple: bool, directory: bool },
    /// Where to save one file, suggesting `name`.
    Save { name: String },
    /// A folder to save these files into.
    SaveFiles { files: Vec<String> },
}

/// One kind of file to offer: "Images", as `*.png` and `image/jpeg`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Filter {
    pub name: String,
    pub patterns: Vec<Pattern>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pattern {
    Glob(String),
    Mime(String),
}

impl Filter {
    /// Whether a file called `name` is of this kind. Globs ignore case, as
    /// people do (`*.jpg` takes `HOLIDAY.JPG`).
    pub fn matches(&self, name: &str, db: &MimeDb) -> bool {
        let lower = name.to_lowercase();
        let mime = || db.by_name(name);
        self.patterns.iter().any(|pattern| match pattern {
            Pattern::Glob(pattern) => pattern == "*" || glob(&pattern.to_lowercase(), &lower),
            Pattern::Mime(wanted) => {
                wanted == "*" || wanted == "*/*" || mime().is_some_and(|mime| db.is_a(mime, wanted))
            }
        })
    }

    /// The extension a saved file of this kind takes: from the first glob
    /// that is a plain `*.ext`.
    pub fn extension(&self) -> Option<&str> {
        self.patterns.iter().find_map(|pattern| match pattern {
            Pattern::Glob(glob) => glob
                .strip_prefix("*.")
                .filter(|ext| !ext.is_empty() && !ext.contains(['*', '?', '['])),
            Pattern::Mime(_) => None,
        })
    }

    /// Parse the command line's `NAME:PATTERN,PATTERN` (a pattern with a
    /// `/` is a MIME type).
    pub fn parse(spec: &str) -> Option<Self> {
        let (name, patterns) = spec.split_once(':').unwrap_or((spec, spec));
        let patterns: Vec<Pattern> = patterns
            .split(',')
            .map(str::trim)
            .filter(|pattern| !pattern.is_empty())
            .map(|pattern| {
                if pattern.contains('/') {
                    Pattern::Mime(pattern.to_owned())
                } else {
                    Pattern::Glob(pattern.to_owned())
                }
            })
            .collect();
        (!patterns.is_empty()).then(|| Self {
            name: name.trim().to_owned(),
            patterns,
        })
    }
}

/// An extra question the application asks alongside the file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Choice {
    /// A checkbox; its value is `"true"` or `"false"`.
    Toggle { id: String, label: String, on: bool },
    /// One of several options, by id.
    Options {
        id: String,
        label: String,
        options: Vec<(String, String)>,
        selected: String,
    },
}

impl Choice {
    pub fn id(&self) -> &str {
        match self {
            Self::Toggle { id, .. } | Self::Options { id, .. } => id,
        }
    }

    pub fn value(&self) -> String {
        match self {
            Self::Toggle { on, .. } => on.to_string(),
            Self::Options { selected, .. } => selected.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub mode: Mode,
    /// The window's title, from the asking application.
    #[serde(default)]
    pub title: String,
    /// What the accept button says, if the application chose.
    #[serde(default)]
    pub accept_label: Option<String>,
    #[serde(default)]
    pub filters: Vec<Filter>,
    #[serde(default)]
    pub current_filter: Option<usize>,
    #[serde(default)]
    pub choices: Vec<Choice>,
    /// Where to start.
    #[serde(default)]
    pub folder: Option<PathBuf>,
    /// For saving over an existing file: that file.
    #[serde(default)]
    pub current_file: Option<PathBuf>,
    /// The asking window, as an `xdg-foreign` handle.
    #[serde(default)]
    pub parent: Option<String>,
    /// The asking application, as the portal names it: its last folder is
    /// remembered under this.
    #[serde(default)]
    pub app_id: String,
}

impl Request {
    pub fn new(mode: Mode) -> Self {
        Self {
            mode,
            title: String::new(),
            accept_label: None,
            filters: Vec::new(),
            current_filter: None,
            choices: Vec::new(),
            folder: None,
            current_file: None,
            parent: None,
            app_id: String::new(),
        }
    }

    pub fn multiple(&self) -> bool {
        matches!(self.mode, Mode::Open { multiple: true, .. })
    }

    /// Picking folders, not files.
    pub fn directory(&self) -> bool {
        matches!(
            self.mode,
            Mode::Open {
                directory: true,
                ..
            } | Mode::SaveFiles { .. }
        )
    }

    pub fn is_save(&self) -> bool {
        matches!(self.mode, Mode::Save { .. })
    }

    /// The title to show when the application gave none.
    pub fn title(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        match &self.mode {
            Mode::Open {
                directory: true, ..
            } => "Choose a folder",
            Mode::Open { multiple: true, .. } => "Choose files",
            Mode::Open { .. } => "Choose a file",
            Mode::Save { .. } => "Save",
            Mode::SaveFiles { .. } => "Choose where to save",
        }
        .to_owned()
    }

    /// What the accept button says when `count` things are chosen.
    pub fn accept(&self, count: usize) -> String {
        if let Some(label) = self
            .accept_label
            .as_deref()
            .filter(|label| !label.is_empty())
        {
            // Applications write mnemonics as `_Open`.
            return label.replacen('_', "", 1);
        }
        match &self.mode {
            Mode::Save { .. } | Mode::SaveFiles { .. } => "Save".to_owned(),
            Mode::Open {
                directory: true, ..
            } => "Select".to_owned(),
            Mode::Open { .. } if count > 1 => format!("Open {count} files"),
            Mode::Open { .. } => "Open".to_owned(),
        }
    }

    /// Where to start: the application's folder, the folder of the file it
    /// is saving over, or `fallback`.
    pub fn start(&self, fallback: &Path) -> PathBuf {
        self.folder
            .clone()
            .or_else(|| {
                self.current_file
                    .as_ref()
                    .and_then(|file| file.parent().map(Path::to_owned))
            })
            .filter(|folder| folder.is_dir())
            .unwrap_or_else(|| fallback.to_owned())
    }

    /// The filter to start with: the asked-for one, else the first.
    pub fn first_filter(&self) -> Option<usize> {
        match self.current_filter {
            Some(index) if index < self.filters.len() => Some(index),
            _ => (!self.filters.is_empty()).then_some(0),
        }
    }
}

/// What was picked.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Answer {
    pub paths: Vec<PathBuf>,
    /// The filter in use when it was chosen.
    #[serde(default)]
    pub filter: Option<usize>,
    /// Each choice's id and final value.
    #[serde(default)]
    pub choices: Vec<(String, String)>,
}

/// `name` with its extension made `ext`: `report.txt` → `report.pdf`. A
/// name with no extension gets one; a leading dot isn't an extension.
pub fn with_extension(name: &str, ext: &str) -> String {
    let stem = match name.rfind('.') {
        Some(dot) if dot > 0 => &name[..dot],
        _ => name,
    };
    format!("{stem}.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> MimeDb {
        MimeDb::parse(
            "50:image/png:*.png\n50:image/jpeg:*.jpg\n50:text/x-rust:*.rs\n",
            "text/x-rust text/plain\n",
            "",
        )
    }

    #[test]
    fn filters_match_by_glob_and_by_type() {
        let images = Filter::parse("Images:*.png,image/jpeg").unwrap();
        let db = db();
        assert!(images.matches("a.png", &db));
        assert!(images.matches("HOLIDAY.PNG", &db));
        assert!(images.matches("b.jpg", &db));
        assert!(!images.matches("c.rs", &db));
        let text = Filter::parse("Text:text/plain").unwrap();
        assert!(text.matches("main.rs", &db), "a kind of text");
        let any = Filter::parse("All:image/*").unwrap();
        assert!(any.matches("x.jpg", &db));
        assert!(
            Filter::parse("Everything:*")
                .unwrap()
                .matches("anything", &db)
        );
    }

    #[test]
    fn filters_parse_from_the_command_line() {
        assert_eq!(
            Filter::parse("Images: *.png, image/webp").unwrap(),
            Filter {
                name: "Images".into(),
                patterns: vec![
                    Pattern::Glob("*.png".into()),
                    Pattern::Mime("image/webp".into())
                ],
            }
        );
        // Without a name, the patterns name it.
        assert_eq!(Filter::parse("*.pdf").unwrap().name, "*.pdf");
        assert_eq!(Filter::parse("Empty:"), None);
        assert_eq!(Filter::parse("PDF:*.pdf").unwrap().extension(), Some("pdf"));
        assert_eq!(Filter::parse("Types:image/png").unwrap().extension(), None);
    }

    #[test]
    fn labels_follow_the_mode() {
        let mut request = Request::new(Mode::Open {
            multiple: true,
            directory: false,
        });
        assert_eq!(request.accept(1), "Open");
        assert_eq!(request.accept(3), "Open 3 files");
        assert_eq!(request.title(), "Choose files");
        request.accept_label = Some("_Upload".into());
        assert_eq!(request.accept(3), "Upload");
        let save = Request::new(Mode::Save {
            name: "x.txt".into(),
        });
        assert_eq!(save.accept(0), "Save");
        assert!(save.is_save());
        let folder = Request::new(Mode::Open {
            multiple: false,
            directory: true,
        });
        assert_eq!(folder.accept(0), "Select");
        assert!(folder.directory());
    }

    #[test]
    fn extensions_follow_the_filter() {
        assert_eq!(with_extension("report.txt", "pdf"), "report.pdf");
        assert_eq!(with_extension("report", "pdf"), "report.pdf");
        assert_eq!(with_extension(".config", "toml"), ".config.toml");
        assert_eq!(with_extension("a.tar.gz", "zip"), "a.tar.zip");
    }

    #[test]
    fn requests_travel_as_toml() {
        let mut request = Request::new(Mode::Save {
            name: "notes.md".into(),
        });
        request
            .filters
            .push(Filter::parse("Markdown:*.md,text/markdown").unwrap());
        request.choices.push(Choice::Toggle {
            id: "encrypt".into(),
            label: "Encrypt".into(),
            on: false,
        });
        request.folder = Some("/home/me".into());
        request.parent = Some("abc123".into());
        let text = toml::to_string(&request).unwrap();
        assert_eq!(toml::from_str::<Request>(&text).unwrap(), request);
        let answer = Answer {
            paths: vec!["/home/me/notes.md".into()],
            filter: Some(0),
            choices: vec![("encrypt".into(), "true".into())],
        };
        let text = toml::to_string(&answer).unwrap();
        assert_eq!(toml::from_str::<Answer>(&text).unwrap(), answer);
    }

    #[test]
    fn the_start_is_somewhere_that_exists() {
        let mut request = Request::new(Mode::Open {
            multiple: false,
            directory: false,
        });
        assert_eq!(
            request.start(Path::new("/fallback")),
            Path::new("/fallback")
        );
        request.folder = Some("/nonexistent/enso".into());
        assert_eq!(request.start(Path::new("/tmp")), Path::new("/tmp"));
        request.folder = Some("/".into());
        assert_eq!(request.start(Path::new("/tmp")), Path::new("/"));
    }
}
