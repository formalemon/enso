//! Archives in the window: Enter on one goes inside, as into a folder —
//! read-only, its folders walked like any others, the crumbs leading back
//! out. A file inside opens as a copy taken out for the moment (in
//! `~/.cache/enso/opened`). "Extract here" takes everything out; "Compress…"
//! makes a zip, `.tar.gz`, or `.tar.xz` of what's chosen.

use std::{
    path::{Path, PathBuf},
    thread,
};

use enso_core::style::{opacity, radius, space, stroke, type_scale};
use enso_fs::archive::{self, Format};
use enso_ui::{
    Align, Canvas, Cx, Key, KeyEvent, Rect, Text,
    widget::{Button, Interaction, Kind, TextField, controls, field},
};

use super::{Files, Hit};
use crate::{
    jobs::{Message, Op},
    pane::Source,
};

/// The formats an archive is made in, as offered.
const MADE: [(Format, &str); 3] = [
    (Format::Zip, "zip"),
    (Format::TarGz, "tar.gz"),
    (Format::TarXz, "tar.xz"),
];

/// Where `path` is inside an archive, if it is: the archive, and the path
/// inside (`""` for the archive itself).
pub(super) fn inside_archive(path: &Path) -> Option<(PathBuf, String)> {
    if path.is_dir() {
        return None;
    }
    path.ancestors().find_map(|candidate| {
        let name = candidate.file_name()?.to_str()?;
        if Format::of(name).is_none() || !candidate.is_file() {
            return None;
        }
        let inner = path
            .strip_prefix(candidate)
            .ok()?
            .to_string_lossy()
            .into_owned();
        Some((candidate.to_owned(), inner))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PackHit {
    Field,
    Format(usize),
    Make,
    Cancel,
}

/// The "Compress…" question: what to call it, in what format.
#[derive(Debug)]
pub(super) struct Packing {
    sources: Vec<PathBuf>,
    name: TextField,
    format: usize,
}

impl Files {
    /// Whether what's shown is inside an archive; if so, say that nothing
    /// can change there.
    pub(super) fn read_only(&mut self, cx: &mut Cx<'_>) -> bool {
        let inside = matches!(self.pane.source(), Source::Archive { .. });
        if inside {
            self.toast("Inside an archive nothing changes · Extract it first", cx);
        }
        inside
    }

    /// A file inside the archive shown: open a copy taken out of it.
    pub(super) fn open_member(&mut self, path: &Path) {
        let archive = self.pane.location().to_owned();
        let Ok(member) = path
            .strip_prefix(&archive)
            .map(|member| member.to_string_lossy().into_owned())
        else {
            return;
        };
        let Some(proxy) = self.proxy.clone() else {
            return;
        };
        self.opened += 1;
        let into = self.ctx.paths.cache_dir.join("opened").join(format!(
            "{}-{}",
            std::process::id(),
            self.opened
        ));
        let _ = thread::Builder::new()
            .name("enso-files-member".to_owned())
            .spawn(move || {
                let result = archive::extract_member(&archive, &member, &into)
                    .map_err(|err| format!("Can't take it out: {err}"))
                    .and_then(|file| {
                        enso_fs::open::open(&file, "alacritty")
                            .map_err(|err| format!("Can't open it: {err}"))
                    });
                if let Err(why) = result {
                    proxy.send(Message::Toast(why));
                }
            });
    }

    /// Take everything out of `archive`, beside it.
    pub(super) fn extract(&mut self, archive: PathBuf, cx: &mut Cx<'_>) {
        let Some(into) = archive.parent().map(Path::to_owned) else {
            return;
        };
        self.start_op(Op::Extract { archive, into }, cx);
    }

    /// Ask what to call an archive of what's chosen.
    pub(super) fn ask_compress(&mut self, cx: &mut Cx<'_>) {
        if self.read_only(cx)
            || !self.pane.source().is_folder()
                && !matches!(self.pane.source(), Source::Search { .. })
        {
            return;
        }
        let sources = self.targets();
        let Some(first) = sources.first() else {
            return;
        };
        // One thing: its name. Several: the folder they're in.
        let suggested = if sources.len() == 1 {
            first
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        } else {
            first
                .parent()
                .and_then(Path::file_name)
                .map(|name| name.to_string_lossy().into_owned())
        }
        .unwrap_or_else(|| "Archive".to_owned());
        let mut name = TextField::new(&suggested);
        name.select_all();
        self.packing = Some(Packing {
            sources,
            name,
            format: 0,
        });
    }

    #[cfg(test)]
    pub(super) fn ask_compress_for_tests(&mut self) {
        let sources = self.targets();
        self.packing = Some(Packing {
            sources,
            name: TextField::new("trip"),
            format: 2,
        });
    }

    fn make_archive(&mut self, cx: &mut Cx<'_>) {
        let Some(packing) = self.packing.take() else {
            return;
        };
        let name = packing.name.text().trim().to_owned();
        if let Err(why) = enso_fs::ops::check_name(&name) {
            self.toast(why, cx);
            self.packing = Some(packing);
            return;
        }
        let (format, _) = MADE[packing.format];
        let Some(folder) = packing.sources[0].parent() else {
            return;
        };
        let into = enso_fs::ops::free_name(folder, &format!("{name}{}", format.suffix()));
        self.start_op(
            Op::Compress {
                sources: packing.sources,
                into,
            },
            cx,
        );
    }

    /// Keys while asking what to call an archive. Returns whether it was.
    pub(super) fn packing_key(&mut self, key: &KeyEvent, cx: &mut Cx<'_>) -> bool {
        let Some(packing) = &mut self.packing else {
            return false;
        };
        match key.key {
            Key::Escape => self.packing = None,
            Key::Enter => self.make_archive(cx),
            Key::Tab => packing.format = (packing.format + 1) % MADE.len(),
            _ => match packing.name.key(key) {
                field::Response::Paste => cx.paste(&enso_ui::TEXT_TYPES),
                field::Response::Copy(text) | field::Response::Cut(text) => {
                    cx.copy(enso_ui::Clip::text(&text))
                }
                _ => {}
            },
        }
        cx.redraw();
        true
    }

    pub(super) fn packing_paste(&mut self, text: &str) -> bool {
        let Some(packing) = &mut self.packing else {
            return false;
        };
        packing.name.insert(text);
        true
    }

    pub(super) fn packing_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        let Some(packing) = &mut self.packing else {
            return false;
        };
        match hit {
            Some(Hit::Pack(PackHit::Format(index))) => packing.format = *index,
            Some(Hit::Pack(PackHit::Make)) => self.make_archive(cx),
            Some(Hit::Pack(PackHit::Cancel)) | None => self.packing = None,
            _ => {}
        }
        cx.redraw();
        true
    }

    pub(super) fn draw_packing(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        if self.packing.is_none() {
            return;
        }
        let look = self.look();
        let colors = self.ctx.theme.colors.clone();
        let accent = self.ctx.accent();
        let body = self.font(type_scale::BODY);
        let small = self.font(type_scale::SMALL);
        let heading = self.font(type_scale::HEADING);
        self.hits.clear();
        let hover = self.hover.clone();
        let Some(packing) = &mut self.packing else {
            return;
        };
        let height =
            44.0 + 22.0 + 40.0 + space::LG + 32.0 + space::XL + controls::HEIGHT + 2.0 * space::XL;
        let inner = controls::dialog(canvas, bounds, 480.0, height, &look, 1.0);
        let what = enso_fs::format::count(packing.sources.len(), "item", "items");
        canvas.text(
            &Text::new(&format!("Compress {what}"), heading, colors.text).medium(),
            inner.x,
            inner.y,
        );
        let mut y = inner.y + 44.0;
        canvas.text(&Text::new("Name", small, colors.muted), inner.x, y);
        y += 22.0;
        let (format, label) = MADE[packing.format];
        let suffix = Text::new(format.suffix(), body, colors.subtle);
        let suffix_w = canvas.measure(&suffix).w;
        let rect = Rect::new(inner.x, y, inner.w, 40.0);
        canvas.fill(rect, radius::CONTROL, colors.base);
        canvas.stroke(rect, radius::CONTROL, stroke::FOCUS, accent);
        let style = field::Style {
            size: body,
            color: colors.text,
            placeholder: "Archive",
            placeholder_color: colors.muted,
            caret: accent,
            selection: accent.alpha(opacity::TINT * 2.0),
        };
        let text = Rect::new(
            rect.x + space::MD,
            rect.y,
            rect.w - 2.0 * space::MD - suffix_w - space::SM,
            rect.h,
        );
        packing.name.draw(canvas, text, &style, true);
        canvas.text_in(
            &suffix,
            Rect::new(text.right(), rect.y, suffix_w + space::SM, rect.h),
            Align::End,
        );
        let mut hits = vec![(rect, Hit::Pack(PackHit::Field))];
        y += 40.0 + space::LG;
        let mut x = inner.x;
        for (index, (_, name)) in MADE.iter().enumerate() {
            let on = index == packing.format;
            let text = Text::new(name, small, if on { look.base } else { colors.subtle });
            let w = canvas.measure(&text).w + 2.0 * space::MD;
            let pill = Rect::new(x, y, w, 30.0);
            canvas.fill(
                pill,
                radius::pill(pill.h),
                if on { accent } else { colors.overlay },
            );
            canvas.text_in(&text, pill, Align::Center);
            hits.push((pill, Hit::Pack(PackHit::Format(index))));
            x = pill.right() + space::XS;
        }
        let note = match label {
            "zip" => "opens anywhere",
            "tar.gz" => "Linux's usual",
            _ => "smallest, slowest",
        };
        canvas.text_in(
            &Text::new(note, small, colors.muted),
            Rect::new(x + space::SM, y, inner.right() - x, 30.0),
            Align::Start,
        );

        let row_y = inner.bottom() - controls::HEIGHT;
        let make = Button::new("Compress", Kind::Primary);
        let w = make.width(canvas, &look);
        let make_rect = Rect::new(inner.right() - w, row_y, w, controls::HEIGHT);
        make.draw(
            canvas,
            make_rect,
            &look,
            Interaction {
                hovered: hover == Some(Hit::Pack(PackHit::Make)),
                disabled: packing.name.text().trim().is_empty(),
                ..Interaction::default()
            },
        );
        hits.push((make_rect, Hit::Pack(PackHit::Make)));
        let cancel = Button::new("Cancel", Kind::Secondary);
        let w = cancel.width(canvas, &look);
        let rect = Rect::new(make_rect.x - space::SM - w, row_y, w, controls::HEIGHT);
        cancel.draw(
            canvas,
            rect,
            &look,
            Interaction {
                hovered: hover == Some(Hit::Pack(PackHit::Cancel)),
                ..Interaction::default()
            },
        );
        hits.push((rect, Hit::Pack(PackHit::Cancel)));
        // The whole card answers, so a press on it doesn't put it away.
        self.hits
            .push((inner.inset(-space::XL), Hit::Pack(PackHit::Field)));
        self.hits.extend(hits);
    }
}
