//! Search (`Ctrl+F`, or `//`): everything below this folder whose name
//! matches, as you type; and inside files (`Ctrl+Shift+F`), when Enter is
//! pressed — reading every file on each keystroke would be too slow to
//! feel like typing. Both take the query words of [`crate::query`].
//!
//! The field is the filter's, in a second mode. The results are a list
//! like a folder's, each with where it is; Enter or a click opens one,
//! "Show in its folder" goes there, Esc goes back to the folder searched.

use std::time::{Duration, Instant};

use enso_ui::{Cx, widget::TextField};

use super::Files;
use crate::pane::Source;

/// How long typing rests before a name search runs.
const SETTLE: Duration = Duration::from_millis(250);

/// The search field's mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Seek {
    Names,
    Contents,
}

impl Files {
    /// Open the field to search, by name or inside files.
    pub(super) fn open_search(&mut self, seek: Seek, cx: &mut Cx<'_>) {
        // Refining a search starts from its words.
        let text = match self.pane.source() {
            Source::Search { query, .. } => query.clone(),
            _ => String::new(),
        };
        let mut field = TextField::new(&text);
        field.select_all();
        self.filter = Some(field);
        self.seek = Some(seek);
        self.seek_due = None;
        if let Some(picker) = &mut self.picker {
            picker.typing_name = false;
        }
        cx.redraw();
    }

    /// The query was typed into: a name search runs once typing rests.
    pub(super) fn search_typed(&mut self, now: Instant, cx: &mut Cx<'_>) {
        if self.seek == Some(Seek::Names) {
            self.seek_due = Some(now + SETTLE);
            cx.wake_in(SETTLE);
        }
    }

    /// Run the search if one is due. Returns whether it ran.
    pub(super) fn search_if_due(&mut self, now: Instant) -> bool {
        match self.seek_due {
            Some(due) if now + Duration::from_millis(10) >= due => {
                self.seek_due = None;
                self.run_search();
                true
            }
            _ => false,
        }
    }

    /// Search for what the field holds, below the folder in front (or the
    /// one the results in front came from).
    pub(super) fn run_search(&mut self) {
        let Some(seek) = self.seek else { return };
        let query = self
            .filter
            .as_ref()
            .map(|field| field.text().trim().to_owned())
            .unwrap_or_default();
        if query.is_empty() {
            // Nothing asked: back to the folder, if results were up.
            if let Source::Search { .. } = self.pane.source() {
                let folder = self.pane.location().to_owned();
                self.navigate(folder);
            }
            return;
        }
        let source = Source::Search {
            query,
            content: seek == Seek::Contents,
        };
        if *self.pane.source() == source {
            return;
        }
        let request = match self.pane.source() {
            // A search refined replaces itself; Back goes to the folder.
            Source::Search { .. } => Some(self.pane.replace(source)),
            Source::Recent => self.pane.show(self.home.clone(), source),
            // From inside an archive: the folder it's in.
            Source::Archive { .. } => {
                let folder = self
                    .pane
                    .location()
                    .parent()
                    .unwrap_or(&self.home)
                    .to_owned();
                self.pane.show(folder, source)
            }
            Source::Folder => {
                let here = self.pane.location().to_owned();
                self.pane.show(here, source)
            }
        };
        self.load(request);
    }

    /// Put the field away; the results stay.
    pub(super) fn close_search(&mut self) {
        self.filter = None;
        self.seek = None;
        self.seek_due = None;
    }

    /// What the field says while it's empty.
    pub(super) fn search_hint(&self) -> String {
        let folder = match self.pane.source() {
            Source::Recent => "~".to_owned(),
            _ => super::tabs::short(self.pane.location(), &self.home),
        };
        match self.seek {
            Some(Seek::Names) => format!("Search {folder}"),
            Some(Seek::Contents) => format!("Search inside files in {folder}"),
            None => "Filter".to_owned(),
        }
    }
}
