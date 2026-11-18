//! Renaming many at once: what each would be called, worked out without
//! touching anything, so the window can show every name before and after.
//!
//! The rule works on each name without its extension (a folder's whole
//! name), which stays as it was:
//!
//! - with **Find** empty, **Replace with** is the whole new name, a
//!   template: `Holiday {n:02}` makes `Holiday 01`, `Holiday 02`…;
//! - with **Find**, what it finds is replaced — every time it appears —
//!   by **Replace with**, literally, or as a regular expression whose
//!   groups `$1`, `$2`… the replacement can use.
//!
//! Either way the replacement may say `{n}` (counting from 1, `{n:03}`
//! three digits wide), `{name}` (the name as it was), `{ext}`, `{date}`
//! (when it was changed, `2026-11-18`), and `{folder}`. Then the case may
//! change: lower, UPPER, or Title.

use std::path::{Path, PathBuf};

use regex::{NoExpand, Regex, RegexBuilder};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Case {
    #[default]
    AsIs,
    Lower,
    Upper,
    Title,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    pub find: String,
    pub replace: String,
    pub regex: bool,
    pub case: Case,
}

/// Something to rename.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub path: PathBuf,
    pub is_dir: bool,
    /// When it was changed, as `2026-11-18`, if known.
    pub date: Option<String>,
}

/// Why a new name won't do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Empty,
    /// It has a `/`, or is `.` or `..`.
    NotAName,
    /// Two would have it.
    Twice,
    /// Something not being renamed has it already.
    Taken,
}

impl Problem {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::NotAName => "not a name",
            Self::Twice => "twice",
            Self::Taken => "taken",
        }
    }
}

/// One item's new name, and what's wrong with it, if anything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub from: PathBuf,
    pub name: String,
    pub problem: Option<Problem>,
}

impl Planned {
    pub fn old_name(&self) -> String {
        self.from
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    pub fn changes(&self) -> bool {
        self.old_name() != self.name
    }

    pub fn to(&self) -> PathBuf {
        self.from.with_file_name(&self.name)
    }
}

/// The name split where the rule works: the part it changes, and the
/// extension (with its dot) it leaves.
fn split(item: &Item) -> (String, String) {
    let name = item
        .path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if item.is_dir {
        return (name, String::new());
    }
    match name.rfind('.') {
        Some(at) if at > 0 => (name[..at].to_owned(), name[at..].to_owned()),
        _ => (name, String::new()),
    }
}

/// What `rule` makes of each item's name, in order. An unreadable
/// regular expression is an error, in words.
pub fn plan(items: &[Item], rule: &Rule) -> Result<Vec<Planned>, String> {
    let finder = finder(rule)?;
    let mut planned: Vec<Planned> = items
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let (stem, ext) = split(item);
            let tokens = |text: &str| expand(text, index + 1, &stem, &ext, item);
            let changed = match &finder {
                None if rule.replace.is_empty() => stem.clone(),
                None => tokens(&rule.replace),
                Some(finder) => {
                    let replacement = tokens(&rule.replace);
                    if rule.regex {
                        finder.replace_all(&stem, replacement.as_str()).into_owned()
                    } else {
                        finder
                            .replace_all(&stem, NoExpand(&replacement))
                            .into_owned()
                    }
                }
            };
            Planned {
                from: item.path.clone(),
                name: format!("{}{ext}", recase(&changed, rule.case)),
                problem: None,
            }
        })
        .collect();
    check(&mut planned);
    Ok(planned)
}

/// Mark what won't do: empty, not a name, twice, taken.
pub fn check(planned: &mut [Planned]) {
    let renamed: std::collections::HashSet<PathBuf> =
        planned.iter().map(|item| item.from.clone()).collect();
    let mut seen = std::collections::HashMap::<PathBuf, usize>::new();
    for item in planned.iter() {
        *seen.entry(item.to()).or_default() += 1;
    }
    for item in planned.iter_mut() {
        item.problem = if item.name.trim().is_empty() {
            Some(Problem::Empty)
        } else if item.name.contains('/') || item.name == "." || item.name == ".." {
            Some(Problem::NotAName)
        } else if seen.get(&item.to()).copied().unwrap_or(0) > 1 {
            Some(Problem::Twice)
        } else if item.changes() && !renamed.contains(&item.to()) && exists(&item.to()) {
            Some(Problem::Taken)
        } else {
            None
        };
    }
}

fn exists(path: &Path) -> bool {
    path.exists() || path.is_symlink()
}

fn finder(rule: &Rule) -> Result<Option<Regex>, String> {
    if rule.find.is_empty() {
        return Ok(None);
    }
    let pattern = if rule.regex {
        rule.find.clone()
    } else {
        regex::escape(&rule.find)
    };
    RegexBuilder::new(&pattern)
        .size_limit(1 << 20)
        .build()
        .map(Some)
        .map_err(|err| match err {
            regex::Error::Syntax(_) => "That pattern doesn't read".to_owned(),
            other => other.to_string(),
        })
}

/// The replacement's `{…}` tokens filled in for item number `n`.
fn expand(text: &str, n: usize, stem: &str, ext: &str, item: &Item) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &after[..close];
        let filled = match token.split_once(':') {
            Some(("n", width)) => width
                .parse::<usize>()
                .ok()
                .map(|width| format!("{n:0width$}", width = width.min(12))),
            None => match token {
                "n" => Some(n.to_string()),
                "name" => Some(stem.to_owned()),
                "ext" => Some(ext.trim_start_matches('.').to_owned()),
                "date" => item.date.clone(),
                "folder" => item
                    .path
                    .parent()
                    .and_then(Path::file_name)
                    .map(|name| name.to_string_lossy().into_owned()),
                _ => None,
            },
            _ => None,
        };
        match filled {
            Some(filled) => out.push_str(&filled),
            // Not a token: left as written.
            None => out.push_str(&rest[open..=open + close + 1]),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

fn recase(text: &str, case: Case) -> String {
    match case {
        Case::AsIs => text.to_owned(),
        Case::Lower => text.to_lowercase(),
        Case::Upper => text.to_uppercase(),
        Case::Title => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for c in text.chars() {
                if start && c.is_alphanumeric() {
                    out.extend(c.to_uppercase());
                    start = false;
                } else {
                    out.extend(c.to_lowercase());
                    if !c.is_alphanumeric() && c != '\'' {
                        start = true;
                    }
                }
            }
            out
        }
    }
}

/// Names from an editor: one a line, as many lines as items, in order.
pub fn from_lines(items: &[Item], text: &str) -> Result<Vec<Planned>, String> {
    let lines: Vec<&str> = text.lines().collect();
    // A trailing empty line or two is the editor's, not a name.
    let lines: Vec<&str> = {
        let mut lines = lines;
        while lines.len() > items.len() && lines.last().is_some_and(|line| line.trim().is_empty()) {
            lines.pop();
        }
        lines
    };
    if lines.len() != items.len() {
        return Err(format!(
            "{} names for {} items: keep one line for each",
            lines.len(),
            items.len()
        ));
    }
    let mut planned: Vec<Planned> = items
        .iter()
        .zip(lines)
        .map(|(item, line)| Planned {
            from: item.path.clone(),
            name: line.to_owned(),
            problem: None,
        })
        .collect();
    check(&mut planned);
    Ok(planned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(names: &[&str]) -> Vec<Item> {
        names
            .iter()
            .map(|name| Item {
                path: PathBuf::from("/nonexistent/bulk").join(name),
                is_dir: !name.contains('.'),
                date: Some("2026-11-18".into()),
            })
            .collect()
    }

    fn names(planned: &[Planned]) -> Vec<String> {
        planned.iter().map(|item| item.name.clone()).collect()
    }

    #[test]
    fn templates_number_and_keep_extensions() {
        let rule = Rule {
            replace: "Holiday {n:02} {date}".into(),
            ..Rule::default()
        };
        let planned = plan(&items(&["IMG_4031.JPG", "IMG_4032.jpg", "notes"]), &rule).unwrap();
        assert_eq!(
            names(&planned),
            [
                "Holiday 01 2026-11-18.JPG",
                "Holiday 02 2026-11-18.jpg",
                "Holiday 03 2026-11-18"
            ]
        );
        assert!(planned.iter().all(|item| item.problem.is_none()));
    }

    #[test]
    fn find_and_replace_literally_or_by_pattern() {
        let rule = Rule {
            find: "IMG_".into(),
            replace: "photo-".into(),
            ..Rule::default()
        };
        assert_eq!(
            names(&plan(&items(&["IMG_1.png"]), &rule).unwrap()),
            ["photo-1.png"]
        );
        // Literal means literal: no groups, no special characters.
        let rule = Rule {
            find: "(1)".into(),
            replace: "$1".into(),
            ..Rule::default()
        };
        assert_eq!(
            names(&plan(&items(&["a (1).txt"]), &rule).unwrap()),
            ["a $1.txt"]
        );
        let rule = Rule {
            find: r"(\d+)-(\d+)".into(),
            replace: "$2-$1".into(),
            regex: true,
            case: Case::Upper,
        };
        assert_eq!(
            names(&plan(&items(&["scan 11-24.pdf"]), &rule).unwrap()),
            ["SCAN 24-11.pdf"]
        );
        assert!(
            plan(
                &items(&["a"]),
                &Rule {
                    find: "(".into(),
                    regex: true,
                    ..Rule::default()
                }
            )
            .is_err()
        );
    }

    #[test]
    fn problems_are_named() {
        let rule = Rule {
            replace: "same".into(),
            ..Rule::default()
        };
        let planned = plan(&items(&["a.txt", "b.txt", "c.md"]), &rule).unwrap();
        let problems: Vec<_> = planned.iter().map(|item| item.problem).collect();
        assert_eq!(problems, [Some(Problem::Twice), Some(Problem::Twice), None]);
        let rule = Rule {
            replace: "x/y".into(),
            ..Rule::default()
        };
        assert_eq!(
            plan(&items(&["a"]), &rule).unwrap()[0].problem,
            Some(Problem::NotAName)
        );
        assert_eq!(
            recase("the cat's hat-trick", Case::Title),
            "The Cat's Hat-Trick"
        );
    }

    #[test]
    fn names_from_an_editor() {
        let list = items(&["a.txt", "b.txt"]);
        let planned = from_lines(&list, "first.txt\nsecond.txt\n\n").unwrap();
        assert_eq!(names(&planned), ["first.txt", "second.txt"]);
        assert!(from_lines(&list, "only one\n").is_err());
    }
}
