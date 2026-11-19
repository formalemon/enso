//! A one-line text field: the filter box, the location bar, a file's new
//! name.
//!
//! Editing is plain data — a string, a caret, an anchor for the selection,
//! and an undo history — so every key's meaning is tested without a window.
//! Drawing measures the text with the canvas, keeps the caret in view by
//! sliding the text sideways, and paints the selection under it.
//!
//! Keys are the ones every text box on Linux answers to: arrows (with Ctrl
//! by word, with Shift selecting), Home/End, Backspace/Delete (with Ctrl by
//! word), Ctrl+A, the readline kills (Ctrl+U, Ctrl+W, Ctrl+K), and
//! Ctrl+Z/Ctrl+Shift+Z. The clipboard belongs to the window, so copying
//! and pasting are handed back as [`Response`]s.
//!
//! The caret doesn't blink: a blinking caret is a timer that wakes the
//! program twice a second for nothing.

use std::ops::Range;

use enso_core::{
    Rgba,
    style::{opacity, radius, space, stroke},
};

use super::Look;
use crate::{Canvas, Key, KeyEvent, Rect, Text};

/// Height of a one-line field in a dialog or a card.
pub const HEIGHT: f32 = 40.0;

/// Room between a field's edge and what's typed in it.
pub const INSET: f32 = space::LG;

/// What a key did to the field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// Not a key for a text field; the window may use it.
    Ignored,
    /// The caret or selection moved; the text is the same.
    Moved,
    /// The text changed.
    Changed,
    /// Enter.
    Submit,
    /// Escape.
    Cancel,
    /// Ctrl+C: put this on the clipboard.
    Copy(String),
    /// Ctrl+X: put this on the clipboard; it has already been cut.
    Cut(String),
    /// Ctrl+V: insert the clipboard with [`TextField::insert`].
    Paste,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct State {
    text: String,
    caret: usize,
    anchor: Option<usize>,
}

/// Steps of undo kept.
const HISTORY: usize = 100;

#[derive(Clone, Debug, Default)]
pub struct TextField {
    now: State,
    undo: Vec<State>,
    redo: Vec<State>,
    /// Whether the last change was typing, so a run of typed letters
    /// undoes as one word, not letter by letter.
    typing: bool,
    /// How far the text is slid left to keep the caret in view.
    offset: f32,
}

impl TextField {
    pub fn new(text: &str) -> Self {
        let mut field = Self::default();
        field.now.text = text.to_owned();
        field.now.caret = text.len();
        field
    }

    pub fn text(&self) -> &str {
        &self.now.text
    }

    pub fn is_empty(&self) -> bool {
        self.now.text.is_empty()
    }

    /// The caret, as a byte offset into [`text`](Self::text).
    pub fn caret(&self) -> usize {
        self.now.caret
    }

    /// The selected bytes, if any are.
    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.now.anchor?;
        let caret = self.now.caret;
        (anchor != caret).then(|| anchor.min(caret)..anchor.max(caret))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|range| &self.now.text[range])
    }

    /// Replace everything, as a fresh start: caret at the end, no history.
    pub fn set_text(&mut self, text: &str) {
        *self = Self::new(text);
    }

    /// Replace everything, undoably (Tab completion, say).
    pub fn replace_all(&mut self, text: &str) {
        self.checkpoint(false);
        self.now = State {
            text: text.to_owned(),
            caret: text.len(),
            anchor: None,
        };
    }

    pub fn select_all(&mut self) {
        self.now.anchor = Some(0);
        self.now.caret = self.now.text.len();
    }

    /// Select `range` (clamped to char boundaries), caret at its end.
    pub fn select(&mut self, range: Range<usize>) {
        let end = self.boundary(range.end);
        self.now.anchor = Some(self.boundary(range.start.min(end)));
        self.now.caret = end;
    }

    /// Select the name without its extension — what a rename or a save
    /// dialog offers to replace: `report` of `report.final.pdf`, all of
    /// `.bashrc` and of `Makefile`.
    pub fn select_stem(&mut self) {
        let text = &self.now.text;
        let end = match text.rfind('.') {
            Some(dot) if dot > 0 => dot,
            _ => text.len(),
        };
        self.select(0..end);
    }

    /// Type `text` where the caret is, over the selection.
    pub fn insert(&mut self, text: &str) {
        // A line of its own: what is pasted keeps to it.
        let text: String = text
            .chars()
            .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        let typing = text.chars().count() == 1 && self.selection().is_none();
        self.checkpoint(typing);
        self.delete_selection();
        self.now.text.insert_str(self.now.caret, &text);
        self.now.caret += text.len();
    }

    /// Handle a key press.
    pub fn key(&mut self, key: &KeyEvent) -> Response {
        let mods = key.mods;
        let select = mods.shift;
        match key.key {
            Key::Enter => Response::Submit,
            Key::Escape => Response::Cancel,
            Key::Left | Key::Right | Key::Home | Key::End => {
                let at = self.now.caret;
                let to = match key.key {
                    Key::Left if !select && !mods.ctrl && self.selection().is_some() => {
                        self.selection().map_or(at, |range| range.start)
                    }
                    Key::Right if !select && !mods.ctrl && self.selection().is_some() => {
                        self.selection().map_or(at, |range| range.end)
                    }
                    Key::Left if mods.ctrl => self.word_before(at),
                    Key::Right if mods.ctrl => self.word_after(at),
                    Key::Left => self.prev_char(at),
                    Key::Right => self.next_char(at),
                    Key::Home => 0,
                    _ => self.now.text.len(),
                };
                self.move_caret(to, select);
                self.typing = false;
                Response::Moved
            }
            Key::Backspace | Key::Delete if self.selection().is_some() => {
                self.checkpoint(false);
                self.delete_selection();
                Response::Changed
            }
            Key::Backspace => {
                let at = self.now.caret;
                let from = if mods.ctrl {
                    self.word_before(at)
                } else {
                    self.prev_char(at)
                };
                self.remove(from..at)
            }
            Key::Delete => {
                let at = self.now.caret;
                let to = if mods.ctrl {
                    self.word_after(at)
                } else {
                    self.next_char(at)
                };
                self.remove(at..to)
            }
            Key::Char(c) if mods.ctrl && !mods.alt => match c.to_ascii_lowercase() {
                'a' => {
                    self.select_all();
                    Response::Moved
                }
                'c' => match self.selected_text() {
                    Some(text) => Response::Copy(text.to_owned()),
                    None => Response::Ignored,
                },
                'x' => match self.selected_text().map(str::to_owned) {
                    Some(text) => {
                        self.checkpoint(false);
                        self.delete_selection();
                        Response::Cut(text)
                    }
                    None => Response::Ignored,
                },
                'v' => Response::Paste,
                'u' => {
                    let at = self.now.caret;
                    self.remove(0..at)
                }
                'w' => {
                    let at = self.now.caret;
                    let from = self.word_before(at);
                    self.remove(from..at)
                }
                'k' => {
                    let at = self.now.caret;
                    let end = self.now.text.len();
                    self.remove(at..end)
                }
                'z' if mods.shift => self.redo(),
                'z' => self.undo(),
                'y' => self.redo(),
                _ => Response::Ignored,
            },
            _ => match key.text() {
                Some(c) => {
                    self.insert(c.encode_utf8(&mut [0; 4]));
                    Response::Changed
                }
                None => Response::Ignored,
            },
        }
    }

    pub fn undo(&mut self) -> Response {
        match self.undo.pop() {
            Some(state) => {
                self.redo.push(std::mem::replace(&mut self.now, state));
                self.typing = false;
                Response::Changed
            }
            None => Response::Moved,
        }
    }

    pub fn redo(&mut self) -> Response {
        match self.redo.pop() {
            Some(state) => {
                self.undo.push(std::mem::replace(&mut self.now, state));
                self.typing = false;
                Response::Changed
            }
            None => Response::Moved,
        }
    }

    /// Remember the state before a change. Typing runs merge into one step.
    fn checkpoint(&mut self, typing: bool) {
        if !(typing && self.typing) {
            if self.undo.len() == HISTORY {
                self.undo.remove(0);
            }
            self.undo.push(self.now.clone());
        }
        self.typing = typing;
        self.redo.clear();
    }

    fn remove(&mut self, range: Range<usize>) -> Response {
        if range.is_empty() {
            return Response::Moved;
        }
        self.checkpoint(false);
        self.now.text.replace_range(range.clone(), "");
        self.now.caret = range.start;
        self.now.anchor = None;
        Response::Changed
    }

    fn delete_selection(&mut self) {
        if let Some(range) = self.selection() {
            self.now.text.replace_range(range.clone(), "");
            self.now.caret = range.start;
        }
        self.now.anchor = None;
    }

    fn move_caret(&mut self, to: usize, select: bool) {
        if select {
            self.now.anchor.get_or_insert(self.now.caret);
        } else {
            self.now.anchor = None;
        }
        self.now.caret = to;
    }

    fn boundary(&self, at: usize) -> usize {
        let mut at = at.min(self.now.text.len());
        while !self.now.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    fn prev_char(&self, at: usize) -> usize {
        self.now.text[..at]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i)
    }

    fn next_char(&self, at: usize) -> usize {
        self.now.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8())
    }

    /// Where the word before `at` starts: back over separators, then over
    /// the word. In a path, a word stops at `/`, `.`, `-`, `_` and spaces,
    /// so Ctrl+W takes off one folder at a time.
    fn word_before(&self, at: usize) -> usize {
        let text = &self.now.text[..at];
        let mut chars = text.char_indices().rev().peekable();
        while chars.next_if(|(_, c)| !is_word(*c)).is_some() {}
        while chars.next_if(|(_, c)| is_word(*c)).is_some() {}
        chars.peek().map_or(0, |(i, c)| i + c.len_utf8())
    }

    fn word_after(&self, at: usize) -> usize {
        let text = &self.now.text[at..];
        let mut chars = text.char_indices().peekable();
        while chars.next_if(|(_, c)| !is_word(*c)).is_some() {}
        while chars.next_if(|(_, c)| is_word(*c)).is_some() {}
        chars.peek().map_or(self.now.text.len(), |(i, _)| at + i)
    }

    // Pointer.

    /// Put the caret at `x` (relative to the field's text, in the frame
    /// last drawn), selecting from where it was with `extend`.
    pub fn click(&mut self, mut measure: impl FnMut(&str) -> f32, x: f32, extend: bool) {
        let x = x + self.offset;
        let text = &self.now.text;
        let mut best = (0, f32::MAX);
        for (i, _) in text
            .char_indices()
            .chain(std::iter::once((text.len(), ' ')))
        {
            let distance = (measure(&text[..i]) - x).abs();
            if distance < best.1 {
                best = (i, distance);
            }
        }
        self.move_caret(best.0, extend);
        self.typing = false;
    }

    /// Select the word at the caret (a double-click).
    pub fn select_word(&mut self) {
        let at = self.now.caret;
        let start = self.word_before(self.next_char(at).min(self.word_after(at)));
        let end = self.word_after(start);
        self.select(start..end);
    }

    // Drawing.

    /// Draw the field's text in `rect`: the selection under it, the caret
    /// (when `focused`), and `style.placeholder` when there is no text.
    ///
    /// A focused field also shows what an input method is composing, at
    /// the caret and underlined, and tells the canvas where its caret is
    /// so the input method's candidates appear beside it.
    pub fn draw(&mut self, canvas: &mut Canvas<'_>, rect: Rect, style: &Style<'_>, focused: bool) {
        let measure = |canvas: &mut Canvas<'_>, text: &str| {
            canvas.measure(&Text::new(text, style.size, style.color)).w
        };
        let at = self.now.caret;
        let preedit = canvas
            .preedit()
            .filter(|_| focused)
            .map(|preedit| (preedit.text.clone(), preedit.cursor.clone()));
        // What's shown: the text, with what's being composed at the caret.
        let (shown, composing) = match &preedit {
            Some((text, _)) => {
                let mut shown = self.now.text.clone();
                shown.insert_str(at, text);
                (shown, Some(at..at + text.len()))
            }
            None => (self.now.text.clone(), None),
        };
        // The caret is the input method's while it composes: hidden, or
        // standing in for a highlight, unless it is a plain caret.
        let (caret, shown_caret) = match (&preedit, &composing) {
            (Some((_, Some(cursor))), Some(range)) => {
                (range.start + cursor.start, cursor.is_empty())
            }
            (Some((_, None)), Some(range)) => (range.end, false),
            _ => (at, true),
        };
        let caret_x = measure(canvas, &shown[..caret]);
        let width = measure(canvas, &shown);
        // Slide the text so the caret stays in view, with a little room.
        let room = (rect.w * 0.15).min(24.0);
        if caret_x - self.offset > rect.w - room {
            self.offset = caret_x - rect.w + room;
        } else if caret_x - self.offset < room {
            self.offset = caret_x - room;
        }
        self.offset = self.offset.clamp(0.0, (width - rect.w + room).max(0.0));
        let offset = self.offset;
        let caret_height = style.size * 1.3;
        let caret_rect = Rect::new(
            rect.x + caret_x - offset,
            rect.y + (rect.h - caret_height) / 2.0,
            2.0,
            caret_height,
        );
        if focused {
            canvas.text_input_at(caret_rect);
        }

        canvas.with_clip(rect, |canvas| {
            let height = style.size * 1.4;
            let top = rect.y + (rect.h - height) / 2.0;
            let highlight = |canvas: &mut Canvas<'_>, range: Range<usize>| {
                let start = measure(canvas, &shown[..range.start]);
                let end = measure(canvas, &shown[..range.end]);
                canvas.fill(
                    Rect::new(rect.x + start - offset, top, end - start, height),
                    radius::CONTROL / 2.0,
                    style.selection,
                );
            };
            match (&composing, &preedit) {
                // What the input method has picked out in what it composes.
                (Some(range), Some((_, Some(cursor)))) if !cursor.is_empty() => {
                    highlight(canvas, range.start + cursor.start..range.start + cursor.end);
                }
                // The selection stays put while something is composed over it.
                (None, _) => {
                    if let Some(range) = self.selection().filter(|_| focused) {
                        highlight(canvas, range);
                    }
                }
                _ => {}
            }
            if shown.is_empty() {
                let placeholder = Text::new(style.placeholder, style.size, style.placeholder_color);
                canvas.text_in(&placeholder, rect, crate::Align::Start);
            } else {
                let text = Text::new(&shown, style.size, style.color);
                let height = canvas.measure(&text).h;
                canvas.text(&text, rect.x - offset, rect.y + (rect.h - height) / 2.0);
            }
            if let Some(range) = &composing {
                let start = measure(canvas, &shown[..range.start]);
                let end = measure(canvas, &shown[..range.end]);
                canvas.fill(
                    Rect::new(
                        rect.x + start - offset,
                        top + height - stroke::HAIRLINE,
                        end - start,
                        stroke::HAIRLINE,
                    ),
                    0.0,
                    style.color,
                );
            }
            if focused && shown_caret {
                canvas.fill(caret_rect, 1.0, style.caret);
            }
        });
    }

    /// How far the text is slid left, for turning a click's position into
    /// one in the text ([`click`](Self::click) does this itself).
    pub fn offset(&self) -> f32 {
        self.offset
    }
}

/// How a field looks.
#[derive(Clone, Copy, Debug)]
pub struct Style<'a> {
    pub size: f32,
    pub color: Rgba,
    pub placeholder: &'a str,
    pub placeholder_color: Rgba,
    pub caret: Rgba,
    pub selection: Rgba,
}

impl<'a> Style<'a> {
    /// Body text in `look`'s colours, saying `placeholder` while empty.
    pub fn new(look: &Look, placeholder: &'a str) -> Self {
        Self {
            size: look.size,
            color: look.text,
            placeholder,
            placeholder_color: look.muted,
            caret: look.accent,
            selection: look.accent.alpha(opacity::TINT * 2.0),
        }
    }
}

/// How a field stands, which decides the ring around it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// Has the keyboard.
    pub focused: bool,
    /// Something is typed in it.
    pub filled: bool,
    /// What was in it was refused.
    pub complaint: bool,
    /// What's in it is being checked: hands off for now.
    pub busy: bool,
}

impl Frame {
    /// Draw a field's shape — the one every field in the suite has: a
    /// pill on `fill`, ringed by how it stands. Returns where its contents
    /// go.
    pub fn draw(self, canvas: &mut Canvas<'_>, rect: Rect, look: &Look, fill: Rgba) -> Rect {
        let pill = radius::pill(rect.h);
        canvas.fill(rect, pill, fill);
        let (width, color) = if self.complaint {
            (stroke::FOCUS, look.danger.alpha(0.9))
        } else if self.busy {
            (stroke::HAIRLINE, look.highlight_high)
        } else if self.focused {
            let strength = if self.filled { 0.9 } else { 0.55 };
            (stroke::FOCUS, look.accent.alpha(strength))
        } else {
            (stroke::HAIRLINE, look.highlight_med)
        };
        canvas.stroke(rect, pill, width, color);
        rect.inset_xy(INSET, 0.0)
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mods;

    fn press(field: &mut TextField, key: Key, ctrl: bool, shift: bool) -> Response {
        field.key(&KeyEvent {
            key,
            mods: Mods {
                ctrl,
                shift,
                ..Mods::default()
            },
            repeat: false,
        })
    }

    fn typed(field: &mut TextField, text: &str) {
        for c in text.chars() {
            assert_eq!(press(field, Key::Char(c), false, false), Response::Changed);
        }
    }

    #[test]
    fn typing_and_deleting() {
        let mut field = TextField::default();
        typed(&mut field, "héllo");
        assert_eq!(field.text(), "héllo");
        press(&mut field, Key::Left, false, false);
        press(&mut field, Key::Left, false, false);
        press(&mut field, Key::Left, false, false);
        press(&mut field, Key::Backspace, false, false);
        assert_eq!(field.text(), "hllo");
        press(&mut field, Key::Delete, false, false);
        assert_eq!(field.text(), "hlo");
        press(&mut field, Key::End, false, false);
        typed(&mut field, "!");
        assert_eq!(field.text(), "hlo!");
        assert_eq!(
            press(&mut field, Key::Enter, false, false),
            Response::Submit
        );
        assert_eq!(
            press(&mut field, Key::Escape, false, false),
            Response::Cancel
        );
        assert_eq!(
            press(&mut field, Key::F(5), false, false),
            Response::Ignored
        );
    }

    #[test]
    fn words_stop_at_path_separators() {
        let mut field = TextField::new("~/dev/enso-files/src");
        press(&mut field, Key::Char('w'), true, false);
        assert_eq!(field.text(), "~/dev/enso-files/");
        press(&mut field, Key::Char('w'), true, false);
        assert_eq!(field.text(), "~/dev/enso-");
        press(&mut field, Key::Backspace, true, false);
        assert_eq!(field.text(), "~/dev/");
        press(&mut field, Key::Home, false, false);
        press(&mut field, Key::Right, true, false);
        assert_eq!(field.caret(), "~/dev".len());
        press(&mut field, Key::Delete, true, false);
        assert_eq!(field.text(), "~/dev");
    }

    #[test]
    fn shift_selects_and_typing_replaces_it() {
        let mut field = TextField::new("report.pdf");
        press(&mut field, Key::Home, false, false);
        press(&mut field, Key::Right, true, true);
        assert_eq!(field.selected_text(), Some("report"));
        typed(&mut field, "x");
        assert_eq!(field.text(), "x.pdf");
        press(&mut field, Key::Char('a'), true, false);
        assert_eq!(field.selected_text(), Some("x.pdf"));
        // Without Shift, an arrow collapses the selection to its side.
        press(&mut field, Key::Left, false, false);
        assert_eq!((field.selection(), field.caret()), (None, 0));
    }

    #[test]
    fn the_stem_is_selected_for_renaming() {
        let mut field = TextField::new("report.final.pdf");
        field.select_stem();
        assert_eq!(field.selected_text(), Some("report.final"));
        let mut field = TextField::new(".bashrc");
        field.select_stem();
        assert_eq!(field.selected_text(), Some(".bashrc"));
        let mut field = TextField::new("Makefile");
        field.select_stem();
        assert_eq!(field.selected_text(), Some("Makefile"));
    }

    #[test]
    fn clipboard_keys_are_handed_back() {
        let mut field = TextField::new("one two");
        assert_eq!(
            press(&mut field, Key::Char('c'), true, false),
            Response::Ignored
        );
        field.select(4..7);
        assert_eq!(
            press(&mut field, Key::Char('c'), true, false),
            Response::Copy("two".into())
        );
        assert_eq!(
            press(&mut field, Key::Char('x'), true, false),
            Response::Cut("two".into())
        );
        assert_eq!(field.text(), "one ");
        assert_eq!(
            press(&mut field, Key::Char('v'), true, false),
            Response::Paste
        );
        field.insert("three\nfour\u{7}");
        assert_eq!(field.text(), "one three four");
    }

    #[test]
    fn undo_takes_back_a_typed_run_at_once() {
        let mut field = TextField::new("a");
        typed(&mut field, "bcd");
        press(&mut field, Key::Backspace, false, false);
        assert_eq!(field.text(), "abc");
        press(&mut field, Key::Char('z'), true, false);
        assert_eq!(field.text(), "abcd");
        press(&mut field, Key::Char('z'), true, false);
        assert_eq!(field.text(), "a");
        press(&mut field, Key::Char('z'), true, true);
        assert_eq!(field.text(), "abcd");
        // Something new forgets what could be redone.
        typed(&mut field, "e");
        assert_eq!(
            press(&mut field, Key::Char('z'), true, true),
            Response::Moved
        );
        assert_eq!(field.text(), "abcde");
    }

    #[test]
    fn kills_to_either_end() {
        let mut field = TextField::new("hello world");
        field.select(5..5);
        press(&mut field, Key::Char('k'), true, false);
        assert_eq!(field.text(), "hello");
        press(&mut field, Key::Char('u'), true, false);
        assert_eq!(field.text(), "");
    }

    #[test]
    fn clicks_land_between_characters() {
        // Every character 10 wide.
        let measure = |text: &str| text.chars().count() as f32 * 10.0;
        let mut field = TextField::new("abcdef");
        field.click(measure, 24.0, false);
        assert_eq!(field.caret(), 2);
        field.click(measure, 41.0, true);
        assert_eq!(field.selected_text(), Some("cd"));
        field.click(measure, 500.0, false);
        assert_eq!(field.caret(), 6);
        let mut field = TextField::new("one two");
        field.click(measure, 52.0, false);
        field.select_word();
        assert_eq!(field.selected_text(), Some("two"));
    }
}
