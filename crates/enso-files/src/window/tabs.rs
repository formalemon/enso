//! Tabs (`Ctrl+T`, `Ctrl+W`, `Ctrl+Tab`, `Alt+1…9`) and the split view
//! (`F3`): several folders open at once, two of them side by side.
//!
//! A tab is everything about the folder it shows — the listing, where the
//! cursor and the scroll are, the filter, the watch on the folder. The tab
//! in front keeps all that in the window itself, so the rest of the window
//! needs to know nothing about tabs; the others are parked here, and
//! switching swaps one in. The split's other side is drawn the same way:
//! swapped in for the moment it takes to draw it, then parked again.
//!
//! In the split, `Tab` moves between the sides, and `F5` and `F6` copy and
//! move what's chosen to the other side (Midnight Commander's keys).

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use enso_core::{
    motion::Tween,
    style::{icon, radius, space, stroke, type_scale},
};
use enso_ui::{Align, Canvas, Rect, Text, widget::Scroll};

use super::{Files, Hit, band, columns, grid, preview};
use crate::{
    jobs::{Loader, Op},
    pane::{Pane, Request},
};

/// The tab strip's height, when there's more than one tab.
pub(super) const STRIP: f32 = 34.0;
/// A split side's header.
const HEADER: f32 = 26.0;
/// Tabs open at most: past this, a new one replaces nothing and says so.
const MOST: usize = 24;

/// A parked tab.
#[derive(Debug)]
pub(super) struct Tab {
    pane: Pane,
    loader: Loader,
    scroll: Scroll,
    highlight: Tween<f32>,
    anchor: Option<usize>,
    filter: Option<enso_ui::widget::TextField>,
    watch: Option<enso_fs::watch::Watch>,
    list: Rect,
    repo: Option<(PathBuf, enso_fs::git::Status)>,
    thumbs: grid::Thumbs,
    band: Option<band::Band>,
    arrival: Tween<f32>,
    sliding: f32,
    slide_pending: bool,
    side_parent: columns::Side,
    side_child: columns::Side,
    shown: preview::Shown,
    shown_at: PathBuf,
}

impl Tab {
    fn new(pane: Pane) -> Self {
        Self {
            pane,
            loader: Loader::default(),
            scroll: Scroll::default(),
            highlight: Tween::new(0.0),
            anchor: None,
            filter: None,
            watch: None,
            list: Rect::default(),
            repo: None,
            thumbs: grid::Thumbs::default(),
            band: None,
            arrival: Tween::new(1.0),
            sliding: 0.0,
            slide_pending: false,
            side_parent: columns::Side::default(),
            side_child: columns::Side::default(),
            shown: preview::Shown::default(),
            shown_at: PathBuf::new(),
        }
    }

    pub(super) fn location(&self) -> &Path {
        self.pane.location()
    }

    pub(super) fn generation(&self) -> u64 {
        self.pane.generation()
    }
}

/// The tab strip's and the split's places, for the pointer.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum TabHit {
    Tab(usize),
    Close(usize),
}

impl Files {
    /// Trade the window's folder for `tab`'s.
    fn swap_tab(&mut self, tab: &mut Tab) {
        use std::mem::swap;
        swap(&mut self.pane, &mut tab.pane);
        swap(&mut self.loader, &mut tab.loader);
        swap(&mut self.scroll, &mut tab.scroll);
        swap(&mut self.highlight, &mut tab.highlight);
        swap(&mut self.anchor, &mut tab.anchor);
        swap(&mut self.filter, &mut tab.filter);
        swap(&mut self.watch, &mut tab.watch);
        swap(&mut self.list, &mut tab.list);
        swap(&mut self.repo, &mut tab.repo);
        swap(&mut self.thumbs, &mut tab.thumbs);
        swap(&mut self.band, &mut tab.band);
        swap(&mut self.arrival, &mut tab.arrival);
        swap(&mut self.sliding, &mut tab.sliding);
        swap(&mut self.slide_pending, &mut tab.slide_pending);
        swap(&mut self.side_parent, &mut tab.side_parent);
        swap(&mut self.side_child, &mut tab.side_child);
        swap(&mut self.shown, &mut tab.shown);
        swap(&mut self.shown_at, &mut tab.shown_at);
    }

    /// Do `f` with tab `index` in front, then put things back.
    pub(super) fn with_tab<R>(&mut self, index: usize, f: impl FnOnce(&mut Self) -> R) -> R {
        if index == self.active {
            return f(self);
        }
        let Some(mut tab) = self.tabs.get_mut(index).and_then(Option::take) else {
            return f(self);
        };
        self.swap_tab(&mut tab);
        let result = f(self);
        self.swap_tab(&mut tab);
        self.tabs[index] = Some(tab);
        result
    }

    /// The tab that asked for `generation`'s listing, if one is still
    /// waiting on it.
    pub(super) fn tab_of(&self, generation: u64) -> Option<usize> {
        if self.pane.generation() == generation {
            return Some(self.active);
        }
        self.tabs.iter().position(|tab| {
            tab.as_ref()
                .is_some_and(|tab| tab.generation() == generation)
        })
    }

    /// The tabs showing `folder`.
    pub(super) fn tabs_at(&self, folder: &Path) -> Vec<usize> {
        (0..self.tabs.len())
            .filter(|&index| self.tab_location(index) == folder)
            .collect()
    }

    pub(super) fn tab_location(&self, index: usize) -> &Path {
        match self.tabs.get(index).and_then(Option::as_ref) {
            Some(tab) => tab.location(),
            None => self.pane.location(),
        }
    }

    fn tab_label(&self, index: usize) -> String {
        let path = self.tab_location(index);
        if path == self.home {
            return "Home".to_owned();
        }
        path.file_name().map_or_else(
            || "/".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// Open `path` in a new tab after this one; in front, if `show`.
    /// Returns false if there are as many tabs as there can be.
    pub(super) fn new_tab(&mut self, path: PathBuf, show: bool) -> bool {
        if self.tabs.len() >= MOST {
            return false;
        }
        let (pane, request) = Pane::new(path, self.config.sort(), self.config.hidden);
        let mut tab = Tab::new(pane);
        if let Some(keep) = self.picker.as_ref().and_then(|picker| picker.keep()) {
            tab.pane.set_keep(Some(keep));
        }
        let at = self.active + 1;
        self.tabs.insert(at, Some(tab));
        if let Some(split) = &mut self.split
            && *split >= at
        {
            *split += 1;
        }
        self.with_tab(at, |files| files.start_loading(request));
        if show {
            self.switch_tab(at);
        }
        true
    }

    /// [`Files::new_tab`], saying so if there's no room.
    pub(super) fn open_tab(&mut self, path: PathBuf, show: bool, cx: &mut enso_ui::Cx<'_>) {
        if !self.new_tab(path, show) {
            self.toast(format!("{MOST} tabs is as many as there can be"), cx);
        }
    }

    /// Start listing in the tab in front, whose request this is.
    fn start_loading(&mut self, request: Request) {
        if let Some(proxy) = &self.proxy {
            self.loader.load(request, proxy);
        }
    }

    pub(super) fn switch_tab(&mut self, index: usize) {
        if index == self.active || index >= self.tabs.len() {
            return;
        }
        let Some(mut tab) = self.tabs[index].take() else {
            return;
        };
        self.swap_tab(&mut tab);
        self.tabs[self.active] = Some(tab);
        // The side that was in front goes to the other side of the split.
        if self.split == Some(index) {
            self.split = Some(self.active);
        }
        self.active = index;
        let here = self.pane.location().to_owned();
        self.apply_folder_view(&here);
        self.hover = None;
        self.menu = None;
        self.renaming = None;
        if let Some(thumbnailer) = &self.thumbnailer {
            thumbnailer.clear();
        }
    }

    /// The next tab (`1`) or the one before (`-1`), round.
    pub(super) fn step_tab(&mut self, step: isize) {
        let count = self.tabs.len() as isize;
        if count > 1 {
            let next = (self.active as isize + step).rem_euclid(count);
            self.switch_tab(next as usize);
        }
    }

    /// Close tab `index`. Returns false if it's the last: the window goes.
    pub(super) fn close_tab(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 || index >= self.tabs.len() {
            return false;
        }
        if index == self.active {
            // The one after takes its place; the last tab's, the one before.
            let next = if index + 1 < self.tabs.len() {
                index + 1
            } else {
                index - 1
            };
            self.switch_tab(next);
        }
        self.tabs.remove(index);
        if self.active > index {
            self.active -= 1;
        }
        self.split = match self.split {
            Some(split) if split == index => None,
            Some(split) if split > index => Some(split - 1),
            other => other,
        };
        if self.split == Some(self.active) {
            self.split = None;
        }
        true
    }

    /// `F3`: split the window, the other side on the next tab (a new one
    /// here, if there isn't one); or put the split away.
    pub(super) fn toggle_split(&mut self) {
        if self.split.take().is_some() {
            return;
        }
        if self.tabs.len() == 1 {
            let here = self.pane.location().to_owned();
            self.new_tab(here, false);
        }
        if self.tabs.len() == 1 {
            return;
        }
        self.split = Some((self.active + 1) % self.tabs.len());
    }

    /// `Tab` in the split: the other side comes in front.
    pub(super) fn other_side(&mut self) {
        if let Some(other) = self.split {
            self.switch_tab(other);
        }
    }

    /// `F5` / `F6`: copy or move what's chosen to the other side's folder.
    pub(super) fn send_across(&mut self, moving: bool, cx: &mut enso_ui::Cx<'_>) {
        let Some(other) = self.split else { return };
        let into = self.tab_location(other).to_owned();
        let sources = self.targets();
        if sources.is_empty() {
            return;
        }
        if into == self.pane.location() {
            self.toast("Both sides show this folder", cx);
            return;
        }
        let op = if moving {
            Op::Move { sources, into }
        } else {
            Op::Copy { sources, into }
        };
        if moving {
            self.selection.clear();
        }
        self.start_op(op, cx);
    }

    /// The strip of tabs along the top of `area`.
    pub(super) fn draw_tabs(&mut self, canvas: &mut Canvas<'_>, area: Rect) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let small = self.font(type_scale::SMALL);
        canvas.fill(
            Rect::new(
                area.x,
                area.bottom() - stroke::HAIRLINE,
                area.w,
                stroke::HAIRLINE,
            ),
            0.0,
            colors.highlight_low,
        );
        let count = self.tabs.len();
        let gap = space::XS;
        let width = ((area.w - 2.0 * space::SM - gap * (count as f32 - 1.0)) / count as f32)
            .clamp(64.0, 200.0);
        let mut x = area.x + space::SM;
        for index in 0..count {
            let rect = Rect::new(x, area.y + 4.0, width, area.h - 8.0);
            let front = index == self.active;
            let hovered = matches!(&self.hover, Some(Hit::Tabs(TabHit::Tab(i) | TabHit::Close(i))) if *i == index);
            if front {
                canvas.fill(rect, radius::CONTROL, colors.surface);
                canvas.fill(
                    Rect::new(
                        rect.x + space::MD,
                        rect.bottom() - 2.0,
                        rect.w - 2.0 * space::MD,
                        2.0,
                    ),
                    1.0,
                    accent,
                );
            } else if hovered {
                canvas.fill(rect, radius::CONTROL, colors.highlight_low);
            }
            let other_side = self.split == Some(index);
            let ink = if front {
                colors.text
            } else if other_side {
                colors.subtle
            } else {
                colors.muted
            };
            let close = Rect::new(rect.right() - 24.0, rect.y, 20.0, rect.h);
            let label = Rect::new(
                rect.x + space::MD,
                rect.y,
                rect.w - 2.0 * space::MD - if front || hovered { 16.0 } else { 0.0 },
                rect.h,
            );
            let name = self.tab_label(index);
            let text = Text::new(&name, small, ink).max_width(label.w);
            canvas.text_in(&text, label, Align::Start);
            self.hits.push((rect, Hit::Tabs(TabHit::Tab(index))));
            if front || hovered {
                let over = self.hover == Some(Hit::Tabs(TabHit::Close(index)));
                canvas.icon(
                    icon::CLOSE,
                    12.0,
                    if over { colors.text } else { colors.muted },
                    close,
                    Align::Center,
                );
                self.hits.push((close, Hit::Tabs(TabHit::Close(index))));
            }
            x = rect.right() + gap;
        }
    }

    /// The split: this tab on the left, the other on the right, each with
    /// its folder's name on top. Returns whether anything is moving.
    pub(super) fn draw_split(&mut self, canvas: &mut Canvas<'_>, area: Rect, now: Instant) -> bool {
        let Some(other) = self.split else {
            return self.draw_body(canvas, area, now, true);
        };
        let colors = self.ctx.theme.colors.clone();
        let half = (area.w / 2.0).floor();
        let left = Rect::new(area.x, area.y, half, area.h);
        let right = Rect::new(area.x + half, area.y, area.w - half, area.h);
        // The tab in front stays on the side it was on.
        let (here, there) = if self.active < other {
            (left, right)
        } else {
            (right, left)
        };
        let mut animating = false;
        self.draw_header(canvas, here, self.active, true);
        animating |= self.draw_body(canvas, body_of(here), now, true);
        let start = self.hits.len();
        self.draw_header(canvas, there, other, false);
        animating |= self.with_tab(other, |files| {
            let mut moving = false;
            canvas.with_opacity(0.8, |canvas| {
                moving = files.draw_body(canvas, body_of(there), now, false);
            });
            moving
        });
        // What's on the other side brings it in front first.
        let theirs: Vec<(Rect, Hit)> = self.hits.drain(start..).collect();
        // Its header: a click there only brings it in front.
        self.hits
            .push((there, Hit::Other(Box::new(Hit::Tabs(TabHit::Tab(other))))));
        self.hits.extend(
            theirs
                .into_iter()
                .map(|(rect, hit)| (rect, Hit::Other(Box::new(hit)))),
        );
        self.split_rect = there;
        canvas.fill(
            Rect::new(right.x, area.y, stroke::HAIRLINE, area.h),
            0.0,
            colors.highlight_med,
        );
        animating
    }

    fn draw_header(&mut self, canvas: &mut Canvas<'_>, side: Rect, index: usize, front: bool) {
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let rect = Rect::new(side.x, side.y, side.w, HEADER);
        canvas.fill(rect, 0.0, colors.surface.mix(colors.base, 0.5));
        let small = self.font(type_scale::SMALL);
        let dot = Rect::new(rect.x + space::MD, rect.y, 8.0, rect.h);
        if front {
            canvas.circle(dot.x + 3.0, rect.y + rect.h / 2.0, 3.0, accent);
        }
        let label = Rect::new(
            dot.right() + space::XS,
            rect.y,
            rect.w - 3.0 * space::MD,
            rect.h,
        );
        let path = short(self.tab_location(index), &self.home);
        let ink = if front { colors.text } else { colors.muted };
        canvas.text_in(
            &Text::new(&path, small, ink).max_width(label.w),
            label,
            Align::Start,
        );
    }
}

/// A path as it's said: from `~` when it's under home.
pub(super) fn short(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// A split side, under its header.
fn body_of(side: Rect) -> Rect {
    Rect::new(side.x, side.y + HEADER, side.w, side.h - HEADER)
}
