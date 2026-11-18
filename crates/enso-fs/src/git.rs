//! Git, through the `git` program: where a repository is, which branch it's
//! on, who commits there, and the few things a file manager does with it —
//! make a repository, switch or start a branch.
//!
//! `git` is run directly (never through a shell), with machine-readable
//! output (`--porcelain`, `-z`), no prompts (`GIT_TERMINAL_PROMPT=0`), and
//! no pager. Finding the repository a folder is in is done without it: a
//! walk up the folders looking for `.git`, so a folder outside any
//! repository costs no process at all.

use std::{
    io,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

/// Whether `git` can be run here.
pub fn available() -> bool {
    git(Path::new("/"), &["--version"]).is_ok_and(|output| output.status.success())
}

/// The top of the repository `dir` is in, if it's in one: the folder that
/// has `.git` (a folder, or a file pointing elsewhere, as in a worktree).
pub fn root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors()
        .find(|folder| folder.join(".git").exists())
        .map(Path::to_owned)
}

/// Where a repository is, at a glance.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    /// The branch checked out; `None` when detached.
    pub branch: Option<String>,
    /// The commit, abbreviated, when detached (or nothing yet, `None`).
    pub commit: Option<String>,
    /// Commits not on the upstream, and on it but not here.
    pub ahead: u32,
    pub behind: u32,
    /// Whether the branch has an upstream at all.
    pub tracking: bool,
}

impl Status {
    /// What the status line says: `main`, `main ↑2 ↓1`, or `a1b2c3d`.
    pub fn label(&self) -> String {
        let mut label = match (&self.branch, &self.commit) {
            (Some(branch), _) => branch.clone(),
            (None, Some(commit)) => commit.chars().take(7).collect(),
            (None, None) => "no commits".to_owned(),
        };
        if self.ahead > 0 {
            label += &format!(" ↑{}", self.ahead);
        }
        if self.behind > 0 {
            label += &format!(" ↓{}", self.behind);
        }
        label
    }
}

/// The branch and how it stands, from `git status --porcelain=v2 --branch`.
pub fn status(root: &Path) -> io::Result<Status> {
    let output = run(
        root,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=no",
            "-z",
        ],
    )?;
    Ok(parse_status(&String::from_utf8_lossy(&output)))
}

fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    for line in text.split(['\0', '\n']) {
        let Some(header) = line.strip_prefix("# branch.") else {
            continue;
        };
        let (key, value) = header.split_once(' ').unwrap_or((header, ""));
        match key {
            "head" if value != "(detached)" => status.branch = Some(value.to_owned()),
            "oid" if value != "(initial)" => status.commit = Some(value.to_owned()),
            "upstream" => status.tracking = true,
            "ab" => {
                for part in value.split_whitespace() {
                    if let Some(n) = part.strip_prefix('+') {
                        status.ahead = n.parse().unwrap_or(0);
                    } else if let Some(n) = part.strip_prefix('-') {
                        status.behind = n.parse().unwrap_or(0);
                    }
                }
            }
            _ => {}
        }
    }
    status
}

/// The local branches, in Git's order.
pub fn branches(root: &Path) -> io::Result<Vec<String>> {
    let output = run(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
    )?;
    Ok(String::from_utf8_lossy(&output)
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect())
}

/// Check out `branch`.
pub fn switch(root: &Path, branch: &str) -> io::Result<()> {
    run(root, &["switch", "--", branch]).map(drop)
}

/// Start `branch` here and check it out.
pub fn create_branch(root: &Path, branch: &str) -> io::Result<()> {
    run(root, &["check-ref-format", "--branch", branch])
        .map_err(|_| io::Error::other(format!("“{branch}” can't be a branch name")))?;
    run(root, &["switch", "--create", branch]).map(drop)
}

/// The branches on the remotes, as `origin/main`, without the remotes'
/// `HEAD` pointers.
pub fn remote_branches(root: &Path) -> io::Result<Vec<String>> {
    let output = run(
        root,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
    )?;
    Ok(String::from_utf8_lossy(&output)
        .lines()
        .filter(|line| !line.is_empty() && !line.ends_with("/HEAD") && line.contains('/'))
        .map(str::to_owned)
        .collect())
}

/// The remotes, by name, and where each fetches from.
pub fn remotes(root: &Path) -> io::Result<Vec<(String, String)>> {
    let output = run(root, &["remote", "-v"])?;
    let mut remotes: Vec<(String, String)> = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        let mut parts = line.split_whitespace();
        if let (Some(name), Some(url), Some("(fetch)")) = (parts.next(), parts.next(), parts.next())
        {
            remotes.push((name.to_owned(), url.to_owned()));
        }
    }
    Ok(remotes)
}

/// How long ago the remotes were last fetched from, if ever.
pub fn since_fetch(root: &Path) -> Option<std::time::Duration> {
    let head = run(root, &["rev-parse", "--git-dir"]).ok()?;
    let git_dir = root.join(String::from_utf8_lossy(&head).trim());
    std::fs::metadata(git_dir.join("FETCH_HEAD"))
        .ok()?
        .modified()
        .ok()?
        .elapsed()
        .ok()
}

/// Bring the remotes' branches up to date (`git fetch --all --prune`).
pub fn fetch(root: &Path) -> io::Result<()> {
    run(root, &["fetch", "--all", "--prune", "--quiet"]).map(drop)
}

/// Bring the branch up to date with its upstream, only by going forward.
pub fn pull(root: &Path) -> io::Result<()> {
    run(root, &["pull", "--ff-only", "--quiet"]).map(drop)
}

/// Send the branch to its upstream — to `origin`, as a new branch there,
/// if it has none yet.
pub fn push(root: &Path, tracking: bool) -> io::Result<()> {
    if tracking {
        run(root, &["push", "--quiet"]).map(drop)
    } else {
        run(
            root,
            &["push", "--quiet", "--set-upstream", "origin", "HEAD"],
        )
        .map(drop)
    }
}

/// Check out a remote's branch (`origin/feature`) as a local one that
/// tracks it.
pub fn track(root: &Path, remote_branch: &str) -> io::Result<()> {
    run(root, &["switch", "--track", "--", remote_branch]).map(drop)
}

/// Copy the repository at `url` into `into`, as a folder of its own name.
/// Returns the folder.
pub fn clone(url: &str, into: &Path) -> io::Result<PathBuf> {
    let name = url
        .trim_end_matches('/')
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("repository")
        .trim_end_matches(".git");
    if name.is_empty() || name == "." || name == ".." {
        return Err(io::Error::other("that address names no repository"));
    }
    let folder = into.join(name);
    if folder.exists() {
        return Err(io::Error::other(format!("there's already a “{name}” here")));
    }
    run(into, &["clone", "--quiet", "--", url, name])?;
    Ok(folder)
}

/// Add the remote `name`, fetching from and pushing to `url`.
pub fn add_remote(root: &Path, name: &str, url: &str) -> io::Result<()> {
    run(root, &["remote", "add", "--", name, url]).map(drop)
}

/// The web page for a remote's address, where it's a forge with one
/// (GitHub, GitLab, Codeberg…): `git@github.com:me/repo.git` and
/// `https://github.com/me/repo.git` both become
/// `https://github.com/me/repo`.
pub fn web_page(url: &str) -> Option<String> {
    let url = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let (host, path) = if let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    {
        let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
        rest.split_once('/')?
    } else if let Some(rest) = url.strip_prefix("ssh://") {
        let rest = rest.split_once('@').map_or(rest, |(_, after)| after);
        let (host, path) = rest.split_once('/')?;
        (host.split(':').next()?, path)
    } else {
        // scp-like: git@host:owner/repo
        let rest = url.split_once('@').map_or(url, |(_, after)| after);
        rest.split_once(':')?
    };
    (!host.is_empty() && path.contains('/')).then(|| format!("https://{host}/{path}"))
}

/// The forge's name for a web page, for "Open on …".
pub fn forge(page: &str) -> &str {
    let host = page
        .trim_start_matches("https://")
        .split('/')
        .next()
        .unwrap_or_default();
    match host {
        "github.com" => "GitHub",
        "gitlab.com" => "GitLab",
        "codeberg.org" => "Codeberg",
        "bitbucket.org" => "Bitbucket",
        _ if host.starts_with("gitlab.") => "GitLab",
        _ => host,
    }
}

/// Make `dir` a repository (`git init`, on `init.defaultBranch`).
pub fn init(dir: &Path) -> io::Result<()> {
    run(dir, &["init", "--quiet"]).map(drop)
}

/// Whose commits these are: everywhere, or in one repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Scope {
    Global,
    Repository(PathBuf),
}

/// A name and an email, as `user.name` and `user.email`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: Option<String>,
    pub email: Option<String>,
}

impl Identity {
    pub fn is_set(&self) -> bool {
        self.name.is_some() && self.email.is_some()
    }

    /// `Anshul <me@example.com>`.
    pub fn label(&self) -> String {
        match (&self.name, &self.email) {
            (Some(name), Some(email)) => format!("{name} <{email}>"),
            (Some(name), None) => name.clone(),
            (None, Some(email)) => format!("<{email}>"),
            (None, None) => String::new(),
        }
    }
}

fn scope_args(scope: &Scope) -> (&Path, &'static str) {
    match scope {
        Scope::Global => (Path::new("/"), "--global"),
        Scope::Repository(root) => (root.as_path(), "--local"),
    }
}

/// The identity set at `scope` alone (a repository's own, not what it
/// inherits).
pub fn identity(scope: &Scope) -> Identity {
    let (dir, flag) = scope_args(scope);
    let get = |key: &str| {
        run(dir, &["config", flag, "--get", key])
            .ok()
            .map(|output| String::from_utf8_lossy(&output).trim().to_owned())
            .filter(|value| !value.is_empty())
    };
    Identity {
        name: get("user.name"),
        email: get("user.email"),
    }
}

/// Set the identity at `scope`.
pub fn set_identity(scope: &Scope, name: &str, email: &str) -> io::Result<()> {
    let (name, email) = (name.trim(), email.trim());
    if name.is_empty() || !email.contains('@') {
        return Err(io::Error::other("a name, and an email with an @, please"));
    }
    let (dir, flag) = scope_args(scope);
    run(dir, &["config", flag, "user.name", name])?;
    run(dir, &["config", flag, "user.email", email]).map(drop)
}

/// Drop a repository's own identity, so it uses the global one again.
pub fn inherit_identity(root: &Path) -> io::Result<()> {
    for key in ["user.name", "user.email"] {
        // Exit code 5: it wasn't set, which is what was wanted.
        match git(root, &["config", "--local", "--unset", key]) {
            Ok(output) if output.status.success() || output.status.code() == Some(5) => {}
            Ok(output) => return Err(failure(&output)),
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn git(dir: &Path, args: &[&str]) -> io::Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
}

/// Run `git` in `dir`; its output, or its complaint as the error.
fn run(dir: &Path, args: &[&str]) -> io::Result<Vec<u8>> {
    let output = git(dir, args)?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(failure(&output))
    }
}

fn failure(output: &Output) -> io::Error {
    let text = String::from_utf8_lossy(&output.stderr);
    let line = text
        .lines()
        .map(|line| {
            line.trim_start_matches("fatal: ")
                .trim_start_matches("error: ")
        })
        .find(|line| !line.trim().is_empty())
        .unwrap_or("git failed");
    io::Error::other(line.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn remotes_have_web_pages() {
        for url in [
            "git@github.com:formalemon/enso.git",
            "https://github.com/formalemon/enso.git",
            "https://token@github.com/formalemon/enso",
            "ssh://git@github.com:22/formalemon/enso.git",
        ] {
            assert_eq!(
                web_page(url).as_deref(),
                Some("https://github.com/formalemon/enso"),
                "{url}"
            );
        }
        assert_eq!(forge("https://github.com/a/b"), "GitHub");
        assert_eq!(forge("https://gitlab.example.org/a/b"), "GitLab");
        assert!(web_page("/srv/git/local.git").is_none());
    }

    #[test]
    fn branches_go_and_come_back_through_a_remote() {
        let root = std::env::temp_dir().join(format!("enso-fs-remote-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("work")).unwrap();
        let work = root.join("work");
        let git = |dir: &Path, args: &[&str]| run(dir, args).unwrap();
        // A bare repository standing in for GitHub.
        git(&root, &["init", "--quiet", "--bare", "server.git"]);
        git(&work, &["init", "--quiet", "--initial-branch=main"]);
        git(
            &work,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@e",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "one",
            ],
        );
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                root.join("server.git").to_str().unwrap(),
            ],
        );
        push(&work, false).unwrap();
        git(&work, &["switch", "--quiet", "-c", "feature"]);
        push(&work, false).unwrap();
        git(&work, &["switch", "--quiet", "main"]);
        git(&work, &["branch", "-D", "feature"]);
        fetch(&work).unwrap();
        assert_eq!(
            remote_branches(&work).unwrap(),
            ["origin/feature", "origin/main"]
        );
        assert!(since_fetch(&work).is_some());
        track(&work, "origin/feature").unwrap();
        assert_eq!(status(&work).unwrap().branch.as_deref(), Some("feature"));
        assert!(status(&work).unwrap().tracking);
        // And a copy of it all, elsewhere.
        let copy = clone(root.join("server.git").to_str().unwrap(), &root).unwrap();
        assert_eq!(copy, root.join("server"));
        assert!(
            clone(root.join("server.git").to_str().unwrap(), &root).is_err(),
            "already there"
        );
        assert_eq!(remotes(&copy).unwrap()[0].0, "origin");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn status_lines_are_read() {
        let text = "# branch.oid 1a2b3c4d5e6f\0# branch.head main\0# branch.upstream origin/main\0# branch.ab +2 -1\0";
        let status = parse_status(text);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert!(status.tracking);
        assert_eq!((status.ahead, status.behind), (2, 1));
        assert_eq!(status.label(), "main ↑2 ↓1");

        let detached = parse_status("# branch.oid 1a2b3c4d5e6f\0# branch.head (detached)\0");
        assert_eq!(detached.label(), "1a2b3c4");
        let fresh = parse_status("# branch.oid (initial)\0# branch.head main\0");
        assert_eq!(fresh.label(), "main");
        assert_eq!(fresh.commit, None);
    }

    #[test]
    fn identities_read_as_people_write_them() {
        let both = Identity {
            name: Some("Anshul".into()),
            email: Some("me@example.com".into()),
        };
        assert!(both.is_set());
        assert_eq!(both.label(), "Anshul <me@example.com>");
        assert!(!Identity::default().is_set());
    }

    /// The whole round, against the real `git`, where there is one.
    #[test]
    fn a_repository_from_nothing() {
        if !available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("enso-fs-git-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("src")).unwrap();
        assert_eq!(root(&dir.join("src")), None);
        init(&dir).unwrap();
        assert_eq!(root(&dir.join("src")).as_deref(), Some(dir.as_path()));
        let repo = Scope::Repository(dir.clone());
        set_identity(&repo, "Test Person", "test@example.com").unwrap();
        assert_eq!(identity(&repo).label(), "Test Person <test@example.com>");
        assert!(set_identity(&repo, "No Email", "nope").is_err());
        let status = status(&dir).unwrap();
        assert!(status.branch.is_some());
        create_branch(&dir, "feature").unwrap();
        assert_eq!(
            super::status(&dir).unwrap().branch.as_deref(),
            Some("feature")
        );
        assert!(create_branch(&dir, "bad..name").is_err());
        inherit_identity(&dir).unwrap();
        assert_eq!(identity(&repo), Identity::default());
        // Twice is fine.
        inherit_identity(&dir).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }
}
