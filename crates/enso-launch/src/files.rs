//! File search: an index of every file under the configured roots, and a
//! matcher fast enough to run over all of it on each keystroke.
//!
//! The index is one string — every path on its own line, directories ending
//! in `/` — so reading it back from the cache is a single read with nothing
//! to parse, and searching it is a linear pass that the CPU prefetches well.
//! [`walk`] fills it with a pool of threads; [`FileIndex::search`] splits the
//! pass over the same number.
//!
//! Like the application index, the saved copy (`~/.cache/enso/files`) only
//! answers until a fresh walk finishes, so it needs no invalidation.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::{self, JoinHandle},
};

use crate::ignores::{self, Rules, glob};

/// First line of the cache file; bump when the format changes.
const HEADER: &str = "enso-launch files 1\n";
/// Shortest query worth running over every file.
pub const MIN_QUERY: usize = 2;
/// Stop indexing here: a root that large is a mistake, not a home directory.
const MAX_ENTRIES: usize = 2_000_000;
/// Below this many bytes one thread searches faster than several start.
const PARALLEL_FROM: usize = 1 << 20;

/// Files whose rules are honoured below the directory they are found in:
/// Git's, and the plain `.ignore` that search tools share.
const IGNORE_FILES: [&str; 2] = [".gitignore", ".ignore"];

/// What to index, from the `[launch]` config table.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Scope {
    /// Directories to index, absolute.
    pub roots: Vec<PathBuf>,
    /// Include dotfiles and dot-directories.
    pub hidden: bool,
    /// The user's ignore list.
    pub ignore: Vec<Pattern>,
    /// Honour `.gitignore` and `.ignore` files.
    pub gitignore: bool,
    /// Leave out what the built-in list names.
    pub defaults: bool,
    /// The home directory. Its own `.gitignore` is not honoured: where
    /// there is one it belongs to a dotfiles repository and typically
    /// ignores everything.
    home: String,
}

/// Why an entry is in or out, most decisive first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    Keep,
    Skip,
}

impl Scope {
    /// Resolve the configured strings; `~` is `home`. The built-in list
    /// and ignore files are on.
    pub fn new(roots: &[String], hidden: bool, ignore: &[String], home: &Path) -> Self {
        Self {
            roots: roots
                .iter()
                .map(|root| expand(root, home))
                .filter(|root| root.is_absolute())
                .collect(),
            hidden,
            ignore: ignore
                .iter()
                .filter(|pattern| !pattern.trim().is_empty())
                .map(|pattern| Pattern::new(pattern, home))
                .collect(),
            gitignore: true,
            defaults: true,
            home: home.to_string_lossy().trim_end_matches('/').to_owned(),
        }
    }

    /// Whether the entry `name` of `dir` is indexed. `rules` are the ignore
    /// files in force there and `siblings` everything else in `dir`.
    ///
    /// In order: version-control internals never are; then the user's list
    /// has the say, either way; then dotfiles go unless asked for; then the
    /// ignore files decide, either way; and what is left is checked against
    /// the built-in list.
    fn verdict(
        &self,
        dir: &str,
        name: &str,
        is_dir: bool,
        rules: Option<&Rules>,
        siblings: &[(String, bool)],
    ) -> Verdict {
        if ignores::VCS.contains(&name) {
            return Verdict::Skip;
        }
        // As in an ignore file, the last entry that matches decides.
        if let Some(pattern) = self
            .ignore
            .iter()
            .rev()
            .find(|pattern| pattern.matches(dir, name))
        {
            return if pattern.keep {
                Verdict::Keep
            } else {
                Verdict::Skip
            };
        }
        if !self.hidden && name.starts_with('.') {
            return Verdict::Skip;
        }
        match rules.and_then(|rules| rules.verdict(dir, name, is_dir)) {
            Some(true) => return Verdict::Skip,
            Some(false) => return Verdict::Keep,
            None => {}
        }
        if self.defaults && ignores::is_junk(name, || siblings) {
            return Verdict::Skip;
        }
        Verdict::Keep
    }
}

fn expand(path: &str, home: &Path) -> PathBuf {
    let path = path.trim();
    match path.strip_prefix('~') {
        Some("") => home.to_owned(),
        Some(rest) if rest.starts_with('/') => home.join(rest.trim_start_matches('/')),
        _ => PathBuf::from(path),
    }
}

/// One entry of the ignore list.
///
/// - `*.iso`, `Downloads` — no slash: any file or directory with that name.
/// - `Music/incoming` — a relative path: anything whose path ends that way.
/// - `~/VMs`, `/mnt/backup` — an absolute path: exactly that one.
/// - `!target`, `!~/dev/tool/build` — the same, kept rather than left out:
///   the way to get back something an ignore file or the built-in list drops.
///
/// `*`, `?`, and `[a-z]` match within one name; a trailing slash is allowed.
/// Where several entries match, the last one decides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pattern {
    /// `!pattern`: what matches is kept.
    keep: bool,
    /// Must match from the filesystem root rather than any tail of the path.
    anchored: bool,
    /// One glob per path component, outermost first.
    parts: Vec<String>,
}

impl Pattern {
    pub fn new(pattern: &str, home: &Path) -> Self {
        let pattern = pattern.trim();
        let keep = pattern.starts_with('!');
        let pattern = pattern.trim_start_matches('!').trim_start();
        let expanded = if pattern == "~" || pattern.starts_with("~/") {
            expand(pattern, home).to_string_lossy().into_owned()
        } else {
            pattern.to_owned()
        };
        Self {
            keep,
            anchored: expanded.starts_with('/'),
            parts: expanded
                .split('/')
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }

    fn matches(&self, parent: &str, name: &str) -> bool {
        let Some((last, outer)) = self.parts.split_last() else {
            return false;
        };
        if !glob(last, name) {
            return false;
        }
        let mut above = parent.split('/').filter(|part| !part.is_empty()).rev();
        let outer_match = outer
            .iter()
            .rev()
            .all(|part| above.next().is_some_and(|dir| glob(part, dir)));
        outer_match && (!self.anchored || above.next().is_none())
    }
}

/// Every indexed path, one per line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileIndex {
    text: String,
}

/// A search result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// Absolute path, without a trailing slash.
    pub path: String,
    pub is_dir: bool,
    pub score: i32,
}

impl Hit {
    /// The last component: what the row is titled.
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }

    /// The containing directory, with the home directory written as `~`.
    pub fn location(&self, home: &str) -> String {
        let parent = match self.path.rfind('/') {
            Some(0) | None => "/",
            Some(cut) => &self.path[..cut],
        };
        match parent.strip_prefix(home) {
            Some("") if !home.is_empty() => "~".to_owned(),
            Some(rest) if !home.is_empty() && rest.starts_with('/') => format!("~{rest}"),
            _ => parent.to_owned(),
        }
    }
}

impl FileIndex {
    #[cfg(test)]
    pub fn from_paths<'a>(paths: impl IntoIterator<Item = &'a str>) -> Self {
        let mut text = String::new();
        for path in paths {
            text.push_str(path);
            text.push('\n');
        }
        Self { text }
    }

    /// Number of indexed files and directories.
    pub fn len(&self) -> usize {
        self.text.bytes().filter(|byte| *byte == b'\n').count()
    }

    /// The saved index, if there is a usable one.
    pub fn read(path: &Path) -> Option<Self> {
        let mut text = fs::read_to_string(path).ok()?;
        if !text.starts_with(HEADER) || !text.ends_with('\n') {
            return None;
        }
        text.drain(..HEADER.len());
        Some(Self { text })
    }

    /// Save the index atomically, like the application index.
    pub fn write(&self, path: &Path) -> io::Result<()> {
        let dir = path
            .parent()
            .ok_or_else(|| io::Error::other("index path has no directory"))?;
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".files.{}.tmp", std::process::id()));
        let mut contents = String::with_capacity(HEADER.len() + self.text.len());
        contents.push_str(HEADER);
        contents.push_str(&self.text);
        fs::write(&tmp, contents)?;
        fs::rename(&tmp, path).inspect_err(|_| {
            let _ = fs::remove_file(&tmp);
        })
    }

    /// The best `limit` matches for `query`, best first. Queries shorter
    /// than [`MIN_QUERY`] match nothing: one letter matches nearly every
    /// path there is, and none of them usefully.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let query = Query::new(query);
        if query.needle.len() < MIN_QUERY || limit == 0 {
            return Vec::new();
        }
        let threads = if self.text.len() < PARALLEL_FROM {
            1
        } else {
            workers()
        };
        let mut best: Vec<(i32, &str)> = if threads == 1 {
            search_chunk(&self.text, &query, limit)
        } else {
            let chunks = self.chunks(threads);
            thread::scope(|scope| {
                let handles: Vec<_> = chunks
                    .into_iter()
                    .map(|chunk| {
                        let query = &query;
                        scope.spawn(move || search_chunk(chunk, query, limit))
                    })
                    .collect();
                handles
                    .into_iter()
                    .flat_map(|handle| handle.join().unwrap_or_default())
                    .collect()
            })
        };
        keep_best(&mut best, limit);
        best.into_iter()
            .map(|(score, line)| {
                let path = line.strip_suffix('/').unwrap_or(line);
                Hit {
                    path: path.to_owned(),
                    is_dir: path.len() != line.len(),
                    score,
                }
            })
            .collect()
    }

    /// Split the text into about `count` pieces, each ending on a line end.
    fn chunks(&self, count: usize) -> Vec<&str> {
        let bytes = self.text.as_bytes();
        let step = (bytes.len() / count.max(1)).max(1);
        let mut chunks = Vec::with_capacity(count);
        let mut start = 0;
        while start < bytes.len() {
            let target = (start + step).min(bytes.len());
            let end = bytes[target..]
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(bytes.len(), |offset| target + offset + 1);
            // A line end is always a character boundary.
            chunks.push(&self.text[start..end]);
            start = end;
        }
        chunks
    }
}

fn workers() -> usize {
    thread::available_parallelism().map_or(2, |n| n.get().min(8))
}

fn search_chunk<'a>(chunk: &'a str, query: &Query, limit: usize) -> Vec<(i32, &'a str)> {
    let mut found: Vec<(i32, &str)> = Vec::new();
    for line in chunk.split_terminator('\n') {
        if let Some(score) = query.score(line) {
            found.push((score, line));
            // Keep the pile small without sorting on every match.
            if found.len() >= limit.max(64) * 8 {
                keep_best(&mut found, limit);
            }
        }
    }
    keep_best(&mut found, limit);
    found
}

/// Sort best first and cut to `limit`. Ties go to the shorter path, then
/// alphabetically, so the order never depends on how the index was walked.
///
/// The best `limit` are picked out first (a selection: linear time), and
/// only they are sorted — the pile is many times larger than what is kept.
fn keep_best(found: &mut Vec<(i32, &str)>, limit: usize) {
    let order = |a: &(i32, &str), b: &(i32, &str)| {
        b.0.cmp(&a.0)
            .then(a.1.len().cmp(&b.1.len()))
            .then(a.1.cmp(b.1))
    };
    if limit == 0 {
        found.clear();
        return;
    }
    if found.len() > limit {
        found.select_nth_unstable_by(limit - 1, order);
        found.truncate(limit);
    }
    found.sort_unstable_by(order);
}

/// A query prepared once and scored against every path.
#[derive(Debug)]
struct Query {
    /// Lowercase, without whitespace.
    needle: Vec<u8>,
}

impl Query {
    fn new(query: &str) -> Self {
        Self {
            needle: query
                .chars()
                .filter(|c| !c.is_whitespace())
                .flat_map(char::to_lowercase)
                .collect::<String>()
                .into_bytes(),
        }
    }

    /// Score one line of the index, or `None` if it does not contain the
    /// query's characters in order.
    ///
    /// The match is pulled as far right as it will go, because the end of a
    /// path — the file's own name — is what people type. Matching ignores
    /// ASCII case.
    fn score(&self, line: &str) -> Option<i32> {
        let path = line.as_bytes();
        let path = path.strip_suffix(b"/").unwrap_or(path);
        let needle = &self.needle;

        // Backward: is it a match at all, and where does the rightmost one
        // begin? This is the pass every path pays for, so it does no more.
        let mut left = needle.len();
        let mut start = 0;
        for (index, byte) in path.iter().enumerate().rev() {
            if byte.to_ascii_lowercase() == needle[left - 1] {
                left -= 1;
                if left == 0 {
                    start = index;
                    break;
                }
            }
        }
        if left > 0 {
            return None;
        }

        // Forward from there: the tightest match with that beginning.
        let name_start = path
            .iter()
            .rposition(|byte| *byte == b'/')
            .map_or(0, |slash| slash + 1);
        let mut score = 0;
        let mut next = 0;
        let mut previous_matched = false;
        let mut end = start;
        for (index, byte) in path.iter().enumerate().skip(start) {
            if next == needle.len() {
                break;
            }
            let matched = byte.to_ascii_lowercase() == needle[next];
            if matched {
                score += 10;
                let before = index.checked_sub(1).map(|i| path[i]);
                if before.is_none_or(|b| !b.is_ascii_alphanumeric())
                    || before.is_some_and(|b| b.is_ascii_lowercase() && byte.is_ascii_uppercase())
                {
                    score += 30;
                }
                // An unbroken run counts for a little more than a fresh
                // word start, so "main" prefers `main.rs` to `mai_NP`.
                if previous_matched {
                    score += 32;
                }
                if index >= name_start {
                    score += 15;
                }
                next += 1;
                end = index;
            }
            previous_matched = matched;
        }

        // A match wholly inside the name, and a name that is little more
        // than the match, are what was meant.
        if start >= name_start {
            score += 40;
            let extra = (path.len() - name_start).saturating_sub(needle.len());
            score -= extra.min(40) as i32;
        }
        // Characters skipped inside the match.
        let gaps = (end + 1 - start).saturating_sub(needle.len());
        score -= gaps.min(10) as i32 * 3;
        let depth = path.iter().filter(|byte| **byte == b'/').count();
        score -= (path.len() - 1 - end).min(30) as i32 / 2;
        score -= depth.min(20) as i32 * 2;
        Some(score)
    }
}

/// Index everything under the scope's roots.
pub fn walk(scope: &Scope) -> FileIndex {
    let roots: Vec<String> = scope
        .roots
        .iter()
        .filter_map(|root| root.to_str())
        .map(|root| match root.trim_end_matches('/') {
            "" => "/".to_owned(),
            trimmed => trimmed.to_owned(),
        })
        .collect();
    // A root inside another is reached from it, unless something on the way
    // is ignored (`~/.config` under `~`): then it is walked on its own.
    let queue = Mutex::new(Queue {
        dirs: roots.into_iter().map(|root| (root, None)).collect(),
        busy: 0,
    });
    let state = Walk {
        scope,
        queue,
        wake: Condvar::new(),
        entries: AtomicUsize::new(0),
    };
    let parts: Vec<String> = thread::scope(|threads| {
        let handles: Vec<_> = (0..workers())
            .map(|_| threads.spawn(|| state.work()))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_default())
            .collect()
    });
    let mut text = parts.concat();
    if scope.roots.len() > 1 {
        text = dedup_lines(&text);
    }
    FileIndex { text }
}

/// Overlapping roots list some paths twice; keep one of each.
fn dedup_lines(text: &str) -> String {
    let mut lines: Vec<&str> = text.split_terminator('\n').collect();
    lines.sort_unstable();
    lines.dedup();
    let mut out = String::with_capacity(text.len());
    for line in lines {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// A directory to read, and the ignore rules in force for it.
type Pending = (String, Option<Arc<Rules>>);

struct Queue {
    /// Directories waiting to be read.
    dirs: Vec<Pending>,
    /// Workers currently reading one, which may yet add more.
    busy: usize,
}

struct Walk<'a> {
    scope: &'a Scope,
    queue: Mutex<Queue>,
    wake: Condvar,
    entries: AtomicUsize,
}

impl Walk<'_> {
    /// Read directories until none are left anywhere; return what was found.
    fn work(&self) -> String {
        let mut found = String::new();
        let mut subdirs = Vec::new();
        loop {
            let (dir, rules) = {
                let Ok(mut queue) = self.queue.lock() else {
                    return found;
                };
                loop {
                    if let Some(dir) = queue.dirs.pop() {
                        queue.busy += 1;
                        break dir;
                    }
                    if queue.busy == 0 {
                        self.wake.notify_all();
                        return found;
                    }
                    queue = match self.wake.wait(queue) {
                        Ok(queue) => queue,
                        Err(_) => return found,
                    };
                }
            };

            if self.entries.load(Ordering::Relaxed) < MAX_ENTRIES {
                self.read(&dir, rules, &mut found, &mut subdirs);
            }

            let Ok(mut queue) = self.queue.lock() else {
                return found;
            };
            queue.dirs.append(&mut subdirs);
            queue.busy -= 1;
            self.wake.notify_all();
        }
    }

    fn read(
        &self,
        dir: &str,
        mut rules: Option<Arc<Rules>>,
        found: &mut String,
        subdirs: &mut Vec<Pending>,
    ) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        // The whole listing first: what a directory holds decides what in
        // it is worth keeping. The type comes from the directory entry
        // itself — no extra system call, and a link to a directory is
        // listed but never followed.
        let listing: Vec<(String, bool)> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                // Names that are not text cannot be typed into a search field.
                let name = entry.file_name().into_string().ok()?;
                let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
                (!name.contains('\n')).then_some((name, is_dir))
            })
            .collect();
        let has = |wanted: &str| listing.iter().any(|(name, _)| name == wanted);
        // Nothing in here is worth finding, by its own account.
        if ignores::TELLTALES.iter().any(|telltale| has(telltale)) {
            return;
        }
        if self.scope.gitignore && dir != self.scope.home {
            for file in IGNORE_FILES.into_iter().filter(|file| has(file)) {
                if let Ok(text) = fs::read_to_string(format!("{dir}/{file}")) {
                    rules = Rules::layer(&text, dir, rules);
                }
            }
        }

        let prefix = if dir == "/" { "" } else { dir };
        let mut count = 0;
        for (name, is_dir) in &listing {
            let verdict = self
                .scope
                .verdict(dir, name, *is_dir, rules.as_deref(), &listing);
            if verdict == Verdict::Skip {
                continue;
            }
            found.push_str(prefix);
            found.push('/');
            found.push_str(name);
            if *is_dir {
                found.push('/');
                subdirs.push((format!("{prefix}/{name}"), rules.clone()));
            }
            found.push('\n');
            count += 1;
        }
        self.entries.fetch_add(count, Ordering::Relaxed);
    }
}

/// What the search thread is told.
#[derive(Debug)]
pub enum Job {
    /// Search for this; `generation` comes back with the answer.
    Query { generation: u64, query: String },
    /// A newer index to search from now on.
    Index(FileIndex),
}

/// Start file search in the background.
///
/// One thread loads the saved index, then walks the disk and saves the
/// result; its handle is returned so the process can let that finish after
/// the window has closed. Another answers queries, always the latest one
/// asked, through `answer`.
pub fn serve(
    scope: Scope,
    cache: PathBuf,
    limit: usize,
    answer: impl Fn(u64, Vec<Hit>) + Send + 'static,
) -> io::Result<(Sender<Job>, JoinHandle<()>)> {
    let (jobs, inbox) = mpsc::channel();
    thread::Builder::new()
        .name("enso-search".to_owned())
        .spawn(move || search_loop(&inbox, limit, &answer))?;

    let indexed = jobs.clone();
    let scan = thread::Builder::new()
        .name("enso-files".to_owned())
        .spawn(move || {
            let saved = FileIndex::read(&cache);
            if let Some(index) = &saved {
                let _ = indexed.send(Job::Index(index.clone()));
            }
            let fresh = walk(&scope);
            if saved.as_ref() != Some(&fresh) {
                if let Err(err) = fresh.write(&cache) {
                    eprintln!("enso-launch: cannot save the file index: {err}");
                }
                let _ = indexed.send(Job::Index(fresh));
            }
        })?;
    Ok((jobs, scan))
}

fn search_loop(inbox: &Receiver<Job>, limit: usize, answer: &impl Fn(u64, Vec<Hit>)) {
    let mut index: Option<FileIndex> = None;
    let mut asked: Option<(u64, String)> = None;
    while let Ok(job) = inbox.recv() {
        // Only the newest query matters; skip the ones typed past.
        for job in std::iter::once(job).chain(inbox.try_iter()) {
            match job {
                Job::Query { generation, query } => asked = Some((generation, query)),
                Job::Index(newer) => index = Some(newer),
            }
        }
        if let (Some(index), Some((generation, query))) = (&index, &asked) {
            answer(*generation, index.search(query, limit));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> &'static Path {
        Path::new("/home/mira")
    }

    fn paths(hits: &[Hit]) -> Vec<&str> {
        hits.iter().map(|hit| hit.path.as_str()).collect()
    }

    #[test]
    fn ignore_patterns() {
        let pattern = |text: &str| Pattern::new(text, home());
        // A bare name matches at any depth.
        assert!(pattern("build").matches("/home/mira/dev/app", "build"));
        assert!(pattern("*.o").matches("/home/mira/dev/app", "main.o"));
        assert!(pattern("build/").matches("/home/mira/dev/app", "build"));
        // A relative path matches the tail.
        assert!(pattern("Music/incoming").matches("/home/mira/Music", "incoming"));
        assert!(!pattern("Music/incoming").matches("/home/mira/Videos", "incoming"));
        assert!(pattern("dev/*/dist").matches("/home/mira/dev/app", "dist"));
        // An absolute one matches exactly that path.
        assert!(pattern("~/VMs").matches("/home/mira", "VMs"));
        assert!(!pattern("~/VMs").matches("/home/mira/old", "VMs"));
        assert!(!pattern("/mnt/backup").matches("/home/mnt", "backup"));
        assert!(pattern("/mnt/backup").matches("/mnt", "backup"));
        assert!(!pattern("").matches("/home/mira", "anything"));
        // A leading `!` marks what to keep; it matches the same way.
        assert!(pattern("!target").keep && pattern("!target").matches("/home/mira/dev", "target"));
        assert!(pattern("! ~/VMs").keep && pattern("! ~/VMs").matches("/home/mira", "VMs"));
        assert!(!pattern("target").keep);
    }

    /// A tree under a fresh temporary directory, and a way to list what a
    /// scope finds in it, as sorted paths relative to it.
    fn tree(name: &str, dirs: &[&str], files: &[(&str, &str)]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("enso-launch-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for dir in dirs {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (file, contents) in files {
            let path = root.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
        root
    }

    fn listed(root: &Path, scope: &Scope) -> Vec<String> {
        let mut lines: Vec<String> = walk(scope)
            .text
            .lines()
            .map(|line| {
                line.strip_prefix(root.to_str().unwrap())
                    .unwrap()
                    .to_owned()
            })
            .collect();
        lines.sort();
        lines
    }

    fn scope_of(root: &Path, ignore: &[&str]) -> Scope {
        let ignore: Vec<String> = ignore.iter().map(|pattern| (*pattern).to_owned()).collect();
        // Home is elsewhere, so the root's own ignore file counts.
        Scope::new(
            &[root.to_str().unwrap().to_owned()],
            false,
            &ignore,
            Path::new("/home/nobody"),
        )
    }

    #[test]
    fn build_output_is_left_out_without_any_ignore_file() {
        let root = tree(
            "defaults",
            &["scripts/bin", "photos/build"],
            &[
                ("app/Cargo.toml", ""),
                ("app/src/main.rs", ""),
                ("app/target/debug/app", ""),
                ("site/package.json", ""),
                ("site/index.js", ""),
                ("site/dist/bundle.js", ""),
                ("site/node_modules/left-pad/index.js", ""),
                ("site/npm_cache/x", ""),
                ("tool/tool.csproj", ""),
                ("tool/bin/tool.dll", ""),
                ("tool/obj/x", ""),
                ("py/venv-odd-name/pyvenv.cfg", ""),
                ("py/venv-odd-name/lib/x.py", ""),
                ("py/__pycache__/x.pyc", ""),
                ("py/mod.py", ""),
                ("py/mod.pyc", ""),
                ("c/main.c", ""),
                ("c/main.o", ""),
                ("c/notes.txt~", ""),
                ("scripts/bin/backup.sh", ""),
                ("photos/build/day1.jpg", ""),
                ("cache/blob", ""),
            ],
        );
        let lines = listed(&root, &scope_of(&root, &[]));
        assert_eq!(
            lines,
            [
                "/app/",
                "/app/Cargo.toml",
                "/app/src/",
                "/app/src/main.rs",
                "/c/",
                "/c/main.c",
                "/photos/",
                // `build` and `bin` are only output beside a project file.
                "/photos/build/",
                "/photos/build/day1.jpg",
                "/py/",
                "/py/mod.py",
                // The environment is still there to be found; its insides are not.
                "/py/venv-odd-name/",
                "/scripts/",
                "/scripts/bin/",
                "/scripts/bin/backup.sh",
                "/site/",
                "/site/index.js",
                "/site/package.json",
                "/tool/",
                "/tool/tool.csproj",
            ]
        );

        // The built-in list can be switched off…
        let mut everything = scope_of(&root, &[]);
        everything.defaults = false;
        let lines = listed(&root, &everything);
        assert!(lines.contains(&"/app/target/debug/app".to_owned()));
        assert!(lines.contains(&"/site/node_modules/left-pad/index.js".to_owned()));
        assert!(lines.contains(&"/c/main.o".to_owned()));
        // …or overruled an entry at a time, and added to.
        let custom = scope_of(&root, &["!target", "photos", "*.js", "!index.js"]);
        let lines = listed(&root, &custom);
        assert!(lines.contains(&"/app/target/debug/app".to_owned()));
        assert!(!lines.iter().any(|line| line.starts_with("/photos")));
        assert!(lines.contains(&"/site/index.js".to_owned()));
        assert!(!lines.iter().any(|line| line.contains("node_modules")));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ignore_files_are_honoured_without_a_repository() {
        let root = tree(
            "gitignore",
            &[],
            &[
                // No `.git` anywhere.
                (
                    "proj/.gitignore",
                    "*.log\n/out\ndocs/_site/\n!keep.log\nnode_modules\n",
                ),
                ("proj/debug.log", ""),
                ("proj/keep.log", ""),
                ("proj/main.c", ""),
                ("proj/out/a.bin", ""),
                ("proj/src/out/kept.txt", ""),
                ("proj/src/trace.log", ""),
                ("proj/docs/_site/index.html", ""),
                ("proj/docs/guide.md", ""),
                // A nested file adds its own rules and can undo the parent's.
                ("proj/src/sub/.gitignore", "!trace.log\nsecret*\n"),
                ("proj/src/sub/trace.log", ""),
                ("proj/src/sub/secret.txt", ""),
                // `.ignore` works the same way.
                ("media/.ignore", "raw/\n"),
                ("media/raw/a.cr3", ""),
                ("media/final.jpg", ""),
                // Rules stop at their own tree.
                ("other/debug.log", ""),
            ],
        );
        let scope = scope_of(&root, &[]);
        assert_eq!(
            listed(&root, &scope),
            [
                "/media/",
                "/media/final.jpg",
                "/other/",
                "/other/debug.log",
                "/proj/",
                "/proj/docs/",
                "/proj/docs/guide.md",
                "/proj/keep.log",
                "/proj/main.c",
                "/proj/src/",
                "/proj/src/out/",
                "/proj/src/out/kept.txt",
                "/proj/src/sub/",
                "/proj/src/sub/trace.log",
            ]
        );

        // Switched off, the files are just files (and hidden ones at that).
        let mut plain = scope.clone();
        plain.gitignore = false;
        let lines = listed(&root, &plain);
        assert!(lines.contains(&"/proj/debug.log".to_owned()));
        assert!(lines.contains(&"/media/raw/a.cr3".to_owned()));
        // The user's list has the last word over an ignore file.
        let lines = listed(&root, &scope_of(&root, &["!debug.log", "guide.md"]));
        assert!(lines.contains(&"/proj/debug.log".to_owned()));
        assert!(!lines.contains(&"/proj/docs/guide.md".to_owned()));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_home_directorys_own_gitignore_is_not_honoured() {
        // A dotfiles repository in the home directory ignores everything.
        let root = tree(
            "home",
            &[],
            &[
                (".gitignore", "*\n"),
                ("notes.md", ""),
                ("dev/app/.gitignore", "*.tmp\n"),
                ("dev/app/a.tmp", ""),
                ("dev/app/a.rs", ""),
            ],
        );
        let home = Scope::new(&[root.to_str().unwrap().to_owned()], false, &[], &root);
        assert_eq!(
            listed(&root, &home),
            ["/dev/", "/dev/app/", "/dev/app/a.rs", "/notes.md"]
        );
        // Anywhere else, a file that says "everything" means it.
        assert!(listed(&root, &scope_of(&root, &[])).is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn walks_and_ignores() {
        let root = std::env::temp_dir().join(format!("enso-launch-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        for dir in [
            "notes/2026",
            "dev/app/.git/objects",
            "dev/app/src",
            "dev/app/target/debug",
            "dev/app/node_modules/left-pad",
            ".config/niri",
            "VMs",
            "Music/incoming",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in [
            "notes/2026/tea.md",
            "notes/scratch.tmp",
            "dev/app/.git/HEAD",
            "dev/app/src/main.rs",
            "dev/app/target/CACHEDIR.TAG",
            "dev/app/target/debug/app",
            "dev/app/node_modules/left-pad/index.js",
            ".config/niri/config.kdl",
            ".bashrc",
            "VMs/arch.qcow2",
            "Music/incoming/part.flac",
            "Music/song.flac",
        ] {
            fs::write(root.join(file), "").unwrap();
        }
        let roots = |extra: &[&str]| -> Vec<String> {
            std::iter::once(root.to_str().unwrap().to_owned())
                .chain(extra.iter().map(|dir| format!("{}/{dir}", root.display())))
                .collect()
        };
        let listed = |scope: &Scope| -> Vec<String> {
            let index = walk(scope);
            let mut lines: Vec<String> = index
                .text
                .lines()
                .map(|line| {
                    line.strip_prefix(root.to_str().unwrap())
                        .unwrap()
                        .to_owned()
                })
                .collect();
            lines.sort();
            assert_eq!(lines.len(), index.len());
            lines
        };

        // Defaults: no dotfiles, no dependency or build trees — but the
        // build directory itself is still there to be found.
        let default = Scope::new(&roots(&[]), false, &[], &root);
        assert_eq!(
            listed(&default),
            [
                "/Music/",
                "/Music/incoming/",
                "/Music/incoming/part.flac",
                "/Music/song.flac",
                "/VMs/",
                "/VMs/arch.qcow2",
                "/dev/",
                "/dev/app/",
                "/dev/app/src/",
                "/dev/app/src/main.rs",
                "/dev/app/target/",
                "/notes/",
                "/notes/2026/",
                "/notes/2026/tea.md",
                "/notes/scratch.tmp",
            ]
        );

        // The custom list: a glob, a relative path, and one under `~`.
        let ignore = [
            "*.tmp".to_owned(),
            "Music/incoming".to_owned(),
            "~/VMs".to_owned(),
        ];
        let custom = Scope::new(&roots(&[]), false, &ignore, &root);
        let lines = listed(&custom);
        assert!(!lines.iter().any(|line| line.contains("scratch")
            || line.contains("incoming")
            || line.contains("VMs")));
        assert!(lines.contains(&"/Music/song.flac".to_owned()));

        // Hidden files on request; `.git` stays out regardless.
        let hidden = Scope::new(&roots(&[]), true, &[], &root);
        let lines = listed(&hidden);
        assert!(lines.contains(&"/.config/niri/config.kdl".to_owned()));
        assert!(lines.contains(&"/.bashrc".to_owned()));
        assert!(!lines.iter().any(|line| line.contains(".git")));

        // A hidden directory named as a root is walked; an overlapping root
        // does not list anything twice.
        let extra = Scope::new(&roots(&[".config", "notes"]), false, &[], &root);
        let lines = listed(&extra);
        assert!(lines.contains(&"/.config/niri/config.kdl".to_owned()));
        assert_eq!(
            lines.iter().filter(|line| line.ends_with("tea.md")).count(),
            1
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ranks_names_above_paths() {
        let index = FileIndex::from_paths([
            "/home/mira/dev/enso/crates/enso-bar/src/main.rs",
            "/home/mira/dev/enso/crates/enso-bar/src/",
            "/home/mira/dev/enso/README.md",
            "/home/mira/notes/main-ideas.md",
            "/home/mira/notes/remaining.txt",
            "/home/mira/Pictures/domain.png",
            "/home/mira/main/",
        ]);
        let hits = index.search("main", 10);
        // The directory named exactly that, then names starting with it,
        // then names merely containing it.
        assert_eq!(
            paths(&hits)[..3],
            [
                "/home/mira/main",
                "/home/mira/dev/enso/crates/enso-bar/src/main.rs",
                "/home/mira/notes/main-ideas.md",
            ]
        );
        assert!(hits[0].is_dir && !hits[1].is_dir);
        assert!(paths(&hits).contains(&"/home/mira/Pictures/domain.png"));
        assert!(!paths(&hits).contains(&"/home/mira/dev/enso/README.md"));

        // Words from the path narrow it down; case and spaces do not matter.
        assert_eq!(
            paths(&index.search("Bar Main", 10)),
            ["/home/mira/dev/enso/crates/enso-bar/src/main.rs"]
        );
        assert_eq!(
            paths(&index.search("readme", 1)),
            ["/home/mira/dev/enso/README.md"]
        );
        // Too short to mean anything.
        assert!(index.search("m", 10).is_empty());
        assert!(index.search("  ", 10).is_empty());
        assert!(index.search("zzzz", 10).is_empty());
    }

    #[test]
    fn hits_describe_themselves() {
        let hit = |path: &str| Hit {
            path: path.to_owned(),
            is_dir: false,
            score: 0,
        };
        assert_eq!(hit("/home/mira/notes/tea.md").name(), "tea.md");
        assert_eq!(
            hit("/home/mira/notes/tea.md").location("/home/mira"),
            "~/notes"
        );
        assert_eq!(hit("/home/mira/tea.md").location("/home/mira"), "~");
        assert_eq!(
            hit("/home/miranda/tea.md").location("/home/mira"),
            "/home/miranda"
        );
        assert_eq!(hit("/etc/fstab").location("/home/mira"), "/etc");
        assert_eq!(hit("/vmlinuz").location("/home/mira"), "/");
    }

    #[test]
    fn large_indexes_search_in_parallel_with_the_same_answer() {
        let mut lines = Vec::new();
        for dir in 0..400 {
            for file in 0..100 {
                lines.push(format!(
                    "/home/mira/archive/batch-{dir:03}/item-{file:03}.dat"
                ));
            }
        }
        lines.push("/home/mira/archive/batch-200/needle.txt".to_owned());
        let index = FileIndex::from_paths(lines.iter().map(String::as_str));
        assert!(index.text.len() > PARALLEL_FROM);
        // Chunks cover the text exactly, each ending on a line.
        let chunks = index.chunks(8);
        assert!(chunks.len() > 1 && chunks.iter().all(|chunk| chunk.ends_with('\n')));
        assert_eq!(chunks.concat(), index.text);

        let hits = index.search("needle", 5);
        assert_eq!(paths(&hits), ["/home/mira/archive/batch-200/needle.txt"]);
        // Ties are broken by the path, not by which thread found it.
        let hits = index.search("item 007", 3);
        assert_eq!(
            paths(&hits),
            [
                "/home/mira/archive/batch-000/item-007.dat",
                "/home/mira/archive/batch-001/item-007.dat",
                "/home/mira/archive/batch-002/item-007.dat",
            ]
        );
    }

    #[test]
    fn the_cache_round_trips_and_rejects_strangers() {
        let dir = std::env::temp_dir().join(format!("enso-launch-fcache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("files");
        assert_eq!(FileIndex::read(&path), None);
        let index = FileIndex::from_paths(["/a/b.txt", "/a/c/"]);
        index.write(&path).unwrap();
        assert_eq!(FileIndex::read(&path), Some(index));
        fs::write(&path, "something else\n").unwrap();
        assert_eq!(FileIndex::read(&path), None);
        // A file cut short mid-line is not trusted either.
        fs::write(&path, format!("{HEADER}/a/b.t")).unwrap();
        assert_eq!(FileIndex::read(&path), None);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_service_answers_the_latest_query_and_follows_a_new_index() {
        let root = std::env::temp_dir().join(format!("enso-launch-serve-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("home")).unwrap();
        fs::write(root.join("home/letter.txt"), "").unwrap();
        let scope = Scope::new(&[format!("{}/home", root.display())], false, &[], &root);
        let (answers, inbox) = mpsc::channel();
        let (jobs, scan) = serve(
            scope,
            root.join("cache/files"),
            5,
            move |generation, hits| {
                let _ = answers.send((generation, hits));
            },
        )
        .unwrap();
        scan.join().unwrap();
        assert!(root.join("cache/files").exists());

        jobs.send(Job::Query {
            generation: 7,
            query: "letter".to_owned(),
        })
        .unwrap();
        // Answers for the same query may arrive more than once (once per
        // index); the last is the one that counts.
        let (generation, hits) = inbox.recv().unwrap();
        assert_eq!(generation, 7);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name(), "letter.txt");

        jobs.send(Job::Index(FileIndex::from_paths(["/x/other.txt"])))
            .unwrap();
        let (generation, hits) = inbox.recv().unwrap();
        assert_eq!((generation, hits.len()), (7, 0));
        fs::remove_dir_all(root).unwrap();
    }
}
