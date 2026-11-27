//! Version control in the window: asked about once, then a branch pill in
//! the status line, a menu of branches, a new repository from the folder's
//! menu, and who commits — everywhere, or in one repository.
//!
//! When it's off, none of this runs: no `git`, no pill, no menu items.

use std::{path::PathBuf, thread};

use enso_core::style::{opacity, radius, space, type_scale};
use enso_fs::git::{self, Identity, Scope, Status};
use enso_ui::{
    Align, Canvas, Clip, Cx, Key, KeyEvent, Rect, TEXT_TYPES, Text,
    widget::{Form, Interaction, Item, Kind, Menu, Note, Part, TextField, field, field::Frame},
};

use super::overlay::{Overlay, Reach};
use super::{Files, Hit, MenuFor};
use crate::{
    forge::{Account, Repo, Site},
    jobs::Message,
};

/// Editing who commits.
#[derive(Debug)]
pub(super) struct IdentityEdit {
    pub scope: Scope,
    pub name: TextField,
    pub email: TextField,
    /// 0: the name; 1: the email.
    pub focus: usize,
    pub error: Option<String>,
}

/// What `git` said, kept so drawing never runs it.
#[derive(Clone, Debug, Default)]
pub(super) struct GitFacts {
    pub available: bool,
    pub global: Identity,
    /// This repository's own identity, if the folder is in one.
    pub own: Option<Identity>,
    /// Whether git asks enso-files for its way in, to each forge.
    pub forge_git: [bool; 2],
}

impl GitFacts {
    pub fn helped(&self, site: Site) -> bool {
        self.forge_git[site_index(site)]
    }

    pub fn set_helped(&mut self, site: Site, on: bool) {
        self.forge_git[site_index(site)] = on;
    }
}

fn site_index(site: Site) -> usize {
    match site {
        Site::GitHub => 0,
        Site::GitLab => 1,
    }
}

/// Your OAuth app on a forge: its client ID, and for GitLab where your
/// GitLab is.
#[derive(Debug)]
pub(super) struct AppForm {
    pub site: Site,
    pub id: TextField,
    /// GitLab's address: gitlab.com, or one of your own.
    pub host: TextField,
    /// GitLab: 0, the address; 1, the ID.
    pub focus: usize,
    pub error: Option<String>,
}

impl AppForm {
    /// The field being typed in.
    fn field(&mut self) -> &mut TextField {
        if self.site == Site::GitLab && self.focus == 0 {
            &mut self.host
        } else {
            &mut self.id
        }
    }
}

/// Publishing this repository on a forge: its name there, whether it's
/// private, which forge (when there's a choice), and what's wrong.
#[derive(Debug)]
pub(super) struct PublishForm {
    pub name: TextField,
    pub private: bool,
    pub site: Site,
    pub error: Option<String>,
}

/// A question or form version control has open.
#[derive(Debug)]
pub(super) enum Vcs {
    /// "Use version control?" — asked once.
    Ask,
    Identity(IdentityEdit),
    /// A new branch's name.
    NewBranch(TextField, Option<String>),
    /// A repository's address, to clone into the folder shown.
    Clone(TextField, Option<String>),
    /// Your OAuth app on a forge, asked once.
    ClientId(AppForm),
    /// Signing in to a forge: the code to type there, its page, and what
    /// went wrong, if anything did.
    Device {
        site: Site,
        code: String,
        page: String,
        error: Option<String>,
    },
    /// "Disconnect from GitHub?"
    Disconnect(Site),
    /// "Make your home folder a repository?": asked before making one of
    /// a folder everything else is inside.
    InitHere(PathBuf),
    Publish(PublishForm),
}

/// What a line of the branch menu does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum BranchLine {
    Local(String),
    /// A remote's branch not here yet: check it out, tracking.
    Remote(String),
    Fetch,
    Pull,
    Push,
    /// Its page on the forge.
    Web(String),
    NewBranch,
    CommitAs,
    /// Make it on a forge and push it there.
    Publish,
}

/// Work with a remote, on a thread.
#[derive(Clone, Debug)]
enum Remote {
    /// Quietly, if `quiet`: when the menu opens and the last was long ago.
    Fetch {
        quiet: bool,
    },
    Pull,
    Push {
        tracking: bool,
    },
    Clone {
        url: String,
        into: PathBuf,
    },
    /// Make a repository on a forge, add it as `origin`, and push.
    Publish {
        account: Account,
        name: String,
        private: bool,
    },
}

/// Repositories the clone form lists at most.
const MOST_REPOS: usize = 5;

/// How old a fetch can be before the branch menu fetches again.
const STALE: std::time::Duration = std::time::Duration::from_secs(600);

impl Files {
    /// Whether version control is on.
    pub(super) fn git_on(&self) -> bool {
        self.overrides.git()
    }

    /// Ask `git` again what's set, for the panels about to be drawn.
    pub(super) fn learn_git(&mut self) {
        let available = git::available();
        self.git_facts = GitFacts {
            available,
            global: if available {
                git::identity(&Scope::Global)
            } else {
                Identity::default()
            },
            own: git::root(self.pane.location())
                .filter(|_| available)
                .map(|root| git::identity(&Scope::Repository(root))),
            forge_git: Site::ALL.map(|site| available && self.forge(site).used_for_git()),
        };
    }

    /// Ask the question, if it has never been asked: in the file manager,
    /// never in a picker, and only where git is installed.
    pub(super) fn maybe_ask_git(&mut self) {
        if self.overrides.git.is_none()
            && self.picker.is_none()
            && self.overlay.vcs().is_none()
            && !self.asked_git
        {
            self.asked_git = true;
            if git::available() {
                self.overlay = Some(Overlay::Vcs(Vcs::Ask));
            }
        }
    }

    /// Learn the repository's state on a worker, for the folder just listed.
    pub(super) fn refresh_git(&mut self, generation: u64) {
        self.repo = None;
        if !self.git_on() || *self.pane.source() == crate::pane::Source::Recent {
            return;
        }
        let Some(root) = git::root(self.pane.location()) else {
            return;
        };
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        let _ = thread::Builder::new()
            .name("enso-files-git".to_owned())
            .spawn(move || {
                let status = git::status(&root).ok().map(|status| (root, status));
                proxy.send(Message::Git { generation, status });
            });
    }

    pub(super) fn git_arrived(
        &mut self,
        generation: u64,
        status: Option<(PathBuf, Status)>,
        cx: &mut Cx<'_>,
    ) {
        if generation == self.pane.generation() {
            self.repo = status;
            cx.redraw();
        }
    }

    /// Turn version control on or off, and remember the answer.
    pub(super) fn set_git(&mut self, on: bool, cx: &mut Cx<'_>) {
        self.overrides.git = Some(on);
        self.save_overrides(cx);
        if on {
            self.refresh_git(self.pane.generation());
        } else {
            self.repo = None;
        }
        cx.redraw();
    }

    pub(super) fn edit_identity(&mut self, scope: Scope) {
        // Start from what's set there — or, for a repository, from the
        // global identity it would otherwise use.
        let mut identity = git::identity(&scope);
        if !identity.is_set() {
            let global = git::identity(&Scope::Global);
            identity = Identity {
                name: identity.name.or(global.name),
                email: identity.email.or(global.email),
            };
        }
        self.learn_git();
        let mut name = TextField::new(identity.name.as_deref().unwrap_or_default());
        name.select_all();
        self.overlay = Some(Overlay::Vcs(Vcs::Identity(IdentityEdit {
            scope,
            name,
            email: TextField::new(identity.email.as_deref().unwrap_or_default()),
            focus: 0,
            error: None,
        })));
    }

    fn save_identity(&mut self, cx: &mut Cx<'_>) {
        let Some(Vcs::Identity(edit)) = self.overlay.vcs_mut() else {
            return;
        };
        match git::set_identity(&edit.scope, edit.name.text(), edit.email.text()) {
            Ok(()) => {
                let whom = format!("{} <{}>", edit.name.text().trim(), edit.email.text().trim());
                let where_ = match &edit.scope {
                    Scope::Global => "everywhere".to_owned(),
                    Scope::Repository(root) => format!(
                        "in {}",
                        root.file_name()
                            .map_or_else(|| root.to_string_lossy(), |name| name.to_string_lossy())
                    ),
                };
                self.overlay = None;
                self.learn_git();
                self.toast(format!("Commits as {whom}, {where_}"), cx);
            }
            Err(err) => edit.error = Some(err.to_string()),
        }
        cx.redraw();
    }

    /// The branch pill was clicked: the branches here, those on the
    /// remotes not here yet, fetching, pulling, pushing, the forge's page,
    /// a new branch, and who commits.
    pub(super) fn branch_menu(&mut self, rect: Rect) {
        let Some((root, status)) = self.repo.clone() else {
            return;
        };
        let branches = git::branches(&root).unwrap_or_default();
        let remotes = git::remotes(&root).unwrap_or_default();
        let mut lines: Vec<(Item, Option<BranchLine>)> = branches
            .iter()
            .map(|branch| {
                (
                    Item::new(branch).checked(status.branch.as_ref() == Some(branch)),
                    Some(BranchLine::Local(branch.clone())),
                )
            })
            .collect();
        // The remotes' branches with no local branch of their name.
        let others: Vec<String> = git::remote_branches(&root)
            .unwrap_or_default()
            .into_iter()
            .filter(|remote| {
                let short = remote
                    .split_once('/')
                    .map_or(remote.as_str(), |(_, name)| name);
                !branches.iter().any(|branch| branch == short)
            })
            .collect();
        if !others.is_empty() {
            lines.push((Item::Separator, None));
            for other in others {
                lines.push((Item::new(&other), Some(BranchLine::Remote(other))));
            }
        }
        if !remotes.is_empty() {
            lines.push((Item::Separator, None));
            lines.push((Item::new("Fetch"), Some(BranchLine::Fetch)));
            if status.tracking {
                let label = if status.behind > 0 {
                    format!("Pull ({} new)", status.behind)
                } else {
                    "Pull".to_owned()
                };
                lines.push((Item::new(&label), Some(BranchLine::Pull)));
            }
            if status.branch.is_some() {
                let label = match (status.tracking, status.ahead) {
                    (false, _) => "Push, as a new branch".to_owned(),
                    (true, 0) => "Push".to_owned(),
                    (true, ahead) => format!("Push ({ahead} to send)"),
                };
                lines.push((Item::new(&label), Some(BranchLine::Push)));
            }
            let origin = remotes
                .iter()
                .find(|(name, _)| name == "origin")
                .or(remotes.first());
            if let Some(page) = origin.and_then(|(_, url)| git::web_page(url)) {
                let label = format!("Open on {}", git::forge(&page));
                lines.push((Item::new(&label), Some(BranchLine::Web(page))));
            }
        }
        // Nowhere to push yet, and an account to make somewhere with.
        let accounts = self.accounts();
        if remotes.is_empty() && !accounts.is_empty() {
            let label = match accounts.as_slice() {
                [only] => format!("Publish on {}…", only.forge.name()),
                _ => "Publish…".to_owned(),
            };
            lines.push((Item::Separator, None));
            lines.push((Item::new(&label), Some(BranchLine::Publish)));
        }
        lines.push((Item::Separator, None));
        lines.push((Item::new("New branch…"), Some(BranchLine::NewBranch)));
        lines.push((Item::new("Commit as…"), Some(BranchLine::CommitAs)));
        let current = branches
            .iter()
            .position(|branch| status.branch.as_ref() == Some(branch))
            .unwrap_or(0);
        let (items, actions): (Vec<Item>, Vec<Option<BranchLine>>) = lines.into_iter().unzip();
        // Opens upward from the status line.
        let menu = Menu::new(items, rect.x, rect.y).with_selected(current);
        self.overlay = Some(Overlay::Menu((menu, MenuFor::Branches(actions))));
        // The remotes' branches as they are now, if it's been a while.
        if !remotes.is_empty() && git::since_fetch(&root).is_none_or(|age| age > STALE) {
            self.remote(Remote::Fetch { quiet: true });
        }
    }

    pub(super) fn branch_chosen(
        &mut self,
        lines: Vec<Option<BranchLine>>,
        index: usize,
        cx: &mut Cx<'_>,
    ) {
        let Some((root, status)) = self.repo.clone() else {
            return;
        };
        let Some(Some(line)) = lines.into_iter().nth(index) else {
            return;
        };
        let switched = |files: &mut Self,
                        result: std::io::Result<()>,
                        name: &str,
                        cx: &mut Cx<'_>| match result {
            Ok(()) => {
                files.toast(format!("On {name} now"), cx);
                let request = files.pane.reload();
                if let Some(proxy) = &files.proxy {
                    files.loader.load(request, proxy);
                }
                files.refresh_git(files.pane.generation());
            }
            Err(err) => files.toast(format!("Can't switch: {err}"), cx),
        };
        match line {
            BranchLine::Local(branch) => switched(self, git::switch(&root, &branch), &branch, cx),
            BranchLine::Remote(remote) => {
                let short = remote
                    .split_once('/')
                    .map_or(remote.clone(), |(_, name)| name.to_owned());
                switched(self, git::track(&root, &remote), &short, cx);
            }
            BranchLine::Fetch => {
                self.toast("Fetching…", cx);
                self.remote(Remote::Fetch { quiet: false });
            }
            BranchLine::Pull => {
                self.toast("Pulling…", cx);
                self.remote(Remote::Pull);
            }
            BranchLine::Push => {
                self.toast("Pushing…", cx);
                self.remote(Remote::Push {
                    tracking: status.tracking,
                });
            }
            BranchLine::Web(page) => {
                if let Err(err) = enso_fs::open::uri(&page) {
                    self.toast(format!("Can't open it: {err}"), cx);
                }
            }
            BranchLine::NewBranch => {
                self.overlay = Some(Overlay::Vcs(Vcs::NewBranch(TextField::default(), None)))
            }
            BranchLine::Publish => {
                let name = root
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut field = TextField::new(&name);
                field.select_all();
                let site = self
                    .accounts()
                    .first()
                    .map_or(Site::GitHub, |account| account.forge.site);
                self.overlay = Some(Overlay::Vcs(Vcs::Publish(PublishForm {
                    name: field,
                    private: true,
                    site,
                    error: None,
                })));
            }
            BranchLine::CommitAs => self.edit_identity(Scope::Repository(root)),
        }
        cx.redraw();
    }

    /// Ask for a repository's address to clone here.
    pub(super) fn ask_clone(&mut self) {
        self.overlay = Some(Overlay::Vcs(Vcs::Clone(TextField::default(), None)));
        self.repo_pick = 0;
        // Signed in: your repositories on each forge, to pick from.
        self.repos.clear();
        for account in self.accounts() {
            let Some(proxy) = self.proxy.clone() else {
                break;
            };
            let _ = thread::Builder::new()
                .name("enso-files-repos".to_owned())
                .spawn(move || {
                    let site = account.forge.site;
                    let result = account
                        .forge
                        .token(&account.login, &account.client_id)
                        .ok_or_else(|| {
                            format!(
                                "The keyring has no {} token: sign in again",
                                account.forge.name()
                            )
                        })
                        .and_then(|token| account.forge.repos(&token));
                    proxy.send(Message::Repos(site, result));
                });
        }
    }

    /// The repositories matching what's typed in the clone form, best
    /// first, at most `MOST_REPOS`.
    pub(super) fn repos_shown(&self) -> Vec<&Repo> {
        let Some(Vcs::Clone(field, _)) = self.overlay.vcs() else {
            return Vec::new();
        };
        let text = field.text().trim();
        // An address being typed: no list.
        if text.contains("://") || text.contains('@') {
            return Vec::new();
        }
        let pattern = enso_fs::fuzzy::Pattern::new(text);
        let mut scored: Vec<(i32, &Repo)> = self
            .repos
            .iter()
            .filter_map(|repo| pattern.score(&repo.name).map(|score| (score, repo)))
            .collect();
        if !text.is_empty() {
            scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        }
        scored
            .into_iter()
            .map(|(_, repo)| repo)
            .take(MOST_REPOS)
            .collect()
    }

    /// Run `work` with a remote on a thread; what came of it arrives as
    /// [`Message::Remote`].
    fn remote(&mut self, work: Remote) {
        let (Some(proxy), Some(root)) = (
            self.proxy.clone(),
            self.repo
                .as_ref()
                .map(|(root, _)| root.clone())
                .or_else(|| match &work {
                    Remote::Clone { into, .. } => Some(into.clone()),
                    _ => None,
                }),
        ) else {
            return;
        };
        let _ = thread::Builder::new()
            .name("enso-files-remote".to_owned())
            .spawn(move || {
                let (said, result) = match &work {
                    Remote::Fetch { quiet } => (
                        (!quiet).then(|| "Up to date with the remotes".to_owned()),
                        git::fetch(&root).map(|()| None),
                    ),
                    Remote::Pull => (Some("Pulled".to_owned()), git::pull(&root).map(|()| None)),
                    Remote::Push { tracking } => (
                        Some("Pushed".to_owned()),
                        git::push(&root, *tracking).map(|()| None),
                    ),
                    Remote::Clone { url, into } => {
                        (Some("Cloned".to_owned()), git::clone(url, into).map(Some))
                    }
                    // Three steps: make it, add it as origin, push.
                    Remote::Publish {
                        account,
                        name,
                        private,
                    } => {
                        let forge = &account.forge;
                        let published = forge
                            .token(&account.login, &account.client_id)
                            .ok_or_else(|| {
                                format!("The keyring has no {} token: sign in again", forge.name())
                            })
                            .and_then(|token| forge.create(&token, name, *private))
                            .and_then(|made| {
                                git::add_remote(&root, "origin", &made.clone_url)
                                    .map_err(|err| err.to_string())?;
                                git::push(&root, false).map_err(|err| err.to_string())?;
                                Ok(made.name)
                            });
                        match published {
                            Ok(made) => (
                                Some(format!("Published on {} as {made}", forge.name())),
                                Ok(None),
                            ),
                            Err(why) => (None, Err(std::io::Error::other(why))),
                        }
                    }
                };
                let quiet_failure = matches!(work, Remote::Fetch { quiet: true });
                let result = result.map_err(|err| err.to_string());
                // A fetch nobody asked for fails quietly: offline is fine.
                if quiet_failure && result.is_err() {
                    return;
                }
                proxy.send(Message::Remote { said, result });
            });
    }

    /// Work with a remote finished.
    pub(super) fn remote_done(
        &mut self,
        said: Option<String>,
        result: Result<Option<PathBuf>, String>,
        cx: &mut Cx<'_>,
    ) {
        match result {
            Ok(folder) => {
                if let Some(said) = said {
                    self.toast(said, cx);
                }
                if let Some(folder) = folder {
                    self.reveal(&folder);
                }
                self.refresh_git(self.pane.generation());
            }
            Err(why) => self.toast(format!("Git says: {why}"), cx),
        }
        cx.redraw();
    }

    /// Make the folder shown a repository — asking first where that would
    /// take in everything below (the home folder, the root).
    pub(super) fn init_repository(&mut self, cx: &mut Cx<'_>) {
        let dir = self.pane.location().to_owned();
        if dir == self.home || dir.parent().is_none() {
            self.overlay = Some(Overlay::Vcs(Vcs::InitHere(dir)));
            cx.redraw();
            return;
        }
        self.init_here(dir, cx);
    }

    /// `git init` in `dir`, on `main`; `Ctrl+Z` takes it back (its `.git`
    /// goes to the trash).
    fn init_here(&mut self, dir: PathBuf, cx: &mut Cx<'_>) {
        self.overlay = None;
        match git::init(&dir) {
            Ok(branch) => {
                self.remember(enso_fs::ops::Done::Created(vec![dir.join(".git")]));
                self.toast(
                    format!("A Git repository now, on {branch} · Ctrl+Z undoes it"),
                    cx,
                );
                self.refresh_git(self.pane.generation());
                // No one to commit as yet: ask.
                if !git::identity(&Scope::Global).is_set() {
                    self.edit_identity(Scope::Repository(dir));
                }
            }
            Err(err) => self.toast(format!("Can't make this a repository: {err}"), cx),
        }
    }

    /// Keys for version control's question and forms. Returns whether one
    /// was open.
    pub(super) fn vcs_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(vcs) = self.overlay.vcs_mut() else {
            return false;
        };
        match vcs {
            Vcs::Ask => match key.key {
                Key::Enter | Key::Char('y') => self.answer_git(true, cx),
                Key::Escape | Key::Char('n') => self.answer_git(false, cx),
                _ => {}
            },
            Vcs::Identity(edit) => {
                if key.key == Key::Tab {
                    edit.focus = 1 - edit.focus;
                    cx.redraw();
                    return true;
                }
                let field = if edit.focus == 0 {
                    &mut edit.name
                } else {
                    &mut edit.email
                };
                match field.key(key) {
                    field::Response::Submit if edit.focus == 0 => edit.focus = 1,
                    field::Response::Submit => self.save_identity(cx),
                    field::Response::Cancel => self.overlay = None,
                    field::Response::Copy(text) | field::Response::Cut(text) => {
                        cx.copy(Clip::text(&text))
                    }
                    field::Response::Paste => cx.paste(&TEXT_TYPES),
                    _ => edit.error = None,
                }
            }
            Vcs::NewBranch(field, error) => match field.key(key) {
                field::Response::Submit => {
                    let name = field.text().trim().to_owned();
                    let Some((root, _)) = self.repo.clone() else {
                        self.overlay = None;
                        return true;
                    };
                    match git::create_branch(&root, &name) {
                        Ok(()) => {
                            self.overlay = None;
                            self.toast(format!("On {name} now"), cx);
                            self.refresh_git(self.pane.generation());
                        }
                        Err(err) => *error = Some(err.to_string()),
                    }
                }
                field::Response::Cancel => self.overlay = None,
                field::Response::Paste => cx.paste(&TEXT_TYPES),
                _ => *error = None,
            },
            Vcs::ClientId(app) if key.key == Key::Tab && app.site == Site::GitLab => {
                app.focus = 1 - app.focus;
            }
            Vcs::ClientId(app) => match app.field().key(key) {
                // GitLab's address first, then on to the ID.
                field::Response::Submit if app.site == Site::GitLab && app.focus == 0 => {
                    app.focus = 1;
                }
                field::Response::Submit => {
                    let id = app.id.text().trim().to_owned();
                    let site = app.site;
                    let host = (site == Site::GitLab).then(|| app.host.text().trim().to_owned());
                    if id.is_empty() || id.contains(char::is_whitespace) {
                        app.error = Some(format!(
                            "The ID, as {} shows it on the app's page",
                            site.name()
                        ));
                    } else if host
                        .as_deref()
                        .is_some_and(|host| host.is_empty() || host.contains(char::is_whitespace))
                    {
                        app.error = Some("Where your GitLab is: gitlab.com, say".to_owned());
                        app.focus = 0;
                    } else {
                        self.set_client_id(site, id, host, cx);
                    }
                }
                field::Response::Cancel => self.overlay = None,
                field::Response::Copy(text) | field::Response::Cut(text) => {
                    cx.copy(Clip::text(&text))
                }
                field::Response::Paste => cx.paste(&TEXT_TYPES),
                _ => app.error = None,
            },
            Vcs::Device { .. } => match key.key {
                Key::Enter => self.open_device_page(cx),
                Key::Escape => self.cancel_sign_in(),
                _ => {}
            },
            Vcs::Disconnect(site) => match key.key {
                Key::Enter => {
                    let site = *site;
                    self.disconnect(site, cx);
                }
                Key::Escape => self.overlay = None,
                _ => {}
            },
            Vcs::InitHere(dir) => match key.key {
                Key::Enter => {
                    let dir = dir.clone();
                    self.init_here(dir, cx);
                }
                Key::Escape => self.overlay = None,
                _ => {}
            },
            Vcs::Publish(form) => match key.key {
                // Shift+Tab: the other forge, when both are signed in to.
                Key::Tab if key.mods.shift => {
                    let other = match form.site {
                        Site::GitHub => Site::GitLab,
                        Site::GitLab => Site::GitHub,
                    };
                    let has_other = match other {
                        Site::GitHub => self.overrides.github_account.is_some(),
                        Site::GitLab => self.overrides.gitlab_account.is_some(),
                    };
                    if has_other && let Some(Vcs::Publish(form)) = self.overlay.vcs_mut() {
                        form.site = other;
                    }
                }
                Key::Tab => form.private = !form.private,
                _ => match form.name.key(key) {
                    field::Response::Submit => {
                        let name = form.name.text().trim().to_owned();
                        let (private, site) = (form.private, form.site);
                        if name.is_empty() || name.contains(char::is_whitespace) {
                            form.error = Some("A name, without spaces".to_owned());
                        } else if let Some(account) = self.signed_in_to(site) {
                            self.overlay = None;
                            self.toast(format!("Publishing on {}…", account.forge.name()), cx);
                            self.remote(Remote::Publish {
                                account,
                                name,
                                private,
                            });
                        }
                    }
                    field::Response::Cancel => self.overlay = None,
                    field::Response::Paste => cx.paste(&TEXT_TYPES),
                    _ => form.error = None,
                },
            },
            Vcs::Clone(..) if key.nav(false).is_some() => {
                let count = self.repos_shown().len();
                if count > 0
                    && let Some(nav) = key.nav(false)
                {
                    self.repo_pick = match nav {
                        enso_ui::Nav::Up => self.repo_pick.checked_sub(1).unwrap_or(count - 1),
                        enso_ui::Nav::Down => (self.repo_pick + 1) % count,
                    };
                }
            }
            Vcs::Clone(..) if key.key == Key::Enter => {
                // A repository picked from the list, or the address typed.
                let picked = self
                    .repos_shown()
                    .get(self.repo_pick)
                    .map(|repo| repo.clone_url.clone());
                let Some(Vcs::Clone(field, error)) = self.overlay.vcs_mut() else {
                    return true;
                };
                let url = picked.unwrap_or_else(|| field.text().trim().to_owned());
                if url.is_empty() {
                    *error = Some("The repository's address goes here".to_owned());
                } else {
                    self.overlay = None;
                    self.toast("Cloning…", cx);
                    let into = self.pane.location().to_owned();
                    self.remote(Remote::Clone { url, into });
                }
            }
            Vcs::Clone(field, error) => match field.key(key) {
                field::Response::Changed => {
                    *error = None;
                    self.repo_pick = 0;
                }
                field::Response::Cancel => self.overlay = None,
                field::Response::Paste => cx.paste(&TEXT_TYPES),
                _ => *error = None,
            },
        }
        cx.redraw();
        true
    }

    /// Text pasted while a form is open.
    pub(super) fn vcs_paste(&mut self, text: &str) -> bool {
        match self.overlay.vcs_mut() {
            Some(Vcs::Identity(edit)) => {
                if edit.focus == 0 {
                    &mut edit.name
                } else {
                    &mut edit.email
                }
                .insert(text);
                true
            }
            Some(Vcs::NewBranch(field, _) | Vcs::Clone(field, _)) => {
                field.insert(text);
                true
            }
            Some(Vcs::ClientId(app)) => {
                app.field().insert(text);
                true
            }
            Some(Vcs::Publish(form)) => {
                form.name.insert(text);
                true
            }
            _ => false,
        }
    }

    fn answer_git(&mut self, on: bool, cx: &mut Cx<'_>) {
        self.overlay = None;
        self.set_git(on, cx);
        if on {
            // Who commits: the global identity, shown to confirm or change,
            // or asked for if there's none.
            self.edit_identity(Scope::Global);
        } else {
            self.toast(
                "Version control stays off · Settings (Ctrl+,) can turn it on",
                cx,
            );
        }
    }

    /// A press while a form is open: only it answers. Returns whether one
    /// was open.
    pub(super) fn vcs_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        if self.overlay.vcs().is_none() {
            return false;
        }
        match hit {
            Some(Hit::GitYes) => self.answer_git(true, cx),
            Some(Hit::GitNo) => self.answer_git(false, cx),
            Some(Hit::FormField(index)) => match self.overlay.vcs_mut() {
                Some(Vcs::Identity(edit)) => edit.focus = *index,
                Some(Vcs::ClientId(app)) => app.focus = *index,
                _ => {}
            },
            Some(Hit::FormChoice(index)) => {
                if let Some(Vcs::Publish(form)) = self.overlay.vcs_mut() {
                    form.site = Site::ALL[*index];
                }
            }
            Some(Hit::FormSave) => match self.overlay.vcs() {
                Some(Vcs::Identity(_)) => self.save_identity(cx),
                Some(Vcs::Device { .. }) => self.open_device_page(cx),
                Some(Vcs::Disconnect(site)) => {
                    let site = *site;
                    self.disconnect(site, cx);
                }
                Some(Vcs::InitHere(dir)) => {
                    let dir = dir.clone();
                    self.init_here(dir, cx);
                }
                Some(
                    Vcs::NewBranch(..) | Vcs::Clone(..) | Vcs::ClientId(..) | Vcs::Publish(..),
                ) => {
                    let enter = KeyEvent {
                        key: Key::Enter,
                        mods: Default::default(),
                        repeat: false,
                    };
                    self.vcs_key(&enter, cx);
                }
                _ => {}
            },
            Some(Hit::FormCard) => {}
            Some(Hit::FormCancel) => {
                if matches!(self.overlay.vcs(), Some(Vcs::Device { .. })) {
                    self.cancel_sign_in();
                } else {
                    self.overlay = None;
                }
            }
            // A press outside: a form is put away, but a sign-in under way
            // isn't lost to a stray click (Esc and Cancel stop it).
            None if !matches!(self.overlay.vcs(), Some(Vcs::Device { .. })) => {
                self.overlay = None;
            }
            _ => {}
        }
        cx.redraw();
        true
    }

    /// The branch pill, at the right of the status line. Returns where its
    /// left edge is, for what else goes there.
    pub(super) fn draw_branch(&mut self, canvas: &mut Canvas<'_>, inner: Rect) -> f32 {
        let Some((_, status)) = &self.repo else {
            return inner.right();
        };
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let small = self.font(type_scale::SMALL);
        let text = status.label();
        let label = Text::new(&text, small, colors.text);
        let width = canvas.measure(&label).w + 2.0 * space::MD + 10.0;
        let height = 22.0;
        let rect = Rect::new(
            inner.right() - width,
            inner.y + (inner.h - height) / 2.0,
            width,
            height,
        );
        let hovered = self.hover == Some(Hit::Branch);
        canvas.fill(
            rect,
            radius::pill(height),
            if hovered {
                colors.highlight_med
            } else {
                colors.overlay
            },
        );
        // A dot for "a branch".
        canvas.circle(rect.x + space::MD, rect.y + height / 2.0, 3.0, accent);
        canvas.text_in(
            &label,
            Rect::new(rect.x + space::MD + 8.0, rect.y, width, height),
            Align::Start,
        );
        self.hits.push((rect, Hit::Branch));
        rect.x - space::MD
    }

    /// Version control's question or form, over everything.
    pub(super) fn draw_vcs(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let Some(vcs) = self.overlay.vcs() else {
            return;
        };
        let look = self.look();
        let small = self.font(type_scale::SMALL);
        let (title, detail, fields, buttons): (String, String, usize, [(&str, Kind, Hit); 2]) = match vcs {
            Vcs::Ask => (
                "Use version control?".to_owned(),
                "See the branch a folder is on, switch branches, and start repositories — with git. \
                 Settings (Ctrl+,) changes this any time."
                    .to_owned(),
                0,
                [("Not now", Kind::Secondary, Hit::GitNo), ("Turn on", Kind::Primary, Hit::GitYes)],
            ),
            Vcs::Identity(edit) => {
                let (title, detail) = match &edit.scope {
                    Scope::Global => (
                        "Who commits?".to_owned(),
                        if self.git_facts.global.is_set() {
                            "Your global identity, used in every repository unless one has its own.".to_owned()
                        } else {
                            "No global identity yet: set one here, used in every repository unless one has its own."
                                .to_owned()
                        },
                    ),
                    Scope::Repository(root) => (
                        format!(
                            "Who commits in {}?",
                            root.file_name().map_or_else(|| root.to_string_lossy(), |name| name.to_string_lossy())
                        ),
                        "Only in this repository; everywhere else uses the global identity.".to_owned(),
                    ),
                };
                (title, detail, 2, [("Cancel", Kind::Secondary, Hit::FormCancel), ("Save", Kind::Primary, Hit::FormSave)])
            }
            Vcs::NewBranch(..) => (
                "New branch".to_owned(),
                "Started from where you are, and checked out.".to_owned(),
                1,
                [("Cancel", Kind::Secondary, Hit::FormCancel), ("Create", Kind::Primary, Hit::FormSave)],
            ),
            Vcs::ClientId(app) => {
                let forge = match app.site {
                    Site::GitHub => self.forge(Site::GitHub),
                    Site::GitLab => crate::forge::Forge::gitlab(app.host.text()),
                };
                (
                    format!("Your {} OAuth app", app.site.name()),
                    forge.app_help(),
                    if app.site == Site::GitLab { 2 } else { 1 },
                    [("Cancel", Kind::Secondary, Hit::FormCancel), ("Sign in", Kind::Primary, Hit::FormSave)],
                )
            }
            Vcs::Device { site, code, .. } => {
                let name = self.forge(*site).name();
                (
                    format!("Sign in to {name}"),
                    if code.is_empty() {
                        format!("Asking {name} for a code…")
                    } else {
                        format!(
                            "It's copied: paste it at {name} and approve. Open takes you there — this waits, \
                             and Esc or Cancel stops it."
                        )
                    },
                    1,
                    [("Cancel", Kind::Secondary, Hit::FormCancel), ("Open", Kind::Primary, Hit::FormSave)],
                )
            }
            Vcs::Publish(form) => (
                format!("Publish on {}", self.forge(form.site).name()),
                format!(
                    "Made on your account as a {} repository, added as origin, and pushed. Tab: make it {}.{}",
                    if form.private { "private" } else { "public" },
                    if form.private { "public" } else { "private" },
                    if self.accounts().len() > 1 { " Shift+Tab: the other forge." } else { "" }
                ),
                1,
                [("Cancel", Kind::Secondary, Hit::FormCancel), ("Publish", Kind::Primary, Hit::FormSave)],
            ),
            Vcs::InitHere(dir) => (
                if *dir == self.home {
                    "Make your home folder a repository?".to_owned()
                } else {
                    "Make the whole disk a repository?".to_owned()
                },
                "Everything in it would be inside it: every folder below would show as part of \
                 it. For a project, make a new Git repository instead — a folder of its own."
                    .to_owned(),
                0,
                [("Cancel", Kind::Secondary, Hit::FormCancel), ("Make it one", Kind::Primary, Hit::FormSave)],
            ),
            Vcs::Disconnect(site) => (
                format!(
                    "Disconnect from {} ({})?",
                    self.forge(*site).name(),
                    self.account(*site).unwrap_or("you")
                ),
                "Its token leaves your keyring. You can sign in again any time.".to_owned(),
                0,
                [("Keep", Kind::Secondary, Hit::FormCancel), ("Disconnect", Kind::Danger, Hit::FormSave)],
            ),
            Vcs::Clone(..) => (
                "Clone a repository".to_owned(),
                if self.repos.is_empty() {
                    "Its address, from a forge or anywhere: copied into this folder, under its own name.".to_owned()
                } else {
                    "One of yours, by name — or any address: copied into this folder, under its own name.".to_owned()
                },
                if self.repos.is_empty() { 1 } else { 4 },
                [("Cancel", Kind::Secondary, Hit::FormCancel), ("Clone", Kind::Primary, Hit::FormSave)],
            ),
        };
        // A one-field form's example.
        let single_placeholder = match vcs {
            Vcs::Clone(..) => "https://github.com/owner/repository",
            Vcs::ClientId(..) => "Ov23li…",
            Vcs::Publish(..) => "repository-name",
            _ => "feature/quiet-mode",
        };
        let error = match vcs {
            Vcs::Identity(edit) => edit.error.clone(),
            Vcs::ClientId(app) => app.error.clone(),
            Vcs::Publish(form) => form.error.clone(),
            Vcs::Device { error, .. } | Vcs::NewBranch(_, error) | Vcs::Clone(_, error) => {
                error.clone()
            }
            Vcs::Ask | Vcs::Disconnect(_) | Vcs::InitHere(_) => None,
        };
        // Publishing with two forges signed in to: which one.
        let publish_on = match vcs {
            Vcs::Publish(form) if self.accounts().len() > 1 => Some(form.site),
            _ => None,
        };
        let forges = Site::ALL.map(Site::name);
        let code = match vcs {
            Vcs::Device { code, .. } if code.is_empty() => Some("· · · ·".to_owned()),
            Vcs::Device { code, .. } => Some(code.clone()),
            _ => None,
        };
        // Signed in: your repositories under the field, the picked one lit,
        // each with where it is when there's more than one place.
        let several = self.accounts().len() > 1;
        let shown: Vec<(String, String)> = if matches!(vcs, Vcs::Clone(..)) {
            self.repos_shown()
                .iter()
                .map(|repo| {
                    let note = match (repo.private, several) {
                        (true, true) => format!("private · {}", repo.forge),
                        (true, false) => "private".to_owned(),
                        (false, true) => repo.forge.to_owned(),
                        (false, false) => String::new(),
                    };
                    (repo.name.clone(), note)
                })
                .collect()
        } else {
            Vec::new()
        };
        const REPO_ROW: f32 = 32.0;

        let labels = if matches!(vcs, Vcs::ClientId(_)) {
            ["Where", "App ID"]
        } else {
            ["Name", "Email"]
        };
        let mut form = Form::new(&title).width(480.0).about(&detail);
        if let Some(site) = publish_on {
            form = form.part(Part::Choices {
                label: Some("On"),
                options: &forges,
                chosen: site_index(site),
                hovered: match self.hover {
                    Some(Hit::FormChoice(index)) => Some(index),
                    _ => None,
                },
                enabled: true,
            });
        }
        if fields == 2 {
            form = form
                .part(Part::Field {
                    label: Some(labels[0]),
                })
                .part(Part::Field {
                    label: Some(labels[1]),
                });
        } else if let Some(code) = &code {
            form = form.part(Part::Code(code));
        } else if fields > 0 {
            form = form.field();
        }
        if !shown.is_empty() {
            form = form.part(Part::Space(shown.len() as f32 * REPO_ROW));
        }
        if !matches!(vcs, Vcs::Ask | Vcs::Disconnect(_) | Vcs::InitHere(_)) {
            form = form.note(error.as_deref().map_or(Note::Nothing, Note::Complains));
        }
        for (label, kind, hit) in &buttons {
            form = form.button(
                label,
                *kind,
                Interaction {
                    hovered: self.hover.as_ref() == Some(hit),
                    ..Interaction::default()
                },
            );
        }
        let placed = form.draw(canvas, bounds, &look, 1.0);

        // The card itself, under its buttons and fields: a press on it is
        // nothing.
        let mut hits = vec![(placed.inner.inset(-space::XL), Hit::FormCard)];
        let field = |canvas: &mut Canvas<'_>, place: Rect, field: &mut TextField, focused, hint| {
            let frame = Frame {
                focused,
                filled: !field.is_empty(),
                ..Frame::default()
            };
            let text = frame.draw(canvas, place, &look, look.base);
            field.draw(canvas, text, &field::Style::new(&look, hint), focused);
        };
        match self.overlay.vcs_mut() {
            Some(Vcs::Identity(edit)) => {
                let focus = edit.focus;
                field(
                    canvas,
                    placed.parts[0],
                    &mut edit.name,
                    focus == 0,
                    "Your name",
                );
                field(
                    canvas,
                    placed.parts[1],
                    &mut edit.email,
                    focus == 1,
                    "you@example.com",
                );
                hits.push((placed.parts[0], Hit::FormField(0)));
                hits.push((placed.parts[1], Hit::FormField(1)));
            }
            Some(Vcs::ClientId(app)) if app.site == Site::GitLab => {
                let focus = app.focus;
                field(
                    canvas,
                    placed.parts[0],
                    &mut app.host,
                    focus == 0,
                    "gitlab.com",
                );
                field(
                    canvas,
                    placed.parts[1],
                    &mut app.id,
                    focus == 1,
                    "a long hexadecimal ID",
                );
                hits.push((placed.parts[0], Hit::FormField(0)));
                hits.push((placed.parts[1], Hit::FormField(1)));
            }
            Some(Vcs::ClientId(app)) => {
                field(
                    canvas,
                    placed.parts[0],
                    &mut app.id,
                    true,
                    single_placeholder,
                );
            }
            Some(Vcs::Publish(form)) => {
                // After the choice of forge, when there is one.
                let at = usize::from(publish_on.is_some());
                field(
                    canvas,
                    placed.parts[at],
                    &mut form.name,
                    true,
                    single_placeholder,
                );
            }
            Some(Vcs::NewBranch(text, _) | Vcs::Clone(text, _)) => {
                field(canvas, placed.parts[0], text, true, single_placeholder);
            }
            _ => {}
        }
        for &(_, index, pill) in &placed.choices {
            hits.push((pill, Hit::FormChoice(index)));
        }
        if let Some(list) = placed.parts.last().filter(|_| !shown.is_empty()) {
            for (index, (name, note)) in shown.iter().enumerate() {
                let rect = Rect::new(list.x, list.y + index as f32 * REPO_ROW, list.w, 30.0);
                if index == self.repo_pick {
                    canvas.fill(rect, radius::CONTROL, look.accent.alpha(opacity::TINT));
                }
                let label = rect.inset_xy(space::MD, 0.0);
                canvas.text_in(
                    &Text::new(name, small, look.text).max_width(label.w - 130.0),
                    label,
                    Align::Start,
                );
                if !note.is_empty() {
                    canvas.text_in(&Text::new(note, small, look.muted), label, Align::End);
                }
            }
        }
        self.hits.extend(hits);
        for (rect, (_, _, hit)) in placed.buttons.iter().zip(buttons) {
            self.hits.push((*rect, hit));
        }
    }
}
