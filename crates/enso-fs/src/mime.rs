//! File types by name, from the shared MIME database, loaded once.
//!
//! [`crate::open`] asks for one file's type at a time; a picker filtering a
//! folder of thousands asks for every name in it on each frame it changes.
//! So the database's globs are read once into tables — extensions in one
//! map, whole names in another, the few real globs in a list — and a lookup
//! is a couple of hash probes.
//!
//! Types also have parents (`text/x-rust` is a kind of `text/plain`), read
//! from `subclasses`; and the spec's implicit ones: every `text/*` is a
//! `text/plain`, and everything not a folder is an
//! `application/octet-stream`.

use std::{collections::HashMap, env, fs, path::PathBuf};

use crate::ignores::glob;

/// The type of a glob match: its weight, the pattern's length (longer
/// wins among equal weights), and the type.
type Ranked = (u32, usize, String);

#[derive(Clone, Debug, Default)]
pub struct MimeDb {
    /// `*.ext` patterns by extension, lower-cased, and the case-sensitive
    /// ones as written.
    extensions: HashMap<String, Ranked>,
    extensions_cs: HashMap<String, Ranked>,
    /// Whole-name patterns (`Makefile`), the same two ways.
    names: HashMap<String, Ranked>,
    names_cs: HashMap<String, Ranked>,
    /// Everything else: real globs, few.
    globs: Vec<(String, bool, Ranked)>,
    parents: HashMap<String, Vec<String>>,
    aliases: HashMap<String, String>,
}

impl MimeDb {
    /// The database from `$XDG_DATA_HOME` and `$XDG_DATA_DIRS`. Missing
    /// files make an empty database, which knows no types.
    pub fn load() -> Self {
        let mut globs = String::new();
        let mut subclasses = String::new();
        let mut aliases = String::new();
        // Least specific first, so the user's own definitions come last
        // and win.
        for dir in data_dirs().iter().rev() {
            let read = |name| fs::read_to_string(dir.join("mime").join(name)).unwrap_or_default();
            globs += &read("globs2");
            globs.push('\n');
            subclasses += &read("subclasses");
            subclasses.push('\n');
            aliases += &read("aliases");
            aliases.push('\n');
        }
        Self::parse(&globs, &subclasses, &aliases)
    }

    /// A database from the text of `globs2`, `subclasses`, and `aliases`.
    pub fn parse(globs2: &str, subclasses: &str, aliases: &str) -> Self {
        let mut db = Self::default();
        for line in globs2.lines().filter(|line| !line.starts_with('#')) {
            let mut fields = line.split(':');
            let (Some(weight), Some(mime), Some(pattern)) =
                (fields.next(), fields.next(), fields.next())
            else {
                continue;
            };
            let cs = fields
                .next()
                .is_some_and(|flags| flags.split(',').any(|f| f == "cs"));
            let rank: Ranked = (weight.parse().unwrap_or(50), pattern.len(), mime.to_owned());
            let keep = |map: &mut HashMap<String, Ranked>, key: String| {
                let better = map
                    .get(&key)
                    .is_none_or(|old| (rank.0, rank.1) > (old.0, old.1));
                if better {
                    map.insert(key, rank.clone());
                }
            };
            match pattern.strip_prefix("*.") {
                Some(ext) if !ext.contains(['*', '?', '[']) => {
                    if cs {
                        keep(&mut db.extensions_cs, ext.to_owned());
                    } else {
                        keep(&mut db.extensions, ext.to_lowercase());
                    }
                }
                _ if pattern.contains(['*', '?', '[']) => {
                    let pattern = if cs {
                        pattern.to_owned()
                    } else {
                        pattern.to_lowercase()
                    };
                    db.globs.push((pattern, cs, rank));
                }
                _ if cs => keep(&mut db.names_cs, pattern.to_owned()),
                _ => keep(&mut db.names, pattern.to_lowercase()),
            }
        }
        for line in subclasses.lines() {
            if let Some((child, parent)) = line.split_once(' ') {
                db.parents
                    .entry(child.to_owned())
                    .or_default()
                    .push(parent.to_owned());
            }
        }
        for line in aliases.lines() {
            if let Some((alias, canonical)) = line.split_once(' ') {
                db.aliases.insert(alias.to_owned(), canonical.to_owned());
            }
        }
        db
    }

    /// The type a file name has, if its name says.
    pub fn by_name(&self, name: &str) -> Option<&str> {
        let lower = name.to_lowercase();
        let mut best: Option<&Ranked> = None;
        consider(&mut best, self.names_cs.get(name));
        consider(&mut best, self.names.get(&lower));
        // Every extension a name could have: `a.tar.gz` is `tar.gz` and `gz`.
        for (at, _) in name.match_indices('.') {
            if at + 1 < name.len() {
                consider(&mut best, self.extensions_cs.get(&name[at + 1..]));
                consider(&mut best, self.extensions.get(&lower[at + 1..]));
            }
        }
        for (pattern, cs, rank) in &self.globs {
            let subject = if *cs { name } else { &lower };
            if glob(pattern, subject) {
                consider(&mut best, Some(rank));
            }
        }
        best.map(|(_, _, mime)| mime.as_str())
    }

    /// The canonical name of `mime`, if it is an alias.
    pub fn canonical<'a>(&'a self, mime: &'a str) -> &'a str {
        self.aliases.get(mime).map_or(mime, String::as_str)
    }

    /// Whether `mime` is `ancestor` or a kind of it. `ancestor` may be a
    /// wildcard like `image/*`.
    pub fn is_a(&self, mime: &str, ancestor: &str) -> bool {
        let ancestor = self.canonical(ancestor);
        if let Some(family) = ancestor.strip_suffix("/*") {
            return self
                .lineage(mime)
                .iter()
                .any(|kind| kind.split_once('/').is_some_and(|(top, _)| top == family));
        }
        self.lineage(mime).iter().any(|kind| kind == ancestor)
    }

    /// `mime` and every type it is a kind of.
    fn lineage(&self, mime: &str) -> Vec<String> {
        let mut found = vec![self.canonical(mime).to_owned()];
        let mut at = 0;
        while at < found.len() && found.len() < 32 {
            let current = found[at].clone();
            let mut parents = self.parents.get(&current).cloned().unwrap_or_default();
            if current.starts_with("text/") {
                parents.push("text/plain".to_owned());
            }
            if current != "inode/directory" {
                parents.push("application/octet-stream".to_owned());
            }
            for parent in parents {
                if !found.contains(&parent) {
                    found.push(parent);
                }
            }
            at += 1;
        }
        found
    }
}

/// Keep `rank` if it outweighs `best`: heavier first, then longer.
fn consider<'a>(best: &mut Option<&'a Ranked>, rank: Option<&'a Ranked>) {
    if let Some(rank) = rank
        && best.is_none_or(|old| (rank.0, rank.1) > (old.0, old.1))
    {
        *best = Some(rank);
    }
}

/// Where the shared databases live, most specific first.
fn data_dirs() -> Vec<PathBuf> {
    let home = env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let data_home = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    let dirs = env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
    std::iter::once(data_home)
        .chain(
            dirs.split(':')
                .map(PathBuf::from)
                .filter(|dir| dir.is_absolute()),
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> MimeDb {
        MimeDb::parse(
            "# comment\n\
             50:image/png:*.png\n\
             50:image/jpeg:*.jpg\n\
             50:application/gzip:*.gz\n\
             50:application/x-compressed-tar:*.tar.gz\n\
             50:text/x-rust:*.rs\n\
             50:text/x-makefile:makefile\n\
             50:text/x-csrc:*.c\n\
             50:text/x-c++src:*.C:cs\n\
             50:application/x-sharedlib:*.so.[0-9]*\n\
             10:text/x-readme:readme*\n",
            "text/x-rust text/plain\nimage/png image/x-generic\n",
            "image/jpg image/jpeg\n",
        )
    }

    #[test]
    fn names_find_their_types() {
        let db = db();
        assert_eq!(db.by_name("photo.PNG"), Some("image/png"));
        assert_eq!(db.by_name("main.rs"), Some("text/x-rust"));
        // The longer extension wins.
        assert_eq!(
            db.by_name("backup.tar.gz"),
            Some("application/x-compressed-tar")
        );
        assert_eq!(db.by_name("notes.gz"), Some("application/gzip"));
        assert_eq!(db.by_name("Makefile"), Some("text/x-makefile"));
        // Case-sensitive patterns only match as written.
        assert_eq!(db.by_name("x.C"), Some("text/x-c++src"));
        assert_eq!(db.by_name("x.c"), Some("text/x-csrc"));
        assert_eq!(db.by_name("libfoo.so.1"), Some("application/x-sharedlib"));
        assert_eq!(db.by_name("README.md"), Some("text/x-readme"));
        assert_eq!(db.by_name("mystery"), None);
        assert_eq!(db.by_name(".bashrc"), None);
    }

    #[test]
    fn types_have_ancestors() {
        let db = db();
        assert!(db.is_a("text/x-rust", "text/plain"));
        assert!(db.is_a("text/x-csrc", "text/plain"), "every text/* is text");
        assert!(db.is_a("image/png", "image/*"));
        assert!(!db.is_a("text/x-rust", "image/*"));
        assert!(db.is_a("image/png", "application/octet-stream"));
        assert!(
            db.is_a("image/jpeg", "image/jpg"),
            "aliases are the same type"
        );
        assert!(!db.is_a("image/png", "image/jpeg"));
    }

    #[test]
    fn the_real_database_loads() {
        // Wherever this runs, an empty database is still a database.
        let db = MimeDb::load();
        if !db.extensions.is_empty() {
            assert_eq!(db.by_name("x.png"), Some("image/png"));
        }
    }
}
