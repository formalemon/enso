//! Input methods (`zwp_text_input_v3`): typing Japanese, Chinese, Korean,
//! or anything else that is composed before it is committed.
//!
//! The input method is only asked for while a text field has focus: a
//! focused [`TextField`](crate::widget::TextField) says where its caret is
//! as it draws ([`Canvas::text_input_at`](crate::Canvas::text_input_at)),
//! and after each frame the runtime enables text input, moves the caret
//! rectangle the candidate window hangs from, or disables it again. A
//! password field never says so, so what is typed into one never passes
//! through an input method.
//!
//! What is being composed — the *preedit* — is shown in the field, at the
//! caret, underlined ([`Canvas::preedit`](crate::Canvas::preedit)). What
//! the input method commits arrives as the plain [`Key::Char`] presses
//! that would have typed it, so every text box takes it the way it takes
//! typing, with no new event to route.
//!
//! No surrounding text is offered, so input methods have no cause to ask
//! for text around the caret to be deleted, and such requests are ignored.

use std::ops::Range;

use smithay_client_toolkit::{
    dispatch2::Dispatch2,
    reexports::{
        client::{Connection, QueueHandle, protocol::wl_surface::WlSurface},
        protocols::wp::text_input::zv3::client::{
            zwp_text_input_manager_v3::ZwpTextInputManagerV3,
            zwp_text_input_v3::{self, ContentHint, ContentPurpose, ZwpTextInputV3},
        },
    },
};

use crate::{Key, KeyEvent, Mods, Rect};

/// Text an input method is composing, not yet committed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    /// The input method's cursor in `text`, in bytes: a caret where the
    /// range is empty, a highlight where it isn't, `None` for no cursor.
    pub cursor: Option<Range<usize>>,
}

impl Preedit {
    /// From the protocol's event: a null or empty string is no preedit,
    /// and a cursor off a character boundary is no cursor.
    fn from_event(text: Option<String>, begin: i32, end: i32) -> Option<Self> {
        let text = text.filter(|text| !text.is_empty())?;
        let at = |i: i32| {
            usize::try_from(i)
                .ok()
                .filter(|&i| i <= text.len() && text.is_char_boundary(i))
        };
        let cursor = at(begin)
            .zip(at(end))
            .map(|(begin, end)| begin.min(end)..begin.max(end));
        Some(Self { text, cursor })
    }
}

/// What arrives between two `done` events, applied together at the second.
#[derive(Debug, Default)]
struct Pending {
    preedit: Option<Preedit>,
    commit: Option<String>,
}

/// A request for the compositor, made when what the window wants changes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Request {
    /// Start composing; the caret is here.
    Enable(Rect),
    /// The caret moved.
    Move(Rect),
    Disable,
}

/// What a `done` event changed.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Done {
    /// Text to type into the field.
    pub commit: Option<String>,
    /// The preedit shown changed.
    pub redraw: bool,
}

/// The state of composition for one surface, kept apart from Wayland so
/// it can be tested without a compositor.
#[derive(Debug, Default)]
pub(crate) struct Composition {
    /// The seat's text input is on our surface (`enter`, until `leave`).
    entered: bool,
    /// Where the focused field's caret was in the last frame, if one was.
    wanted: Option<Rect>,
    /// What the compositor was last told: enabled, with this caret.
    enabled: Option<Rect>,
    pending: Pending,
    preedit: Option<Preedit>,
}

impl Composition {
    /// What is being composed, for the focused field to show.
    pub fn preedit(&self) -> Option<&Preedit> {
        self.preedit.as_ref()
    }

    /// The text input came to our surface.
    pub fn enter(&mut self) -> Option<Request> {
        self.entered = true;
        self.sync()
    }

    /// It left. The compositor ignores requests until it is back, and the
    /// preedit is gone. Returns whether one was shown.
    pub fn leave(&mut self) -> bool {
        self.entered = false;
        self.enabled = None;
        self.pending = Pending::default();
        self.preedit.take().is_some()
    }

    /// A frame was drawn; a focused field's caret was at `caret`, or no
    /// field had focus.
    pub fn drawn(&mut self, caret: Option<Rect>) -> Option<Request> {
        self.wanted = caret;
        self.sync()
    }

    fn sync(&mut self) -> Option<Request> {
        match (self.entered.then_some(self.wanted).flatten(), self.enabled) {
            (Some(caret), None) => {
                self.enabled = Some(caret);
                Some(Request::Enable(caret))
            }
            (Some(caret), Some(was)) if !same_place(caret, was) => {
                self.enabled = Some(caret);
                Some(Request::Move(caret))
            }
            (None, Some(_)) => {
                self.enabled = None;
                // Composition stops with the field's focus; what was
                // being composed goes with it.
                self.preedit = None;
                self.pending = Pending::default();
                Some(Request::Disable)
            }
            _ => None,
        }
    }

    pub fn preedit_string(&mut self, text: Option<String>, begin: i32, end: i32) {
        self.pending.preedit = Preedit::from_event(text, begin, end);
    }

    pub fn commit_string(&mut self, text: Option<String>) {
        self.pending.commit = text.filter(|text| !text.is_empty());
    }

    /// Apply what arrived since the last `done`. A preedit not sent again
    /// is a preedit that's over.
    pub fn done(&mut self) -> Done {
        let pending = std::mem::take(&mut self.pending);
        // Ignored while disabled: a late answer to a field that has
        // lost focus mustn't type into whatever has it now.
        if self.enabled.is_none() {
            return Done::default();
        }
        let redraw = pending.preedit != self.preedit;
        self.preedit = pending.preedit;
        Done {
            commit: pending.commit,
            redraw,
        }
    }
}

/// The caret rectangle in the protocol's whole pixels.
pub(crate) fn whole(rect: Rect) -> (i32, i32, i32, i32) {
    (
        rect.x.round() as i32,
        rect.y.round() as i32,
        (rect.w.round() as i32).max(1),
        (rect.h.round() as i32).max(1),
    )
}

fn same_place(a: Rect, b: Rect) -> bool {
    whole(a) == whole(b)
}

/// The key presses that type `text`: one plain [`Key::Char`] for each
/// character, leaving out control characters.
pub(crate) fn keys(text: &str) -> impl Iterator<Item = KeyEvent> + '_ {
    text.chars().filter(|c| !c.is_control()).map(|c| KeyEvent {
        key: Key::Char(c),
        mods: Mods::default(),
        repeat: false,
    })
}

/// Send `request` on `input`.
pub(crate) fn send(input: &ZwpTextInputV3, request: Request) {
    let place = |rect| {
        let (x, y, w, h) = whole(rect);
        input.set_cursor_rectangle(x, y, w, h);
    };
    match request {
        Request::Enable(caret) => {
            input.enable();
            input.set_content_type(ContentHint::None, ContentPurpose::Normal);
            place(caret);
        }
        Request::Move(caret) => place(caret),
        Request::Disable => input.disable(),
    }
    input.commit();
}

/// User data for the text-input manager global: nothing.
pub(crate) struct Manager;

/// User data for a seat's text input.
pub(crate) struct TextInputData;

/// A runtime that composes text on one surface.
pub(crate) trait Composing {
    fn is_own(&self, surface: &WlSurface) -> bool;
    fn composition(&mut self) -> &mut Composition;
    /// Send `request` to the compositor.
    fn request(&mut self, request: Request);
    /// Apply what a `done` changed.
    fn composed(&mut self, done: Done);
}

impl<D: 'static> Dispatch2<ZwpTextInputManagerV3, D> for Manager {
    fn event(
        &self,
        _: &mut D,
        _: &ZwpTextInputManagerV3,
        _: <ZwpTextInputManagerV3 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D: Composing + 'static> Dispatch2<ZwpTextInputV3, D> for TextInputData {
    fn event(
        &self,
        state: &mut D,
        _: &ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
        use zwp_text_input_v3::Event;
        match event {
            Event::Enter { surface } if state.is_own(&surface) => {
                if let Some(request) = state.composition().enter() {
                    state.request(request);
                }
            }
            Event::Leave { surface } if state.is_own(&surface) => {
                let redraw = state.composition().leave();
                state.composed(Done {
                    commit: None,
                    redraw,
                });
            }
            Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => state
                .composition()
                .preedit_string(text, cursor_begin, cursor_end),
            Event::CommitString { text } => state.composition().commit_string(text),
            Event::Done { .. } => {
                let done = state.composition().done();
                state.composed(done);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARET: Rect = Rect {
        x: 20.0,
        y: 10.0,
        w: 2.0,
        h: 18.0,
    };

    #[test]
    fn enabled_only_while_a_field_has_focus() {
        let mut ime = Composition::default();
        // A frame with a focused field, before the text input arrives.
        assert_eq!(ime.drawn(Some(CARET)), None);
        assert_eq!(ime.enter(), Some(Request::Enable(CARET)));
        // The same caret again is nothing new; less than a pixel neither.
        assert_eq!(ime.drawn(Some(CARET)), None);
        let nudged = Rect { x: 20.2, ..CARET };
        assert_eq!(ime.drawn(Some(nudged)), None);
        let moved = Rect { x: 40.0, ..CARET };
        assert_eq!(ime.drawn(Some(moved)), Some(Request::Move(moved)));
        // Focus went to a list, or to a password field.
        assert_eq!(ime.drawn(None), Some(Request::Disable));
        assert_eq!(ime.drawn(None), None);
        assert_eq!(ime.drawn(Some(CARET)), Some(Request::Enable(CARET)));
    }

    #[test]
    fn leaving_forgets_the_preedit() {
        let mut ime = Composition::default();
        ime.drawn(Some(CARET));
        ime.enter();
        ime.preedit_string(Some("にほ".into()), 6, 6);
        assert!(ime.done().redraw);
        assert!(ime.leave());
        assert_eq!(ime.preedit(), None);
        // Back again: enabled anew.
        assert_eq!(ime.enter(), Some(Request::Enable(CARET)));
    }

    #[test]
    fn composing_then_committing() {
        let mut ime = Composition::default();
        ime.drawn(Some(CARET));
        ime.enter();

        ime.preedit_string(Some("にほ".into()), 6, 6);
        let done = ime.done();
        assert_eq!(done.commit, None);
        assert!(done.redraw);
        assert_eq!(
            ime.preedit(),
            Some(&Preedit {
                text: "にほ".into(),
                cursor: Some(6..6),
            })
        );

        // The same preedit again changes nothing on screen.
        ime.preedit_string(Some("にほ".into()), 6, 6);
        assert!(!ime.done().redraw);

        // Converted and committed: the preedit, not sent again, is over.
        ime.commit_string(Some("日本".into()));
        let done = ime.done();
        assert_eq!(done.commit.as_deref(), Some("日本"));
        assert!(done.redraw);
        assert_eq!(ime.preedit(), None);
    }

    #[test]
    fn nothing_applies_while_disabled() {
        let mut ime = Composition::default();
        ime.enter();
        ime.commit_string(Some("x".into()));
        assert_eq!(ime.done(), Done::default());
        // And it doesn't linger for when a field takes focus.
        ime.drawn(Some(CARET));
        assert_eq!(ime.done(), Done::default());
    }

    #[test]
    fn preedit_cursors() {
        let cursor = |begin, end| {
            Preedit::from_event(Some("日本".into()), begin, end).map(|preedit| preedit.cursor)
        };
        assert_eq!(cursor(3, 3), Some(Some(3..3)));
        assert_eq!(cursor(6, 0), Some(Some(0..6)));
        // Hidden, inside a character, or past the end: no cursor.
        assert_eq!(cursor(-1, -1), Some(None));
        assert_eq!(cursor(1, 1), Some(None));
        assert_eq!(cursor(0, 9), Some(None));
        assert_eq!(Preedit::from_event(Some(String::new()), 0, 0), None);
        assert_eq!(Preedit::from_event(None, 0, 0), None);
    }

    #[test]
    fn commits_type_as_keys() {
        let typed: String = keys("日本\n語").filter_map(|key| key.text()).collect();
        assert_eq!(typed, "日本語");
    }
}
