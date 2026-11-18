//! Version control in the window: asked about once, then a branch pill in
//! the status line, a menu of branches, a new repository from the folder's
//! menu, and who commits — everywhere, or in one repository.
//!
//! When it's off, none of this runs: no `git`, no pill, no menu items.

use std::{path::PathBuf, thread};

use enso_core::style::{opacity, radius, space, stroke, type_scale};
use enso_fs::git::{self, Identity, Scope, Status};
use enso_ui::{
    Align, Canvas, Clip, Cx, Key, KeyEvent, Rect, TEXT_TYPES, Text,
    widget::{Button, Interaction, Item, Kind, Menu, TextField, controls, field},
};

use super::{Files, Hit, MenuFor};
use crate::jobs::Message;

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
}

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
        };
    }

    /// Ask the question, if it has never been asked: in the file manager,
    /// never in a picker, and only where git is installed.
    pub(super) fn maybe_ask_git(&mut self) {
        if self.overrides.git.is_none()
            && self.picker.is_none()
            && self.vcs.is_none()
            && !self.asked_git
        {
            self.asked_git = true;
            if git::available() {
                self.vcs = Some(Vcs::Ask);
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
        self.vcs = Some(Vcs::Identity(IdentityEdit {
            scope,
            name,
            email: TextField::new(identity.email.as_deref().unwrap_or_default()),
            focus: 0,
            error: None,
        }));
    }

    fn save_identity(&mut self, cx: &mut Cx<'_>) {
        let Some(Vcs::Identity(edit)) = &mut self.vcs else {
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
                self.vcs = None;
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
        self.menu = Some((menu, MenuFor::Branches(actions)));
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
            BranchLine::NewBranch => self.vcs = Some(Vcs::NewBranch(TextField::default(), None)),
            BranchLine::CommitAs => self.edit_identity(Scope::Repository(root)),
        }
        cx.redraw();
    }

    /// Ask for a repository's address to clone here.
    pub(super) fn ask_clone(&mut self) {
        self.vcs = Some(Vcs::Clone(TextField::default(), None));
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

    pub(super) fn init_repository(&mut self, cx: &mut Cx<'_>) {
        let dir = self.pane.location().to_owned();
        match git::init(&dir) {
            Ok(()) => {
                self.toast("A Git repository now", cx);
                self.refresh_git(self.pane.generation());
                // No one to commit as yet: ask.
                if !git::identity(&Scope::Global).is_set() {
                    self.edit_identity(Scope::Repository(dir));
                }
            }
            Err(err) => self.toast(format!("Can't start a repository here: {err}"), cx),
        }
    }

    /// Keys for version control's question and forms. Returns whether one
    /// was open.
    pub(super) fn vcs_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(vcs) = &mut self.vcs else {
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
                    field::Response::Cancel => self.vcs = None,
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
                        self.vcs = None;
                        return true;
                    };
                    match git::create_branch(&root, &name) {
                        Ok(()) => {
                            self.vcs = None;
                            self.toast(format!("On {name} now"), cx);
                            self.refresh_git(self.pane.generation());
                        }
                        Err(err) => *error = Some(err.to_string()),
                    }
                }
                field::Response::Cancel => self.vcs = None,
                field::Response::Paste => cx.paste(&TEXT_TYPES),
                _ => *error = None,
            },
            Vcs::Clone(field, error) => match field.key(key) {
                field::Response::Submit => {
                    let url = field.text().trim().to_owned();
                    if url.is_empty() {
                        *error = Some("The repository's address goes here".to_owned());
                    } else {
                        self.vcs = None;
                        self.toast("Cloning…", cx);
                        let into = self.pane.location().to_owned();
                        self.remote(Remote::Clone { url, into });
                    }
                }
                field::Response::Cancel => self.vcs = None,
                field::Response::Paste => cx.paste(&TEXT_TYPES),
                _ => *error = None,
            },
        }
        cx.redraw();
        true
    }

    /// Text pasted while a form is open.
    pub(super) fn vcs_paste(&mut self, text: &str) -> bool {
        match &mut self.vcs {
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
            _ => false,
        }
    }

    fn answer_git(&mut self, on: bool, cx: &mut Cx<'_>) {
        self.vcs = None;
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
        if self.vcs.is_none() {
            return false;
        }
        match hit {
            Some(Hit::GitYes) => self.answer_git(true, cx),
            Some(Hit::GitNo) => self.answer_git(false, cx),
            Some(Hit::FormField(index)) => {
                if let Some(Vcs::Identity(edit)) = &mut self.vcs {
                    edit.focus = *index;
                }
            }
            Some(Hit::FormSave) => match &self.vcs {
                Some(Vcs::Identity(_)) => self.save_identity(cx),
                Some(Vcs::NewBranch(..) | Vcs::Clone(..)) => {
                    let enter = KeyEvent {
                        key: Key::Enter,
                        mods: Default::default(),
                        repeat: false,
                    };
                    self.vcs_key(&enter, cx);
                }
                _ => {}
            },
            Some(Hit::FormCancel) | None => self.vcs = None,
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
        let Some(vcs) = &self.vcs else { return };
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let title_size = self.font(type_scale::TITLE);
        let small = self.font(type_scale::SMALL);
        self.hits.clear();
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
            Vcs::Clone(..) => (
                "Clone a repository".to_owned(),
                "Its address, from GitHub or anywhere: copied into this folder, under its own name.".to_owned(),
                1,
                [("Cancel", Kind::Secondary, Hit::FormCancel), ("Clone", Kind::Primary, Hit::FormSave)],
            ),
        };
        let height = 150.0 + fields as f32 * 56.0;
        let inner = controls::dialog(canvas, bounds, 480.0, height, &look, 1.0);
        canvas.text(
            &Text::new(&title, title_size, colors.text)
                .medium()
                .max_width(inner.w),
            inner.x,
            inner.y,
        );
        canvas.text(
            &Text::new(&detail, small, colors.subtle)
                .max_width(inner.w)
                .max_lines(2),
            inner.x,
            inner.y + 30.0,
        );

        let style = |placeholder| field::Style {
            size: look.size,
            color: colors.text,
            placeholder,
            placeholder_color: colors.muted,
            caret: accent,
            selection: accent.alpha(opacity::TINT * 2.0),
        };
        let mut y = inner.y + 78.0;
        let mut error = None;
        let mut field_rects = Vec::new();
        // A one-field form's example.
        let single_placeholder = if matches!(self.vcs, Some(Vcs::Clone(..))) {
            "https://github.com/owner/repository"
        } else {
            "feature/quiet-mode"
        };
        match &mut self.vcs {
            Some(Vcs::Identity(edit)) => {
                for (index, (label, placeholder)) in
                    [("Name", "Your name"), ("Email", "you@example.com")]
                        .into_iter()
                        .enumerate()
                {
                    canvas.text_in(
                        &Text::new(label, small, colors.subtle),
                        Rect::new(inner.x, y, 64.0, controls::HEIGHT),
                        Align::Start,
                    );
                    let rect = Rect::new(inner.x + 64.0, y, inner.w - 64.0, controls::HEIGHT);
                    let focused = edit.focus == index;
                    canvas.fill(rect, radius::pill(rect.h), colors.base);
                    canvas.stroke(
                        rect,
                        radius::pill(rect.h),
                        stroke::HAIRLINE,
                        if focused {
                            accent
                        } else {
                            colors.highlight_med
                        },
                    );
                    let text = rect.inset_xy(space::LG, 0.0);
                    let field = if index == 0 {
                        &mut edit.name
                    } else {
                        &mut edit.email
                    };
                    field.draw(canvas, text, &style(placeholder), focused);
                    field_rects.push((rect, Hit::FormField(index)));
                    y += 48.0;
                }
                error = edit.error.clone();
            }
            Some(Vcs::NewBranch(field, problem) | Vcs::Clone(field, problem)) => {
                let rect = Rect::new(inner.x, y, inner.w, controls::HEIGHT);
                canvas.fill(rect, radius::pill(rect.h), colors.base);
                canvas.stroke(rect, radius::pill(rect.h), stroke::HAIRLINE, accent);
                field.draw(
                    canvas,
                    rect.inset_xy(space::LG, 0.0),
                    &style(single_placeholder),
                    true,
                );
                error = problem.clone();
            }
            _ => {}
        }
        self.hits.extend(field_rects);
        if let Some(error) = error {
            canvas.text(
                &Text::new(&error, small, colors.love).max_width(inner.w),
                inner.x,
                inner.bottom() - controls::HEIGHT - 26.0,
            );
        }
        let mut right = inner.right();
        for (label, kind, hit) in buttons.into_iter().rev() {
            let button = Button::new(label, kind);
            let width = button.width(canvas, &look);
            let rect = Rect::new(
                right - width,
                inner.bottom() - controls::HEIGHT,
                width,
                controls::HEIGHT,
            );
            button.draw(
                canvas,
                rect,
                &look,
                Interaction {
                    hovered: self.hover.as_ref() == Some(&hit),
                    ..Interaction::default()
                },
            );
            self.hits.push((rect, hit));
            right = rect.x - space::SM;
        }
    }
}
