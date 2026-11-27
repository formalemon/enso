//! The window as a picker: the request it answers, the filter and choices
//! as they stand, the name being saved as, and the answer once there is
//! one.
//!
//! Deciding what Enter or the accept button means — given the request, the
//! selection, and what's under the cursor — is here and plain, so each case
//! is tested without a window.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
};

use enso_fs::mime::MimeDb;
use enso_ui::widget::TextField;

use crate::{
    pane::Keep,
    pick::{Answer, Choice, Mode, Request, with_extension},
    selection::Selection,
};

/// Where the picker's answer is left for whoever started it.
pub type Outcome = Rc<RefCell<Option<Answer>>>;

#[derive(Debug)]
pub struct Picker {
    pub request: Request,
    /// The filter in use, as an index into the request's.
    pub filter: Option<usize>,
    pub choices: Vec<Choice>,
    /// Saving: the name, in its field.
    pub name: Option<TextField>,
    /// Whether the keyboard is in the name field rather than the list.
    pub typing_name: bool,
    pub outcome: Outcome,
    db: Rc<MimeDb>,
}

/// What accepting means right now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Accept {
    /// Hand these back.
    Answer(Vec<PathBuf>),
    /// Go into this folder first.
    Enter(PathBuf),
    /// Saving over an existing file: ask first.
    Confirm(PathBuf),
    /// Nothing to accept yet.
    Nothing,
}

/// What is under the cursor, as far as accepting is concerned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Under {
    Dir(PathBuf),
    File(PathBuf),
    Nothing,
}

impl Picker {
    pub fn new(request: Request, outcome: Outcome) -> Self {
        let filter = request.first_filter();
        let name = match &request.mode {
            Mode::Save { name } => {
                let mut field = TextField::new(name);
                field.select_stem();
                Some(field)
            }
            _ => None,
        };
        let typing_name = name.is_some();
        let choices = request.choices.clone();
        // Loading the MIME database costs a few milliseconds; only filters
        // that name types need it.
        let db = if request.filters.is_empty() {
            MimeDb::default()
        } else {
            MimeDb::load()
        };
        Self {
            request,
            filter,
            choices,
            name,
            typing_name,
            outcome,
            db: Rc::new(db),
        }
    }

    /// What the folder shows: folders always (to get somewhere), and files
    /// only when picking files and, if a filter is on, of its kind.
    pub fn keep(&self) -> Option<Keep> {
        let directory = self.request.directory();
        let filter = self
            .filter
            .and_then(|index| self.request.filters.get(index))
            .cloned();
        if !directory && filter.is_none() {
            return None;
        }
        let db = self.db.clone();
        Some(Keep::new(move |name, is_dir| {
            is_dir
                || (!directory
                    && filter
                        .as_ref()
                        .is_none_or(|filter| filter.matches(name, &db)))
        }))
    }

    /// Change the filter. Saving, the name takes the filter's extension.
    pub fn set_filter(&mut self, index: usize) {
        if index >= self.request.filters.len() {
            return;
        }
        self.filter = Some(index);
        let extension = self.request.filters[index].extension().map(str::to_owned);
        if let (Some(field), Some(extension)) = (&mut self.name, extension)
            && !field.is_empty()
        {
            let renamed = with_extension(field.text(), &extension);
            field.replace_all(&renamed);
            field.select_stem();
        }
    }

    /// Flip a toggle choice, or set an options choice.
    pub fn set_choice(&mut self, index: usize, value: Option<&str>) {
        match self.choices.get_mut(index) {
            Some(Choice::Toggle { on, .. }) => *on = !*on,
            Some(Choice::Options {
                options, selected, ..
            }) => {
                if let Some(value) = value.filter(|value| options.iter().any(|(id, _)| id == value))
                {
                    *selected = value.to_owned();
                }
            }
            None => {}
        }
    }

    /// What accepting means: from the button, from Enter, or from a
    /// double-click (`under` the pointer, then).
    pub fn accept(&self, location: &Path, selection: &Selection, under: &Under) -> Accept {
        match &self.request.mode {
            Mode::Save { .. } => {
                let name = self.name.as_ref().map_or("", |field| field.text()).trim();
                if name.is_empty() {
                    return Accept::Nothing;
                }
                let path = location.join(name);
                if path.is_dir() {
                    Accept::Enter(path)
                } else if path.exists() {
                    Accept::Confirm(path)
                } else {
                    Accept::Answer(vec![path])
                }
            }
            Mode::SaveFiles { .. } => match under {
                // A folder chosen in the list is where they go…
                _ if !selection.is_empty() => Accept::Answer(selection.paths()[..1].to_vec()),
                // …otherwise here.
                _ => Accept::Answer(vec![location.to_owned()]),
            },
            Mode::Open {
                directory: true, ..
            } => {
                let folders: Vec<PathBuf> = selection
                    .paths()
                    .iter()
                    .filter(|path| path.is_dir())
                    .cloned()
                    .collect();
                if folders.is_empty() {
                    Accept::Answer(vec![location.to_owned()])
                } else {
                    Accept::Answer(self.limit(folders))
                }
            }
            Mode::Open { .. } => {
                let files: Vec<PathBuf> = selection
                    .paths()
                    .iter()
                    .filter(|path| !path.is_dir())
                    .cloned()
                    .collect();
                match under {
                    Under::Dir(path) if files.is_empty() => Accept::Enter(path.clone()),
                    Under::File(path) if files.is_empty() => Accept::Answer(vec![path.clone()]),
                    _ if files.is_empty() => Accept::Nothing,
                    _ => Accept::Answer(self.limit(files)),
                }
            }
        }
    }

    /// One path when only one was asked for.
    fn limit(&self, mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
        if !self.request.multiple() {
            paths.truncate(1);
        }
        paths
    }

    /// Whether the accept button has anything to do. It takes what is
    /// chosen, never what the cursor is on: a file isn't offered up for
    /// sitting first in its folder, and the button never enters folders
    /// (that's Enter's, and a double-click's).
    pub fn ready(&self, location: &Path, selection: &Selection) -> bool {
        matches!(
            self.accept(location, selection, &Under::Nothing),
            Accept::Answer(_) | Accept::Confirm(_)
        )
    }

    /// Leave `paths` as the answer.
    pub fn answer(&self, paths: Vec<PathBuf>) {
        *self.outcome.borrow_mut() = Some(Answer {
            paths,
            filter: self.filter,
            choices: self
                .choices
                .iter()
                .map(|choice| (choice.id().to_owned(), choice.value()))
                .collect(),
        });
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::pick::Filter;

    fn picker(mode: Mode) -> Picker {
        Picker::new(Request::new(mode), Outcome::default())
    }

    fn open(multiple: bool) -> Picker {
        picker(Mode::Open {
            multiple,
            directory: false,
        })
    }

    fn chosen(paths: &[&str]) -> Selection {
        let mut selection = Selection::default();
        for path in paths {
            selection.insert(PathBuf::from(path));
        }
        selection
    }

    #[test]
    fn opening_takes_the_selection_or_the_file_under_the_cursor() {
        let here = Path::new("/h");
        let none = Selection::default();
        let file = Under::File("/h/a.txt".into());
        let dir = Under::Dir("/h/sub".into());
        let single = open(false);
        assert_eq!(
            single.accept(here, &none, &file),
            Accept::Answer(vec!["/h/a.txt".into()])
        );
        assert_eq!(
            single.accept(here, &none, &dir),
            Accept::Enter("/h/sub".into())
        );
        assert_eq!(single.accept(here, &none, &Under::Nothing), Accept::Nothing);
        assert!(
            !single.ready(here, &none),
            "nothing chosen: the button waits, wherever the cursor is"
        );
        assert!(single.ready(here, &chosen(&["/h/a.txt"])));

        let many = open(true);
        let basket = chosen(&["/x/1.png", "/y/2.png"]);
        // With things chosen, they're the answer wherever the cursor is.
        assert_eq!(
            many.accept(here, &basket, &dir),
            Accept::Answer(vec!["/x/1.png".into(), "/y/2.png".into()])
        );
        // Only one, if only one was asked for.
        assert_eq!(
            single.accept(here, &basket, &file),
            Accept::Answer(vec!["/x/1.png".into()])
        );
    }

    #[test]
    fn choosing_a_folder_defaults_to_where_we_are() {
        let folder = picker(Mode::Open {
            multiple: false,
            directory: true,
        });
        let here = Path::new("/tmp");
        assert_eq!(
            folder.accept(here, &Selection::default(), &Under::Nothing),
            Accept::Answer(vec!["/tmp".into()])
        );
        assert!(folder.keep().is_some(), "files are left out");
        assert!(!(folder.keep().unwrap().0)("a.txt", false));
        assert!((folder.keep().unwrap().0)("sub", true));
    }

    #[test]
    fn saving_asks_before_replacing() {
        let root = std::env::temp_dir().join(format!("enso-files-save-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("taken.txt"), "").unwrap();
        let mut save = picker(Mode::Save {
            name: "new.txt".into(),
        });
        let none = Selection::default();
        assert_eq!(
            save.accept(&root, &none, &Under::Nothing),
            Accept::Answer(vec![root.join("new.txt")])
        );
        save.name.as_mut().unwrap().set_text("taken.txt");
        assert_eq!(
            save.accept(&root, &none, &Under::Nothing),
            Accept::Confirm(root.join("taken.txt"))
        );
        save.name.as_mut().unwrap().set_text("sub");
        assert_eq!(
            save.accept(&root, &none, &Under::Nothing),
            Accept::Enter(root.join("sub"))
        );
        save.name.as_mut().unwrap().set_text("  ");
        assert_eq!(save.accept(&root, &none, &Under::Nothing), Accept::Nothing);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_save_filter_sets_the_extension() {
        let mut request = Request::new(Mode::Save {
            name: "report.txt".into(),
        });
        request.filters = vec![
            Filter::parse("Text:*.txt").unwrap(),
            Filter::parse("PDF:*.pdf").unwrap(),
        ];
        let mut save = Picker::new(request, Outcome::default());
        assert_eq!(save.name.as_ref().unwrap().selected_text(), Some("report"));
        save.set_filter(1);
        assert_eq!(save.name.as_ref().unwrap().text(), "report.pdf");
        save.set_filter(9);
        assert_eq!(save.filter, Some(1));
    }

    #[test]
    fn the_answer_carries_the_filter_and_choices() {
        let mut request = Request::new(Mode::Open {
            multiple: false,
            directory: false,
        });
        request.choices = vec![
            Choice::Toggle {
                id: "ro".into(),
                label: "Read-only".into(),
                on: false,
            },
            Choice::Options {
                id: "enc".into(),
                label: "Encoding".into(),
                options: vec![
                    ("utf8".into(), "UTF-8".into()),
                    ("latin1".into(), "Latin-1".into()),
                ],
                selected: "utf8".into(),
            },
        ];
        let mut picker = Picker::new(request, Outcome::default());
        picker.set_choice(0, None);
        picker.set_choice(1, Some("latin1"));
        picker.set_choice(1, Some("klingon"));
        picker.answer(vec!["/a".into()]);
        let answer = picker.outcome.borrow().clone().unwrap();
        assert_eq!(answer.paths, [PathBuf::from("/a")]);
        assert_eq!(
            answer.choices,
            [
                ("ro".into(), "true".into()),
                ("enc".into(), "latin1".into())
            ]
        );
    }
}
