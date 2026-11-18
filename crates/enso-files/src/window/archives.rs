//! Archives in the window: Enter on one goes inside, as into a folder —
//! read-only, its folders walked like any others, the crumbs leading back
//! out. A file inside opens as a copy taken out for the moment (in
//! `~/.cache/enso/opened`). "Extract here" takes everything out; "Compress…"
//! makes a zip, `.tar.gz`, or `.tar.xz` of what's chosen.

use std::{
    path::{Path, PathBuf},
    thread,
};

use enso_core::style::space;
use enso_fs::archive::{self, Format};
use enso_ui::{
    Align, Canvas, Cx, Key, KeyEvent, Rect, Text,
    widget::{Form, Interaction, Kind, Note, Part, TextField, field, field::Frame},
};

use super::overlay::{Overlay, Reach};
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
        self.overlay = Some(Overlay::Packing(Packing {
            sources,
            name,
            format: 0,
        }));
    }

    #[cfg(test)]
    pub(super) fn ask_compress_for_tests(&mut self) {
        let sources = self.targets();
        self.overlay = Some(Overlay::Packing(Packing {
            sources,
            name: TextField::new("trip"),
            format: 2,
        }));
    }

    fn make_archive(&mut self, cx: &mut Cx<'_>) {
        let Some(packing) = self.overlay.take_packing() else {
            return;
        };
        let name = packing.name.text().trim().to_owned();
        if let Err(why) = enso_fs::ops::check_name(&name) {
            self.toast(why, cx);
            self.overlay = Some(Overlay::Packing(packing));
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
        let Some(packing) = self.overlay.packing_mut() else {
            return false;
        };
        match key.key {
            Key::Escape => self.overlay = None,
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
        let Some(packing) = self.overlay.packing_mut() else {
            return false;
        };
        packing.name.insert(text);
        true
    }

    pub(super) fn packing_press(&mut self, hit: Option<&Hit>, cx: &mut Cx<'_>) -> bool {
        let Some(packing) = self.overlay.packing_mut() else {
            return false;
        };
        match hit {
            Some(Hit::Pack(PackHit::Format(index))) => packing.format = *index,
            Some(Hit::Pack(PackHit::Make)) => self.make_archive(cx),
            Some(Hit::Pack(PackHit::Cancel)) | None => self.overlay = None,
            _ => {}
        }
        cx.redraw();
        true
    }

    pub(super) fn draw_packing(&mut self, canvas: &mut Canvas<'_>, bounds: Rect) {
        let Some(packing) = self.overlay.packing() else {
            return;
        };
        let look = self.look();
        let what = enso_fs::format::count(packing.sources.len(), "item", "items");
        let title = format!("Compress {what}");
        let (format, label) = MADE[packing.format];
        let about = match label {
            "zip" => "zip opens anywhere.",
            "tar.gz" => "tar.gz is Linux's usual.",
            _ => "tar.xz is the smallest, and the slowest to make.",
        };
        let names = MADE.map(|(_, name)| name);
        let hovered = |hit: PackHit| Interaction {
            hovered: self.hover == Some(Hit::Pack(hit)),
            ..Interaction::default()
        };
        let placed = Form::new(&title)
            .width(480.0)
            .part(Part::Field {
                label: Some("Name"),
            })
            .part(Part::Choices {
                label: Some("As"),
                options: &names,
                chosen: packing.format,
                hovered: match self.hover {
                    Some(Hit::Pack(PackHit::Format(index))) => Some(index),
                    _ => None,
                },
                enabled: true,
            })
            .note(Note::Says(about))
            .button("Cancel", Kind::Secondary, hovered(PackHit::Cancel))
            .button(
                "Compress",
                Kind::Primary,
                Interaction {
                    disabled: packing.name.text().trim().is_empty(),
                    ..hovered(PackHit::Make)
                },
            )
            .draw(canvas, bounds, &look, 1.0);

        // The name, and the suffix it will have.
        let place = placed.parts[0];
        let frame = Frame {
            focused: true,
            filled: true,
            ..Frame::default()
        };
        let text = frame.draw(canvas, place, &look, look.base);
        let suffix = Text::new(format.suffix(), look.size, look.subtle);
        let suffix_w = canvas.measure(&suffix).w;
        let name = Rect::new(text.x, text.y, text.w - suffix_w - space::SM, text.h);
        if let Some(packing) = self.overlay.packing_mut() {
            packing
                .name
                .draw(canvas, name, &field::Style::new(&look, "Archive"), true);
        }
        canvas.text_in(
            &suffix,
            Rect::new(name.right(), text.y, suffix_w + space::SM, text.h),
            Align::End,
        );

        // The whole card answers, so a press on it doesn't put it away.
        self.hits
            .push((placed.inner.inset(-space::XL), Hit::Pack(PackHit::Field)));
        self.hits.push((place, Hit::Pack(PackHit::Field)));
        for &(_, index, pill) in &placed.choices {
            self.hits.push((pill, Hit::Pack(PackHit::Format(index))));
        }
        self.hits
            .push((placed.buttons[0], Hit::Pack(PackHit::Cancel)));
        self.hits
            .push((placed.buttons[1], Hit::Pack(PackHit::Make)));
    }
}
