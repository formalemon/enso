//! The window, rendered offscreen and driven by keys and clicks: what
//! each view, panel, and question looks like, and that it does what it
//! should.

use std::time::Duration;

use enso_core::{Context, Paths};
use enso_fs::list::{Kind, Listing, Meta, Target};
use enso_ui::{Offscreen, TextEngine, widget::TextField};

use super::draw::{TITLE_HEIGHT, crumbs, title};
use super::overlay::Reach;
use super::*;
use crate::{
    config::FilesConfig,
    pick::{self, Filter, Mode},
    picker::Outcome,
};

fn context() -> Context {
    Context::load_from(Paths::rooted("/nonexistent/enso-files-test"))
}

/// A Monday morning, in UTC, whatever the hour and the zone here.
fn monday() -> jiff::Zoned {
    "2026-11-16T09:41:00+00:00[UTC]".parse().expect("a time")
}

/// A machine that's the same everywhere: [`monday`], a disk half full,
/// the trash empty.
fn steady() -> super::machine::Machine {
    super::machine::Machine {
        now: monday,
        space: |_| {
            Ok(enso_fs::places::Space {
                total: 500_000_000_000,
                free: 250_000_000_000,
            })
        },
        trash_full: || false,
    }
}

fn fill(files: &mut Files) {
    let request = files.pending.take().unwrap();
    let entries = [
        ("crates", Kind::Dir),
        ("docs", Kind::Dir),
        ("target", Kind::Dir),
        ("latest", Kind::Link(Target::Dir)),
        (".git", Kind::Dir),
        ("Cargo.lock", Kind::File),
        ("Cargo.toml", Kind::File),
        (
            "a file with a name far too long to fit in its column at all.txt",
            Kind::File,
        ),
        ("README.md", Kind::File),
        ("photo.png", Kind::File),
    ];
    let listing = Listing::from_entries(request.path.clone(), entries);
    let now = monday().timestamp().as_nanosecond() as i64;
    let metas: Vec<Option<Meta>> = (0..entries.len())
        .map(|i| {
            Some(Meta {
                size: 1500 * (i as u64 + 1).pow(4),
                modified: now - (i as i64) * 7_200_000_000_000,
                created: None,
                accessed: now,
                mode: 0o644,
                uid: 1000,
                gid: 1000,
                link_size: None,
            })
        })
        .collect();
    assert!(files.pane.set_listing(request.generation, Ok(listing)));
    assert!(files.pane.apply_stats(request.generation, 0, &metas));
}

/// The manager onto `folder`, on the steady machine, with a made-up home
/// in its sidebar in place of this machine's places and drives.
fn steady_files(folder: PathBuf) -> Files {
    let mut files = Files::new(context(), FilesConfig::default(), folder);
    files.places = vec![super::Place {
        label: "Home".into(),
        icon: icon::FOLDER,
        path: "/home/me".into(),
    }];
    files.volumes = Vec::new();
    files.machine = steady();
    files
}

/// The manager onto a made-up folder, its listing and sizes arrived.
fn files() -> Files {
    let mut files = Files::new(
        context(),
        FilesConfig::default(),
        "/home/me/dev/enso".into(),
    );
    files.home = "/home/me".into();
    files.places = Vec::new();
    files.volumes = Vec::new();
    files.machine = steady();
    fill(&mut files);
    files
}

fn render(files: &mut Files, size: (u32, u32)) -> Offscreen {
    let later = Instant::now() + Duration::from_secs(2);
    let mut text = TextEngine::new(&files.ctx.config.appearance);
    Offscreen::render(&mut text, size, 2, |canvas| {
        assert_eq!(files.draw_window(canvas, later), Paint::Idle);
    })
}

/// Draw until whatever was set moving has stopped.
fn settle(files: &mut Files, size: (u32, u32)) -> Offscreen {
    let start = Instant::now() + Duration::from_secs(2);
    let mut text = TextEngine::new(&files.ctx.config.appearance);
    Offscreen::render(&mut text, size, 2, |canvas| {
        files.draw_window(canvas, start);
        assert_eq!(
            files.draw_window(canvas, start + Duration::from_secs(2)),
            Paint::Idle
        );
    })
}

/// The pictures held to a reference in `snapshots/`; the rest are
/// only written, to look at.
const CHECKED: &[&str] = &["enso-files", "enso-files-identity"];

fn snapshot(frame: &Offscreen, name: &str) {
    if CHECKED.contains(&name) {
        frame.check(env!("CARGO_MANIFEST_DIR"), name);
    } else {
        frame.snapshot(env!("CARGO_MANIFEST_DIR"), name);
    }
}

fn rows(files: &Files) -> usize {
    files
        .hits
        .iter()
        .filter(|(_, hit)| matches!(hit, Hit::Row(_)))
        .count()
}

#[test]
fn renders_a_folder() {
    let mut files = files();
    files.pane.set_cursor(1);
    files
        .selection
        .insert("/home/me/dev/enso/Cargo.toml".into());
    files.places = vec![super::Place {
        label: "Home".into(),
        icon: icon::FOLDER,
        path: "/home/me".into(),
    }];
    let frame = render(&mut files, (900, 420));
    snapshot(&frame, "enso-files");
    assert_eq!(rows(&files), 9);
    assert!(
        files
            .hits
            .iter()
            .all(|(rect, hit)| !matches!(hit, Hit::Row(_)) || rect.y >= TITLE_HEIGHT)
    );
    let crumbs: Vec<&Hit> = files
        .hits
        .iter()
        .map(|(_, hit)| hit)
        .filter(|hit| matches!(hit, Hit::Crumb(_)))
        .collect();
    assert_eq!(
        crumbs,
        [
            &Hit::Crumb("/home/me".into()),
            &Hit::Crumb("/home/me/dev".into())
        ]
    );
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| *hit == Hit::Place("/home/me".into()))
    );
}

#[test]
fn renders_the_filter() {
    let mut files = files();
    let mut field = TextField::new("carg");
    field.select(1..3);
    files.filter = Some(field);
    files.pane.set_filter("carg");
    snapshot(&render(&mut files, (900, 300)), "enso-files-filter");
    assert_eq!(files.pane.len(), 2);
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::Filter(_)))
    );
}

#[test]
fn renders_narrow_and_empty() {
    let mut files = files();
    snapshot(&render(&mut files, (420, 300)), "enso-files-narrow");
    assert!(
        !files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::Place(_))),
        "no sidebar when narrow"
    );

    let mut empty = steady_files("/home/me/new".into());
    let request = empty.pending.take().unwrap();
    let only_hidden = Listing::from_entries("/home/me/new", [(".keep", Kind::File)]);
    assert!(empty.pane.set_listing(request.generation, Ok(only_hidden)));
    snapshot(&render(&mut empty, (600, 300)), "enso-files-empty");
    assert_eq!(rows(&empty), 0);
}

fn picker(mode: Mode) -> Files {
    let mut request = pick::Request::new(mode);
    request.title = "Upload a file — file.io".into();
    request.folder = Some("/".into());
    request.filters = vec![
        Filter::parse("All files:*").unwrap(),
        Filter::parse("Images:*.png,*.jpg").unwrap(),
    ];
    request.choices = vec![pick::Choice::Toggle {
        id: "ro".into(),
        label: "Read-only".into(),
        on: true,
    }];
    let mut files = Files::picker(
        context(),
        FilesConfig::default(),
        request,
        Outcome::default(),
    );
    files.home = "/home/me".into();
    files.places = Vec::new();
    fill(&mut files);
    files
}

#[test]
fn renders_a_picker_with_its_basket() {
    let mut files = picker(Mode::Open {
        multiple: true,
        directory: false,
    });
    files.selection.insert("/Cargo.toml".into());
    files
        .selection
        .insert("/home/me/Pictures/samurai_bebop.png".into());
    files.selection.insert("/home/me/notes.txt".into());
    snapshot(&render(&mut files, (980, 480)), "enso-files-picker");
    let hits: Vec<&Hit> = files.hits.iter().map(|(_, hit)| hit).collect();
    assert!(hits.contains(&&Hit::Accept), "three chosen: ready");
    assert!(hits.contains(&&Hit::Cancel));
    assert!(hits.contains(&&Hit::FilterMenu));
    assert!(hits.contains(&&Hit::Choice(0)));
    assert!(hits.contains(&&Hit::Unchoose("/home/me/notes.txt".into())));
}

#[test]
fn renders_a_save_dialog_and_its_question() {
    let mut files = picker(Mode::Save {
        name: "report.pdf".into(),
    });
    snapshot(&render(&mut files, (900, 420)), "enso-files-save");
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::Name(_)))
    );
    files.overlay = Some(Overlay::Replace("/Cargo.toml".into()));
    snapshot(&render(&mut files, (900, 420)), "enso-files-replace");
    let hits: Vec<&Hit> = files.hits.iter().map(|(_, hit)| hit).collect();
    assert_eq!(
        hits,
        [&Hit::Replace, &Hit::KeepFile],
        "only the question answers"
    );
}

#[test]
fn renders_the_location_and_the_keys() {
    let mut files = files();
    let mut field = TextField::new("~/dev/en");
    field.select(0..0);
    files.location = Some(field);
    snapshot(&render(&mut files, (900, 300)), "enso-files-location");
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::Location(_)))
    );
    assert!(
        !files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::Crumb(_)))
    );
    files.location = None;
    files.overlay = Some(Overlay::Help);
    snapshot(&render(&mut files, (900, 560)), "enso-files-keys");
}

#[test]
fn renders_operations_and_their_questions() {
    let mut files = files();
    files.begin_rename("/home/me/dev/enso/Cargo.toml".into());
    files.running = Some(super::ops::Running {
        label: "Copying 3 items".into(),
        progress: enso_fs::ops::Progress {
            bytes: 40,
            total_bytes: 100,
            ..Default::default()
        },
        stop: Default::default(),
    });
    files.toast = Some((
        "Moved 2 items to the trash · Ctrl+Z puts them back".into(),
        Instant::now(),
    ));
    snapshot(&render(&mut files, (900, 420)), "enso-files-rename");
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::StopOp));

    let (reply, _answer) = std::sync::mpsc::channel();
    files.overlay = Some(Overlay::Asked(super::ops::Asked {
        source: "/tmp/notes.txt".into(),
        target: "/home/me/dev/enso/notes.txt".into(),
        reply,
        all: true,
    }));
    snapshot(&render(&mut files, (900, 420)), "enso-files-conflict");
    let hits: Vec<&Hit> = files.hits.iter().map(|(_, hit)| hit).collect();
    assert!(hits.contains(&&Hit::Resolve(enso_fs::ops::Resolve::KeepBoth)));
    assert!(hits.contains(&&Hit::ForAll));
    assert!(
        !hits.iter().any(|hit| matches!(hit, Hit::Row(_))),
        "only the question answers"
    );

    files.overlay = None;
    files.overlay = Some(Overlay::ConfirmDelete(vec![
        "/home/me/dev/enso/target".into(),
    ]));
    snapshot(&render(&mut files, (900, 420)), "enso-files-delete");
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::DeleteForGood));
}

#[test]
fn renders_settings_and_version_control() {
    let mut files = files();
    files.repo = Some((
        "/home/me/dev/enso".into(),
        enso_fs::git::Status {
            branch: Some("enso-files".into()),
            ahead: 2,
            tracking: true,
            ..Default::default()
        },
    ));
    files.overrides.git = Some(true);
    files.git_facts = super::vcs::GitFacts {
        available: true,
        global: enso_fs::git::Identity {
            name: Some("Anshul".into()),
            email: Some("me@example.com".into()),
        },
        own: None,
        forge_git: [false; 2],
    };
    files.overlay = Some(Overlay::Settings(super::settings::Panel::at(4)));
    snapshot(&render(&mut files, (900, 640)), "enso-files-settings");
    // In a window too short for every line, none runs under the footer.
    let done = files
        .hits
        .iter()
        .find(|(_, hit)| *hit == Hit::FormCancel)
        .map(|(rect, _)| *rect)
        .expect("the panel has its Done button");
    let lines: Vec<(Rect, usize)> = files
        .hits
        .iter()
        .filter_map(|(rect, hit)| match hit {
            Hit::Setting(index) if rect.h > 0.0 => Some((*rect, *index)),
            _ => None,
        })
        .collect();
    let rows = files.setting_rows().len();
    assert!(lines.len() < rows, "some lines wait below");
    assert!(lines.iter().all(|(rect, _)| rect.bottom() < done.y));
    // The last line, given the keyboard, scrolls into view, whole.
    files.overlay = Some(Overlay::Settings(super::settings::Panel::at(rows - 1)));
    snapshot(
        &settle(&mut files, (900, 640)),
        "enso-files-settings-scrolled",
    );
    let last = files
        .hits
        .iter()
        .find(|(_, hit)| *hit == Hit::Setting(rows - 1))
        .map(|(rect, _)| *rect)
        .expect("the last line shows");
    assert!(
        last.h >= 40.0 && last.bottom() < done.y,
        "{last:?} {done:?}"
    );
    files.overlay = Some(Overlay::Settings(super::settings::Panel::at(4)));
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::SortPill(_)))
    );
    assert_eq!(
        files
            .hits
            .iter()
            .filter(|(_, hit)| matches!(hit, Hit::ViewPill(_)))
            .count(),
        3,
        "list, grid, and columns can be chosen in settings"
    );
    files.overlay = None;
    snapshot(&render(&mut files, (900, 420)), "enso-files-branch");
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::Branch));

    files.overlay = Some(Overlay::Vcs(super::vcs::Vcs::Ask));
    snapshot(&render(&mut files, (900, 420)), "enso-files-ask-git");
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::GitYes));
    files.overlay = Some(Overlay::Vcs(super::vcs::Vcs::Identity(
        super::vcs::IdentityEdit {
            scope: enso_fs::git::Scope::Global,
            name: TextField::new("Anshul"),
            email: TextField::new("me@example.com"),
            focus: 1,
            error: None,
        },
    )));
    snapshot(&render(&mut files, (900, 420)), "enso-files-identity");
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::FormField(1)));
}

#[test]
fn renders_the_drives_and_the_trash() {
    let mut files = files();
    files.places = vec![super::Place {
        label: "Home".into(),
        icon: icon::HOME,
        path: "/home/me".into(),
    }];
    let volume = |label: Option<&str>, mount: Option<&str>, removable| enso_sys::udisks::Volume {
        object: format!(
            "/org/freedesktop/UDisks2/block_devices/{}",
            label.unwrap_or("x")
        ),
        device: "/dev/sdb1".into(),
        label: label.map(str::to_owned),
        kind: "ntfs".into(),
        size: 1_022_789_419_008,
        mount: mount.map(PathBuf::from),
        removable,
        system: !removable,
        drive: Some("/d".into()),
        can_eject: removable,
    };
    files.volumes = vec![
        volume(Some("STICK"), Some("/tmp"), true),
        volume(None, None, false),
    ];
    let frame = render(&mut files, (900, 520));
    snapshot(&frame, "enso-files-drives");
    let hits: Vec<&Hit> = files.hits.iter().map(|(_, hit)| hit).collect();
    assert!(hits.contains(&&Hit::Volume(1)));
    assert!(hits.contains(&&Hit::Eject(0)), "a mounted drive ejects");
    assert!(!hits.contains(&&Hit::Eject(1)), "an unmounted one doesn't");
    assert!(hits.contains(&&Hit::TrashPlace));
    assert!(hits.contains(&&Hit::Place("/".into())));
}

#[test]
fn renders_a_grid_with_thumbnails() {
    let mut files = files();
    files.config.view = crate::config::View::Grid;
    let (w, h) = (160u32, 100u32);
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| {
            let (x, y) = (i % w, i / w);
            [(x * 255 / w) as u8, (y * 255 / h) as u8, 200, 255]
        })
        .collect();
    files.thumbs.arrived(
        "/home/me/dev/enso/photo.png".into(),
        Some(enso_fs::thumbs::Thumb {
            width: w,
            height: h,
            rgba,
        }),
    );
    files.selection.insert("/home/me/dev/enso/README.md".into());
    files.pane.set_cursor(2);
    let frame = render(&mut files, (900, 520));
    snapshot(&frame, "enso-files-grid");
    let tiles = files
        .hits
        .iter()
        .filter(|(_, hit)| matches!(hit, Hit::Row(_)))
        .count();
    assert_eq!(tiles, 9, "every tile answers the pointer");
    assert!(files.columns() >= 4);
}

#[test]
fn renders_the_preview_and_the_quick_look() {
    let mut files = files();
    files.pane.set_cursor(6);
    files.previewing = true;
    let path = files.pane.path(6);
    files.shown.path = Some(path.clone());
    files.shown.side = 10_000;
    files.preview_arrived(
        path,
        crate::jobs::Preview::Text(vec![
            "[workspace]".into(),
            "resolver = \"3\"".into(),
            "members = [\"crates/*\"]".into(),
        ]),
    );
    snapshot(&render(&mut files, (1000, 480)), "enso-files-preview");

    files.pane.set_cursor(7);
    let picture = files.pane.path(7);
    assert!(picture.ends_with("photo.png"));
    files.shown = Default::default();
    files.shown.path = Some(picture.clone());
    files.shown.side = 10_000;
    let (w, h) = (300u32, 200u32);
    let rgba: Vec<u8> = (0..w * h)
        .flat_map(|i| [(i % w * 255 / w) as u8, 120, (i / w * 255 / h) as u8, 255])
        .collect();
    files.preview_arrived(
        picture,
        crate::jobs::Preview::Picture(enso_fs::thumbs::Thumb {
            width: w,
            height: h,
            rgba,
        }),
    );
    files.overlay = Some(Overlay::Peek);
    snapshot(&render(&mut files, (1000, 600)), "enso-files-peek");
    assert!(files.hits.is_empty(), "the quick look takes every click");
}

#[test]
fn a_dragged_box_chooses_what_it_touches() {
    let mut files = files();
    render(&mut files, (900, 520));
    let now = Instant::now();
    let list = files.list;
    let row = files.row_height();
    // From the empty space below the rows, up over the last three.
    files.begin_band(list.x + 300.0, list.bottom() - 4.0, false, now);
    assert!(files.drag_band(list.x + 40.0, list.y + row * 6.5, now));
    let chosen: Vec<String> = files
        .selection
        .paths()
        .iter()
        .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(chosen, ["Cargo.toml", "photo.png", "README.md"]);
    // Smaller again: only what it still touches.
    files.drag_band(list.x + 40.0, list.y + row * 8.5, now);
    assert_eq!(files.selection.len(), 1);
    assert!(files.end_band());
    assert!(!files.end_band());
}

#[test]
fn renders_columns() {
    let root = std::env::temp_dir().join(format!("enso-files-columns-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in [
        "dev/enso/crates/enso-ui",
        "dev/enso/crates/enso-fs",
        "dev/enso/docs",
        "dev/sumi",
        "dev/notes",
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    std::fs::write(root.join("dev/enso/Cargo.toml"), "").unwrap();
    let here = root.join("dev/enso");
    let mut files = steady_files(here.clone());
    files.places = Vec::new();
    files.config.view = crate::config::View::Columns;
    let request = files.pending.take().unwrap();
    assert!(
        files
            .pane
            .set_listing(request.generation, Ok(Listing::read(&here).unwrap()))
    );
    let frame = render(&mut files, (1100, 420));
    snapshot(&frame, "enso-files-columns");
    let sides: Vec<&Hit> = files
        .hits
        .iter()
        .map(|(_, hit)| hit)
        .filter(|hit| matches!(hit, Hit::Side(..)))
        .collect();
    // The folder above (enso, notes, sumi), and the cursor's folder
    // (crates: enso-fs, enso-ui).
    assert_eq!(sides.len(), 5, "{sides:?}");
    assert!(sides.contains(&&Hit::Side(root.join("dev/sumi"), false)));
    assert!(sides.contains(&&Hit::Side(here.join("crates/enso-ui"), true)));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn tabs_and_the_split() {
    let root = std::env::temp_dir().join(format!("enso-files-tabs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["music/albums", "photos/2026", "photos/2025"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let now = Instant::now();
    let mut files = steady_files(root.join("music"));
    files.places = Vec::new();
    files.asked_git = true;
    let first = files.pending.take().unwrap();
    assert!(files.listed_now(
        first.generation,
        Ok(Listing::read(root.join("music")).unwrap()),
        now
    ));

    // A second tab, behind: its listing finds it by its number.
    assert!(files.new_tab(root.join("photos"), false));
    assert_eq!((files.tabs.len(), files.active), (2, 0));
    let generation = files.tabs[1].as_ref().unwrap().generation();
    assert_ne!(
        generation,
        files.pane.generation(),
        "numbers are never shared"
    );
    assert_eq!(files.tab_of(generation), Some(1));
    let photos = Listing::read(root.join("photos")).unwrap();
    assert!(files.with_tab(1, |files| files.listed_now(generation, Ok(photos), now)));
    assert_eq!(
        files.pane.location(),
        root.join("music"),
        "the tab in front is untouched"
    );

    files.switch_tab(1);
    assert_eq!(files.pane.location(), root.join("photos"));
    assert_eq!(files.pane.len(), 2);
    assert_eq!(files.tab_location(0), root.join("music"));

    // Side by side: the other side's rows answer as the other side's.
    files.toggle_split();
    assert_eq!(files.split, Some(0));
    let frame = render(&mut files, (1100, 420));
    snapshot(&frame, "enso-files-split");
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::Tabs(_)))
    );
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::Other(inner) if **inner == Hit::Row(0)))
    );
    files.other_side();
    assert_eq!((files.active, files.split), (0, Some(1)));
    assert_eq!(files.pane.location(), root.join("music"));

    // Closing the one in front leaves the other, and no split.
    assert!(files.close_tab(0));
    assert_eq!((files.tabs.len(), files.active, files.split), (1, 0, None));
    assert_eq!(files.pane.location(), root.join("photos"));
    assert!(!files.close_tab(0), "the last tab is the window");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn renders_search_results() {
    use crate::pane::Source;
    let root = std::env::temp_dir().join(format!("enso-files-found-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/window")).unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "fn main() {\n    let sky = \"blue\";\n}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/window/draw.rs"), "// the sky, drawn\n").unwrap();
    std::fs::write(root.join("notes.md"), "# Notes\nLook at the sky tonight.\n").unwrap();
    let now = Instant::now();
    let mut files = steady_files(root.clone());
    files.asked_git = true;
    files.bookmarks = vec![enso_fs::bookmarks::Bookmark {
        path: root.join("src"),
        label: None,
    }];
    let first = files.pending.take().unwrap();
    files.listed_now(first.generation, Ok(Listing::read(&root).unwrap()), now);

    let source = Source::Search {
        query: "sky".into(),
        content: true,
    };
    let request = files.pane.show(root.clone(), source).unwrap();
    let none = std::sync::atomic::AtomicBool::new(false);
    let found = crate::gather::search(&root, false, "sky", true, &none).unwrap();
    files.pane.set_notes(found.notes);
    assert!(files.listed_now(request.generation, Ok(found.listing), now));
    assert_eq!(files.pane.len(), 3);
    assert!(files.pane.note(0).is_some_and(|note| note.contains("sky")));
    snapshot(&render(&mut files, (1000, 440)), "enso-files-search");
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::Recent));
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| *hit == Hit::Place(root.join("src")))
    );

    // Up from the results is the folder searched.
    let request = files.pane.parent().unwrap();
    assert_eq!(request.source, Source::Folder);
    assert_eq!(request.path, root);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_palette_finds_commands_and_chooses_by_kind() {
    let mut files = files();
    files.open_palette();
    {
        let palette = files.overlay.palette_mut().unwrap();
        palette.field.insert("grid");
        palette.refilter();
    }
    let palette = files.overlay.palette().unwrap();
    let first = &palette.lines[palette.shown[0]];
    assert_eq!(first.label, "Show as a grid");
    assert_eq!(first.aside, "Ctrl+2");
    {
        let palette = files.overlay.palette_mut().unwrap();
        palette.field.set_text("");
        palette.refilter();
    }
    snapshot(&render(&mut files, (900, 560)), "enso-files-palette");
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::PaletteLine(_)))
    );
    files.overlay = None;
    assert_eq!(files.choose_where("type:image"), 1);
    assert!(
        files
            .selection
            .paths()
            .iter()
            .any(|path| path.ends_with("photo.png"))
    );
}

#[test]
fn folders_slide_in_the_way_you_went() {
    let root = std::env::temp_dir().join(format!("enso-files-slide-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("inner")).unwrap();
    let now = Instant::now();
    let mut files = steady_files(root.clone());
    files.asked_git = true;
    let first = files.pending.take().unwrap();
    files.listed_now(first.generation, Ok(Listing::read(&root).unwrap()), now);
    files.navigate(root.join("inner"));
    assert_eq!(files.sliding, 1.0, "in: from the right");
    let generation = files.pane.generation();
    files.listed_now(
        generation,
        Ok(Listing::read(root.join("inner")).unwrap()),
        now,
    );
    let request = files.pane.parent();
    files.load(request);
    assert_eq!(files.sliding, -1.0, "out: from the left");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn renames_many_at_once_and_back() {
    let root = std::env::temp_dir().join(format!("enso-files-bulk-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    for name in ["IMG_0031.jpg", "IMG_0032.jpg", "IMG_0040.jpg", "notes.txt"] {
        std::fs::write(root.join(name), name).unwrap();
    }
    let now = Instant::now();
    let mut files = steady_files(root.clone());
    files.asked_git = true;
    let first = files.pending.take().unwrap();
    files.listed_now(first.generation, Ok(Listing::read(&root).unwrap()), now);
    assert!(!files.rename_targets(), "one thing: the usual rename");
    files.choose_where("ext:jpg");
    files.config.bulk_rename = false;
    assert!(!files.rename_targets(), "off in Settings");
    files.config.bulk_rename = true;
    assert!(files.rename_targets());
    {
        let renamer = files.overlay.renamer_mut().unwrap();
        renamer.fields[1].set_text("Holiday {n:02}");
        renamer.replan();
    }
    snapshot(&render(&mut files, (900, 600)), "enso-files-rename-many");
    assert_eq!(files.commit_renames(), Some(Ok(3)));
    for name in [
        "Holiday 01.jpg",
        "Holiday 02.jpg",
        "Holiday 03.jpg",
        "notes.txt",
    ] {
        assert!(root.join(name).exists(), "{name}");
    }
    assert!(files.selection.contains(&root.join("Holiday 02.jpg")));
    // One undo puts every name back.
    let done = files.undo.pop().unwrap();
    enso_fs::ops::undo(&done).unwrap();
    assert!(root.join("IMG_0040.jpg").exists());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn properties_count_and_permissions_change() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("enso-files-props-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("album")).unwrap();
    std::fs::write(root.join("album/a.jpg"), [0; 1500]).unwrap();
    std::fs::write(root.join("run.sh"), "echo hi").unwrap();
    std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o644)).unwrap();
    let now = Instant::now();
    let mut files = steady_files(root.clone());
    files.asked_git = true;
    let first = files.pending.take().unwrap();
    files.listed_now(first.generation, Ok(Listing::read(&root).unwrap()), now);

    // A folder: counted (here, by hand) as it would be in the background.
    files.pane.set_cursor(0);
    files.open_properties();
    let token = files.tallies;
    let tally = enso_fs::tally::tally(
        &[root.join("album")],
        &std::sync::atomic::AtomicBool::new(false),
        |_| {},
    );
    files.tallied(token, tally, true);
    snapshot(&render(&mut files, (900, 720)), "enso-files-properties");
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| *hit == Hit::Permission(0o200))
    );

    // A file: others may now write, then not.
    files.overlay = None;
    files.pane.set_cursor(1);
    files.open_properties();
    let mode = |files: &Files| {
        let _ = files;
        std::fs::metadata(root.join("run.sh"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(files.flip_bit(0o002), 0);
    assert_eq!(mode(&files), 0o646);
    files.flip_bit(0o002);
    assert_eq!(mode(&files), 0o644);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn folders_keep_their_own_view() {
    use crate::config::{SortBy, View};
    let root = std::env::temp_dir().join(format!("enso-files-views-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["pictures", "papers"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    let mut files = steady_files(root.clone());
    files.folder_views.change(&root.join("pictures"), |own| {
        own.view = Some(View::Grid);
        own.zoom = Some(3);
    });
    files.folder_views.change(&root.join("papers"), |own| {
        own.sort = Some(SortBy::Modified);
        own.descending = Some(true);
    });
    files.navigate(root.join("pictures"));
    assert_eq!((files.config.view, files.zoom), (View::Grid, 3));
    files.shown_at = root.join("pictures");
    files.navigate(root.join("papers"));
    assert_eq!(files.config.view, View::List, "the default again");
    assert_eq!(files.zoom, super::grid::ZOOM);
    assert!(files.pane.sort().descending);
    files.shown_at = root.join("papers");
    files.navigate(root.clone());
    assert!(!files.pane.sort().descending);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn archives_are_walked_like_folders() {
    use crate::pane::Source;
    let root = std::env::temp_dir().join(format!("enso-files-zip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("trip/day 1")).unwrap();
    std::fs::write(root.join("trip/day 1/beach.jpg"), [0; 2048]).unwrap();
    std::fs::write(root.join("trip/notes.md"), "sun").unwrap();
    struct Quiet;
    impl enso_fs::ops::Watcher for Quiet {
        fn progress(&mut self, _: &enso_fs::ops::Progress) {}
        fn conflict(
            &mut self,
            _: &std::path::Path,
            _: &std::path::Path,
        ) -> (enso_fs::ops::Resolve, bool) {
            (enso_fs::ops::Resolve::Skip, true)
        }
        fn stop(&self) -> bool {
            false
        }
    }
    let zip = root.join("trip.zip");
    enso_fs::archive::compress(&[root.join("trip")], &zip, &mut Quiet);
    let now = Instant::now();
    let mut files = steady_files(root.clone());
    files.asked_git = true;
    let first = files.pending.take().unwrap();
    files.listed_now(first.generation, Ok(Listing::read(&root).unwrap()), now);

    // In, and into a folder in it.
    let arrive = |files: &mut Files| {
        let Source::Archive { inner } = files.pane.source().clone() else {
            panic!("not in the archive");
        };
        let found = crate::gather::archive(files.pane.location(), &inner).unwrap();
        let generation = files.pane.generation();
        assert!(files.listed_now(generation, Ok(found.listing), now));
    };
    files.navigate(zip.clone());
    arrive(&mut files);
    files.navigate(zip.join("trip"));
    arrive(&mut files);
    assert_eq!(files.pane.len(), 2);
    assert_eq!(
        files.pane.entry(0).meta.map(|meta| meta.size),
        Some(0),
        "a folder"
    );
    snapshot(&render(&mut files, (900, 400)), "enso-files-archive");
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| *hit == Hit::Crumb(zip.clone()))
    );

    // Out a folder, then out beside it, the cursor on it.
    let request = files.pane.parent().unwrap();
    assert_eq!(
        request.source,
        Source::Archive {
            inner: String::new()
        }
    );
    let request = {
        files.load(Some(request));
        arrive(&mut files);
        files.pane.parent().unwrap()
    };
    assert_eq!(
        (request.path.as_path(), request.source),
        (root.as_path(), Source::Folder)
    );

    // Compress…: its question.
    let generation = request.generation;
    files.listed_now(generation, Ok(Listing::read(&root).unwrap()), now);
    files.selection.insert(root.join("trip"));
    files.overlay = None;
    files.pane.set_cursor(0);
    files.ask_compress_for_tests();
    snapshot(&render(&mut files, (900, 520)), "enso-files-compress");
    assert!(
        files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::Pack(_)))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn drops_land_in_the_folder_under_them() {
    let mut files = files();
    render(&mut files, (900, 480));
    let row_at = |files: &Files, wanted: usize| {
        files
            .hits
            .iter()
            .find(|(_, hit)| *hit == Hit::Row(wanted))
            .map(|(rect, _)| rect.center())
            .unwrap()
    };
    // Row 0 is a folder (crates): into it.
    let (x, y) = row_at(&files, 0);
    let (into, lit) = files.drop_target_for_tests(x, y).unwrap();
    assert_eq!(into, files.pane.path(0));
    assert!(lit.is_some());
    // A file's row: the folder shown, the whole list lit.
    let (x, y) = row_at(&files, 6);
    let (into, lit) = files.drop_target_for_tests(x, y).unwrap();
    assert_eq!(into, files.pane.location());
    assert!(lit.is_none());
    files.drop_at = Some((into, None));
    snapshot(&render(&mut files, (900, 480)), "enso-files-drop");
}

#[test]
fn the_branch_menu_reaches_the_remote() {
    use super::vcs::BranchLine;
    let root = std::env::temp_dir().join(format!("enso-files-remote-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("work")).unwrap();
    let work = root.join("work");
    let git = |dir: &std::path::Path, args: &[&str]| {
        let done = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            done.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&done.stderr)
        );
    };
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
    git(&work, &["push", "--quiet", "-u", "origin", "main"]);
    git(&work, &["push", "--quiet", "origin", "main:feature"]);
    git(&work, &["fetch", "--quiet"]);

    let mut files = files();
    let status = enso_fs::git::status(&work).unwrap();
    files.repo = Some((work.clone(), status));
    files.branch_menu(Rect::new(700.0, 400.0, 100.0, 24.0));
    let Some((_, super::MenuFor::Branches(lines))) = files.overlay.menu() else {
        panic!("no branch menu");
    };
    let lines: Vec<&BranchLine> = lines.iter().flatten().collect();
    assert!(lines.contains(&&BranchLine::Local("main".into())));
    assert!(lines.contains(&&BranchLine::Remote("origin/feature".into())));
    assert!(
        !lines.contains(&&BranchLine::Remote("origin/main".into())),
        "main is here already"
    );
    for wanted in [BranchLine::Fetch, BranchLine::Pull, BranchLine::Push] {
        assert!(lines.contains(&&wanted), "{wanted:?}");
    }
    snapshot(&render(&mut files, (900, 480)), "enso-files-branches");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn renders_github_sign_in_clone_and_publish() {
    use super::vcs::Vcs;
    let mut files = files();
    files.overlay = Some(Overlay::Vcs(Vcs::Device {
        site: crate::forge::Site::GitHub,
        code: "4F2A-91CD".into(),
        page: "https://github.com/login/device".into(),
        error: None,
    }));
    snapshot(&render(&mut files, (900, 480)), "enso-files-github-code");
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::FormSave));

    files.overrides.github_account = Some("Formalemon".into());
    files.repos = ["formalemon/enso", "formalemon/sumi", "formalemon/notes"]
        .into_iter()
        .map(|name| crate::forge::Repo {
            name: name.into(),
            clone_url: format!("https://github.com/{name}.git"),
            private: name.ends_with("notes"),
            description: String::new(),
            forge: "GitHub",
        })
        .collect();
    files.overlay = Some(Overlay::Vcs(Vcs::Clone(
        enso_ui::widget::TextField::new("su"),
        None,
    )));
    assert_eq!(files.repos_shown()[0].name, "formalemon/sumi");
    snapshot(&render(&mut files, (900, 520)), "enso-files-github-clone");
    // An address typed: no list.
    files.overlay = Some(Overlay::Vcs(Vcs::Clone(
        enso_ui::widget::TextField::new("https://example.com/x.git"),
        None,
    )));
    assert!(files.repos_shown().is_empty());

    files.overlay = Some(Overlay::Vcs(Vcs::Publish(super::vcs::PublishForm {
        name: enso_ui::widget::TextField::new("enso"),
        private: true,
        site: crate::forge::Site::GitHub,
        error: None,
    })));
    snapshot(&render(&mut files, (900, 480)), "enso-files-github-publish");
    // One forge: no choice of where.
    assert!(
        !files
            .hits
            .iter()
            .any(|(_, hit)| matches!(hit, Hit::FormChoice(_)))
    );
}

#[test]
fn gitlab_asks_where_it_is_and_publish_offers_both() {
    use super::vcs::{AppForm, PublishForm, Vcs};
    use crate::forge::Site;
    let mut files = files();
    files.config.gitlab_host = "gitlab.com".into();
    // GitLab's app: where it is, then its ID.
    files.overlay = Some(Overlay::Vcs(Vcs::ClientId(AppForm {
        site: Site::GitLab,
        id: enso_ui::widget::TextField::default(),
        host: enso_ui::widget::TextField::new("git.example.org"),
        focus: 1,
        error: None,
    })));
    snapshot(&render(&mut files, (900, 520)), "enso-files-gitlab-app");
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::FormField(0)));
    assert!(files.hits.iter().any(|(_, hit)| *hit == Hit::FormField(1)));

    // Signed in to both: publish asks which.
    files.overrides.github_account = Some("formalemon".into());
    files.overrides.gitlab_account = Some("formalemon".into());
    files.overlay = Some(Overlay::Vcs(Vcs::Publish(PublishForm {
        name: enso_ui::widget::TextField::new("sumi"),
        private: true,
        site: Site::GitLab,
        error: None,
    })));
    snapshot(&render(&mut files, (900, 520)), "enso-files-publish-where");
    let choices: Vec<_> = files
        .hits
        .iter()
        .filter(|(_, hit)| matches!(hit, Hit::FormChoice(_)))
        .collect();
    assert_eq!(choices.len(), 2);
}

#[test]
fn crumbs_start_at_home_or_the_root() {
    let home = Path::new("/home/me");
    let labels = |path: &str| -> Vec<String> {
        crumbs(Path::new(path), home)
            .into_iter()
            .map(|(label, _)| label)
            .collect()
    };
    assert_eq!(labels("/home/me/dev/enso"), ["~", "dev", "enso"]);
    assert_eq!(labels("/home/me"), ["~"]);
    assert_eq!(labels("/usr/lib"), ["/", "usr", "lib"]);
    assert_eq!(labels("/"), ["/"]);
}

#[test]
fn titles_name_the_folder() {
    let home = Path::new("/home/me");
    assert_eq!(title(Path::new("/home/me"), home), "Home — Files");
    assert_eq!(title(Path::new("/home/me/dev"), home), "dev — Files");
    assert_eq!(title(Path::new("/"), home), "/ — Files");
}

#[test]
fn a_drag_shows_what_is_held() {
    let files = files();
    let mut text = TextEngine::new(&files.ctx.config.appearance);
    let one = files.held(&["/home/me/dev/enso/Cargo.toml".into()]);
    let many = files.held(&[
        "/home/me/dev/enso/Cargo.toml".into(),
        "/home/me/dev/enso/README.md".into(),
        "/home/me/dev/enso/CHANGES.md".into(),
    ]);
    assert_eq!(one.label, "Cargo.toml");
    assert_eq!(many.label, "3 items");
    for (held, name) in [(one, "enso-files-drag-one"), (many, "enso-files-drag-many")] {
        // Sized to what it says, as the drag sizes it.
        let mut label = 0.0;
        Offscreen::render(&mut text, (1, 1), 1, |canvas| {
            label = canvas
                .measure(&enso_ui::Text::new(&held.label, held.size * 0.9, held.text))
                .w;
        });
        let width = held.width(label).ceil() as u32;
        assert!(width > 60 && width <= 280, "{width}");
        let picture = Offscreen::render(&mut text, (width, 34), 2, |canvas| held.draw(canvas));
        // Opaque in its middle, clear at its rounded corners.
        assert_eq!(picture.pixel(picture.width / 2, picture.height / 2)[3], 255);
        assert_eq!(picture.pixel(0, 0)[3], 0);
        snapshot(&picture, name);
    }
}
