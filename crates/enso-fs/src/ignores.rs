//! What the file search leaves out, and why.
//!
//! Three sources, each able to overrule the one below it:
//!
//! 1. the user's own list (`ignore` under `[launch]`), where `!pattern`
//!    keeps something the others would drop;
//! 2. `.gitignore` and `.ignore` files, read as Git reads them — but
//!    wherever they are found, repository or not;
//! 3. the built-in list in this file: the things `.gitignore` templates
//!    everywhere agree nobody wants to find.
//!
//! The built-in list is deliberately careful about names that only
//! *sometimes* mean build output. `node_modules` is never anything else, so
//! it is dropped on sight ([`NAMES`]). `target`, `build`, `dist`, `bin` are
//! ordinary words — `~/bin` is where people keep their scripts — so they are
//! dropped only next to the file that makes them build output: `target`
//! beside a `Cargo.toml`, `bin` beside a `.csproj` ([`OUTPUTS`]).
//!
//! # Maintaining the list
//!
//! Add to [`NAMES`] only what is junk wherever it appears; anything that
//! could be a folder someone made on purpose goes in [`OUTPUTS`] with the
//! project files that give it away. Order within the lists does not matter.
//! The reference is <https://github.com/github/gitignore>.

use std::sync::{Arc, LazyLock};

/// Version-control internals. Left out even with the built-in list turned
/// off: nothing in them is a file anyone opens.
pub const VCS: &[&str] = &[
    ".git", ".hg", ".svn", ".jj", ".bzr", ".pijul", "_darcs", "CVS",
];

/// A directory holding one of these files is, by its own account, not worth
/// searching. `CACHEDIR.TAG` marks regenerable contents
/// (<https://bford.info/cachedir/>; Cargo's `target` carries one) and
/// `pyvenv.cfg` a Python virtual environment, whatever it is called.
pub const TELLTALES: &[&str] = &["CACHEDIR.TAG", "pyvenv.cfg"];

/// Names that are junk wherever they turn up: dependency trees, caches,
/// tool state, operating-system litter.
pub const NAMES: &[&str] = &[
    // JavaScript and the web.
    "node_modules",
    "bower_components",
    "jspm_packages",
    "web_modules",
    "npm_cache",
    "_cacache",
    ".npm",
    ".yarn",
    ".pnpm-store",
    ".next",
    ".nuxt",
    ".output",
    ".svelte-kit",
    ".parcel-cache",
    ".turbo",
    ".vercel",
    ".netlify",
    ".angular",
    ".sass-cache",
    ".nyc_output",
    ".docusaurus",
    ".expo",
    ".serverless",
    "npm-debug.log",
    "yarn-error.log",
    // Python.
    "__pycache__",
    "__pypackages__",
    "site-packages",
    "htmlcov",
    ".venv",
    ".tox",
    ".nox",
    ".eggs",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".hypothesis",
    ".ipynb_checkpoints",
    ".pyenv",
    // Rust, Go, Zig, Haskell, Elm, Dart, Ruby.
    ".cargo",
    ".rustup",
    "zig-cache",
    ".zig-cache",
    "zig-out",
    ".stack-work",
    "dist-newstyle",
    "elm-stuff",
    ".dart_tool",
    ".pub-cache",
    ".bundle",
    ".rbenv",
    // JVM and Android.
    ".gradle",
    ".kotlin",
    ".m2",
    ".ivy2",
    ".sbt",
    ".metals",
    ".bloop",
    ".bsp",
    // C, C++, Apple.
    "CMakeFiles",
    ".ccls-cache",
    ".clangd",
    "DerivedData",
    // Editors and tools.
    ".idea",
    ".vs",
    ".vscode-test",
    ".direnv",
    ".terraform",
    ".vagrant",
    ".nvm",
    // Caches by their plain names.
    ".cache",
    "cache",
    "Cache",
    "caches",
    "Caches",
    "CachedData",
    "GPUCache",
    "ShaderCache",
    // Operating-system litter.
    "lost+found",
    ".Trash",
    ".Trashes",
    "Trash",
    "$RECYCLE.BIN",
    "System Volume Information",
    ".Spotlight-V100",
    ".fseventsd",
    "__MACOSX",
    ".DS_Store",
    "Thumbs.db",
    "desktop.ini",
    ".thumbnails",
];

/// Endings that mark a file as a by-product: compiled objects, bytecode,
/// editor swap and backup files.
pub const SUFFIXES: &[&str] = &[
    ".o",
    ".obj",
    ".pyc",
    ".pyo",
    ".class",
    ".rlib",
    ".rmeta",
    ".swp",
    ".swo",
    ".egg-info",
    "~",
];

/// Beginnings that do the same.
pub const PREFIXES: &[&str] = &["cmake-build-"];

/// Names that are build output only in the right company: each is left out
/// when a file beside it matches one of the listed names (`*.ext` matches
/// by ending).
pub const OUTPUTS: &[(&str, &[&str])] = &[
    (
        "target",
        &["Cargo.toml", "pom.xml", "build.sbt", "project.clj"],
    ),
    (
        "build",
        &[
            "CMakeLists.txt",
            "meson.build",
            "Makefile",
            "makefile",
            "GNUmakefile",
            "configure",
            "build.gradle",
            "build.gradle.kts",
            "settings.gradle",
            "settings.gradle.kts",
            "package.json",
            "setup.py",
            "pyproject.toml",
            "build.zig",
            "pubspec.yaml",
        ],
    ),
    (
        "dist",
        &[
            "package.json",
            "setup.py",
            "setup.cfg",
            "pyproject.toml",
            "*.cabal",
        ],
    ),
    (
        "out",
        &[
            "package.json",
            "build.gradle",
            "build.gradle.kts",
            "foundry.toml",
            "*.iml",
        ],
    ),
    ("vendor", &["go.mod", "composer.json", "Gemfile"]),
    ("bin", &["*.csproj", "*.fsproj", "*.vbproj", "*.sln"]),
    ("obj", &["*.csproj", "*.fsproj", "*.vbproj", "*.sln"]),
    ("Debug", &["*.sln", "*.vcxproj"]),
    ("Release", &["*.sln", "*.vcxproj"]),
    ("x64", &["*.sln", "*.vcxproj"]),
    ("x86", &["*.sln", "*.vcxproj"]),
    (
        "coverage",
        &["package.json", "Gemfile", "pyproject.toml", ".coveragerc"],
    ),
    (
        "_build",
        &["mix.exs", "dune-project", "conf.py", "Makefile"],
    ),
    ("deps", &["mix.exs"]),
    ("tmp", &["Gemfile", "package.json"]),
    ("log", &["Gemfile"]),
    ("logs", &["package.json"]),
    ("Pods", &["Podfile"]),
    ("Carthage", &["Cartfile"]),
    (
        "bazel-bin",
        &["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel"],
    ),
    (
        "bazel-out",
        &["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel"],
    ),
    (
        "bazel-testlogs",
        &["WORKSPACE", "WORKSPACE.bazel", "MODULE.bazel"],
    ),
    ("env", &["requirements.txt", "pyproject.toml", "setup.py"]),
    ("venv", &["requirements.txt", "pyproject.toml", "setup.py"]),
];

/// [`NAMES`], sorted once so a lookup is a handful of comparisons.
static SORTED_NAMES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    let mut names = NAMES.to_vec();
    names.sort_unstable();
    names
});

/// Whether the built-in list leaves `name` out, in a directory holding
/// `siblings`.
pub fn is_junk<'a>(name: &str, siblings: impl Fn() -> &'a [(String, bool)]) -> bool {
    if SORTED_NAMES.binary_search(&name).is_ok()
        || SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
        || PREFIXES.iter().any(|prefix| name.starts_with(prefix))
    {
        return true;
    }
    let Some((_, company)) = OUTPUTS.iter().find(|(output, _)| *output == name) else {
        return false;
    };
    siblings().iter().any(|(sibling, _)| {
        company.iter().any(|marker| match marker.strip_prefix('*') {
            Some(ending) => sibling.ends_with(ending),
            None => sibling == marker,
        })
    })
}

/// Shell-style match of one path component: `*` is any run of characters,
/// `?` any one, `[a-z]` and `[!a-z]` a set, and `\` takes the next
/// character literally. No character is special for being a `/`: callers
/// match component by component.
pub fn glob(pattern: &str, name: &str) -> bool {
    let (pattern, name) = (pattern.as_bytes(), name.as_bytes());
    let (mut p, mut n) = (0, 0);
    // Where to resume after the last `*` if the match goes wrong.
    let mut retry: Option<(usize, usize)> = None;
    while n < name.len() {
        let step = match pattern.get(p) {
            Some(b'*') => {
                retry = Some((p, n));
                p += 1;
                continue;
            }
            // One whole character, however many bytes it takes.
            Some(b'?') => Some((p + 1, n + char_len(name[n]))),
            Some(b'[') => match set(pattern, p, name[n]) {
                Some((true, after)) => Some((after, n + 1)),
                Some((false, _)) => None,
                // No closing bracket: an ordinary `[`.
                None => (name[n] == b'[').then_some((p + 1, n + 1)),
            },
            Some(b'\\') if p + 1 < pattern.len() => {
                (pattern[p + 1] == name[n]).then_some((p + 2, n + 1))
            }
            Some(&byte) => (byte == name[n]).then_some((p + 1, n + 1)),
            None => None,
        };
        match (step, retry) {
            (Some((next_p, next_n)), _) => {
                p = next_p;
                n = next_n;
            }
            (None, Some((star, at))) => {
                let at = at + char_len(name[at]);
                retry = Some((star, at));
                p = star + 1;
                n = at;
            }
            (None, None) => return false,
        }
    }
    pattern[p..].iter().all(|byte| *byte == b'*')
}

/// Bytes in the UTF-8 character that starts with `first`.
fn char_len(first: u8) -> usize {
    match first {
        0xF0.. => 4,
        0xE0.. => 3,
        0xC0.. => 2,
        _ => 1,
    }
}

/// Match `byte` against the bracket expression starting at `pattern[open]`.
/// Returns whether it matched and where the pattern continues, or `None`
/// if the bracket is never closed.
fn set(pattern: &[u8], open: usize, byte: u8) -> Option<(bool, usize)> {
    let mut at = open + 1;
    let negated = matches!(pattern.get(at), Some(b'!' | b'^'));
    if negated {
        at += 1;
    }
    let first = at;
    let mut matched = false;
    loop {
        let low = *pattern.get(at)?;
        // A `]` right after the opening is a member, not the end.
        if low == b']' && at > first {
            return Some((matched != negated, at + 1));
        }
        if pattern.get(at + 1) == Some(&b'-') && pattern.get(at + 2).is_some_and(|b| *b != b']') {
            matched |= (low..=pattern[at + 2]).contains(&byte);
            at += 3;
        } else {
            matched |= low == byte;
            at += 1;
        }
    }
}

/// One line of a `.gitignore`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Rule {
    /// `!pattern`: what matches is kept after all.
    keep: bool,
    /// `pattern/`: matches directories only.
    dir_only: bool,
    /// The pattern names a path from the file's own directory, rather than
    /// a name to match at any depth.
    anchored: bool,
    parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Part {
    /// `**`: any number of directories, including none.
    Any,
    Glob(String),
}

impl Rule {
    fn parse(line: &str) -> Option<Self> {
        let line = line.trim_end_matches('\r');
        // Trailing spaces do not count unless escaped.
        let mut text = line.trim_end_matches(' ');
        if text.ends_with('\\') && text.len() < line.len() {
            text = &line[..text.len() + 1];
        }
        if text.is_empty() || text.starts_with('#') {
            return None;
        }
        let keep = text.starts_with('!');
        let text = if keep { &text[1..] } else { text };
        let dir_only = text.ends_with('/');
        let text = text.trim_end_matches('/');
        // A slash anywhere but the end ties the pattern to this directory.
        let anchored = text.contains('/');
        let parts: Vec<Part> = text
            .split('/')
            .filter(|part| !part.is_empty())
            .map(|part| match part {
                "**" => Part::Any,
                glob => Part::Glob(glob.to_owned()),
            })
            .collect();
        (!parts.is_empty()).then_some(Self {
            keep,
            dir_only,
            anchored,
            parts,
        })
    }

    /// Whether the rule matches the entry `name`, reached from the rule
    /// file's directory through the directories `above`.
    fn matches(&self, above: &[&str], name: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        match (&self.parts[..], self.anchored) {
            ([Part::Glob(pattern)], false) => glob(pattern, name),
            (parts, _) => match_path(parts, above, Some(name)),
        }
    }
}

/// Match pattern parts against a path given as its directories and then,
/// if not yet consumed, its last component.
fn match_path(parts: &[Part], above: &[&str], name: Option<&str>) -> bool {
    match parts.split_first() {
        None => above.is_empty() && name.is_none(),
        // A trailing `**` is everything inside, so something must follow.
        Some((Part::Any, [])) => !above.is_empty() || name.is_some(),
        Some((Part::Any, rest)) => {
            if match_path(rest, above, name) {
                return true;
            }
            match above.split_first() {
                Some((_, deeper)) => match_path(parts, deeper, name),
                None => name.is_some() && match_path(parts, &[], None),
            }
        }
        Some((Part::Glob(pattern), rest)) => match above.split_first() {
            Some((dir, deeper)) => glob(pattern, dir) && match_path(rest, deeper, name),
            None => name.is_some_and(|name| glob(pattern, name)) && match_path(rest, &[], None),
        },
    }
}

/// The rules in force somewhere in the tree: one ignore file's, and behind
/// them those of every directory above.
#[derive(Debug)]
pub struct Rules {
    /// Length of the path of the directory the file was found in.
    base: usize,
    rules: Vec<Rule>,
    /// Some rule needs the path, not just the name.
    needs_path: bool,
    parent: Option<Arc<Rules>>,
}

impl Rules {
    /// The rules of an ignore file found in `dir`, on top of `parent`'s.
    /// A file with no rules in it adds nothing.
    pub fn layer(text: &str, dir: &str, parent: Option<Arc<Self>>) -> Option<Arc<Self>> {
        let rules: Vec<Rule> = text.lines().filter_map(Rule::parse).collect();
        if rules.is_empty() {
            return parent;
        }
        Some(Arc::new(Self {
            base: dir.trim_end_matches('/').len(),
            needs_path: rules
                .iter()
                .any(|rule| rule.anchored || rule.parts.len() > 1),
            rules,
            parent,
        }))
    }

    /// What the rules say about the entry `name` in `dir`: `Some(true)` to
    /// leave it out, `Some(false)` to keep it, `None` if they are silent.
    /// As in Git, the last rule that matches decides, and a directory's
    /// own file outranks those above it.
    pub fn verdict(&self, dir: &str, name: &str, is_dir: bool) -> Option<bool> {
        let mut layer = Some(self);
        let mut above: Vec<&str> = Vec::new();
        while let Some(rules) = layer {
            if rules.needs_path {
                above.clear();
                above.extend(
                    dir.get(rules.base..)
                        .unwrap_or_default()
                        .split('/')
                        .filter(|part| !part.is_empty()),
                );
            }
            let decided = rules
                .rules
                .iter()
                .rev()
                .find(|rule| rule.matches(&above, name, is_dir));
            if let Some(rule) = decided {
                return Some(!rule.keep);
            }
            layer = rules.parent.as_deref();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        assert!(glob("*.iso", "arch.iso"));
        assert!(!glob("*.iso", "arch.iso.sig"));
        assert!(glob("a*b*c", "aXXbYYc"));
        assert!(glob("a*b*c", "abbc"));
        assert!(!glob("a*b*c", "aXXbYY"));
        assert!(glob("no?es", "notes"));
        assert!(!glob("no?es", "noes"));
        assert!(glob("*", ""));
        assert!(glob("Downloads", "Downloads"));
        assert!(!glob("Downloads", "downloads"));
        // One `?` is one character, not one byte.
        assert!(glob("caf?", "café"));
        assert!(glob("*é", "café"));
        assert!(!glob("caf?", "cafés"));
    }

    #[test]
    fn glob_sets_and_escapes() {
        assert!(glob("*.[oa]", "main.o"));
        assert!(glob("*.[oa]", "lib.a"));
        assert!(!glob("*.[oa]", "main.c"));
        assert!(glob("log[0-9].txt", "log7.txt"));
        assert!(!glob("log[0-9].txt", "logx.txt"));
        assert!(glob("[!.]*", "visible"));
        assert!(!glob("[!.]*", ".hidden"));
        assert!(glob("[]x]", "]"));
        // An unclosed bracket, and escaped specials, are themselves.
        assert!(glob("a[b", "a[b"));
        assert!(glob(r"\*.txt", "*.txt"));
        assert!(!glob(r"\*.txt", "a.txt"));
        assert!(glob(r"\#notes", "#notes"));
    }

    #[test]
    fn junk_is_junk_everywhere_but_outputs_need_company() {
        let alone: Vec<(String, bool)> = Vec::new();
        let junk = |name: &str| is_junk(name, || &alone);
        assert!(junk("node_modules"));
        assert!(junk("__pycache__"));
        assert!(junk("npm_cache"));
        assert!(junk("cache"));
        assert!(junk("main.o"));
        assert!(junk("module.pyc"));
        assert!(junk("notes.txt~"));
        assert!(junk("cmake-build-debug"));
        assert!(junk("thing.egg-info"));
        assert!(!junk("notes.txt"));
        assert!(!junk("src"));
        // Ordinary words on their own are ordinary folders.
        for name in ["target", "build", "dist", "bin", "vendor", "out", "env"] {
            assert!(!junk(name), "{name} alone should be kept");
        }

        let beside = |files: &[&str]| -> Vec<(String, bool)> {
            files
                .iter()
                .map(|file| ((*file).to_owned(), false))
                .collect()
        };
        let rust = beside(&["Cargo.toml", "src", "target"]);
        assert!(is_junk("target", || &rust));
        assert!(!is_junk("build", || &rust));
        let node = beside(&["package.json", "dist", "build", "coverage"]);
        assert!(is_junk("dist", || &node) && is_junk("build", || &node));
        assert!(!is_junk("target", || &node));
        let dotnet = beside(&["App.csproj", "bin", "obj"]);
        assert!(is_junk("bin", || &dotnet) && is_junk("obj", || &dotnet));
        let scripts = beside(&["backup.sh", "bin"]);
        assert!(!is_junk("bin", || &scripts));
    }

    #[test]
    fn the_lists_hold_no_duplicates_or_paths() {
        let mut names = NAMES.to_vec();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(names.len(), before, "NAMES lists something twice");
        assert!(NAMES.iter().chain(VCS).all(|name| !name.contains('/')));
        // Something in both lists would never reach its company check.
        assert!(OUTPUTS.iter().all(|(name, _)| !NAMES.contains(name)));
    }

    /// What one ignore file in `/repo` says about a path below it.
    fn verdict(file: &str, path: &str) -> Option<bool> {
        let rules = Rules::layer(file, "/repo", None)?;
        let is_dir = path.ends_with('/');
        let path = path.trim_end_matches('/');
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        let dir = if dir.is_empty() {
            "/repo".to_owned()
        } else {
            format!("/repo/{dir}")
        };
        rules.verdict(&dir, name, is_dir)
    }

    #[test]
    fn gitignore_names_match_at_any_depth() {
        let file = "# build output\n\n*.log\ntarget/\n";
        assert_eq!(verdict(file, "debug.log"), Some(true));
        assert_eq!(verdict(file, "a/b/debug.log"), Some(true));
        assert_eq!(verdict(file, "notes.txt"), None);
        // A trailing slash means directories only.
        assert_eq!(verdict(file, "target/"), Some(true));
        assert_eq!(verdict(file, "crates/x/target/"), Some(true));
        assert_eq!(verdict(file, "target"), None);
        // Comments and blank lines are not rules.
        assert!(Rules::layer("# nothing\n\n   \n", "/repo", None).is_none());
    }

    #[test]
    fn gitignore_paths_are_tied_to_their_directory() {
        let file = "/dist\ndocs/_site\nsrc/*.gen.rs\n";
        assert_eq!(verdict(file, "dist/"), Some(true));
        assert_eq!(verdict(file, "packages/app/dist/"), None);
        assert_eq!(verdict(file, "docs/_site/"), Some(true));
        assert_eq!(verdict(file, "other/docs/_site/"), None);
        assert_eq!(verdict(file, "src/lexer.gen.rs"), Some(true));
        // `*` stops at a slash.
        assert_eq!(verdict(file, "src/deep/lexer.gen.rs"), None);
    }

    #[test]
    fn gitignore_double_stars() {
        let file = "**/generated\nlogs/**\nassets/**/*.min.js\n";
        assert_eq!(verdict(file, "generated/"), Some(true));
        assert_eq!(verdict(file, "a/b/generated"), Some(true));
        // Everything inside, but not the directory itself.
        assert_eq!(verdict(file, "logs/today.txt"), Some(true));
        assert_eq!(verdict(file, "logs/2026/11/today.txt"), Some(true));
        assert_eq!(verdict(file, "logs/"), None);
        assert_eq!(verdict(file, "assets/app.min.js"), Some(true));
        assert_eq!(verdict(file, "assets/vendor/x/app.min.js"), Some(true));
        assert_eq!(verdict(file, "assets/vendor/app.js"), None);
        assert_eq!(verdict(file, "other/app.min.js"), None);
    }

    #[test]
    fn gitignore_last_match_wins_and_exceptions_keep() {
        let file = "*.log\n!important.log\n";
        assert_eq!(verdict(file, "debug.log"), Some(true));
        assert_eq!(verdict(file, "important.log"), Some(false));
        // Order matters: the exception first is undone by the rule after.
        let file = "!important.log\n*.log\n";
        assert_eq!(verdict(file, "important.log"), Some(true));
        // Escapes: a name that begins with `#` or `!`, a trailing space.
        let file = "\\#notes\n\\!shout\ntrailing\\ \nplain   \n";
        assert_eq!(verdict(file, "#notes"), Some(true));
        assert_eq!(verdict(file, "!shout"), Some(true));
        assert_eq!(verdict(file, "trailing "), Some(true));
        assert_eq!(verdict(file, "plain"), Some(true));
    }

    #[test]
    fn a_nested_file_outranks_the_one_above() {
        let top = Rules::layer("*.log\n/build\n", "/repo", None);
        let nested = Rules::layer("!keep.log\n/local\n", "/repo/sub", top.clone()).unwrap();
        let top = top.unwrap();
        // Above the nested file only the top one speaks.
        assert_eq!(top.verdict("/repo", "keep.log", false), Some(true));
        // Below it, its exception wins; everything else falls through.
        assert_eq!(nested.verdict("/repo/sub", "keep.log", false), Some(false));
        assert_eq!(
            nested.verdict("/repo/sub/deep", "other.log", false),
            Some(true)
        );
        // Each file's anchored paths are relative to its own directory.
        assert_eq!(nested.verdict("/repo/sub", "local", true), Some(true));
        assert_eq!(nested.verdict("/repo/sub", "build", true), None);
        assert_eq!(top.verdict("/repo", "build", true), Some(true));
        assert_eq!(top.verdict("/repo", "local", true), None);
    }
}
