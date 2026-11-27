use std::time::{Duration, Instant};

use enso_core::Paths;
use enso_fs::picture::Details;
use enso_media::{Media, probe::Video as Pictures};
use enso_ui::{Offscreen, TextEngine};

use super::*;

/// An 800 × 400 picture, its left half red, its right half blue.
fn wide() -> Photo {
    let mut rgba = Vec::new();
    for _ in 0..400 {
        for x in 0..800 {
            rgba.extend(if x < 400 {
                [200, 40, 40, 255]
            } else {
                [40, 40, 200, 255]
            });
        }
    }
    Photo::new(800, 400, rgba)
}

/// A viewer of an album: `names` in a made-up folder, the one at `at`
/// read as `item`.
fn viewer(names: &[&str], at: usize, item: Item) -> Viewer {
    let root = std::env::temp_dir().join("enso-view-test");
    let paths = Paths::rooted(&root);
    let _ = enso_core::theme::set_active(&paths, "rose-pine-moon");
    let ctx = Context::load_from(paths);
    let items: Vec<PathBuf> = names
        .iter()
        .map(|name| PathBuf::from("/pictures").join(name))
        .collect();
    let path = items[at].clone();
    let mut viewer = Viewer::new(ctx, Album { items, at });
    viewer.autoplay = None;
    viewer.read.insert(path, Ok(Arc::new(item)));
    viewer.show_current();
    viewer
}

fn picture() -> Item {
    Item::Picture {
        photo: Arc::new(wide()),
        details: Details {
            taken: Some("2026:11:02 10:00:00".to_owned()),
            camera: Some("Google Pixel 8".to_owned()),
        },
    }
}

fn frame(viewer: &mut Viewer, size: (u32, u32), now: Instant) -> Offscreen {
    let mut text = TextEngine::new(&viewer.ctx.config.appearance);
    Offscreen::render(&mut text, size, 1, |canvas| {
        viewer.draw(canvas, now);
    })
}

#[test]
fn fits_the_stage_and_zooms_about_the_pointer() {
    let mut viewer = viewer(&["wide.png"], 0, picture());
    let now = Instant::now() + Duration::from_secs(2);
    let shown = frame(&mut viewer, (600, 500), now);
    let photo = viewer.photo().unwrap();
    // One picture: no strip. The stage is between the title and the status.
    assert_eq!(
        viewer.stage,
        Rect::new(0.0, HEADER, 600.0, 500.0 - HEADER - STATUS)
    );
    // Fitted inside a margin.
    let placed = viewer.placement(&photo);
    assert_eq!((placed.w, placed.h), (568.0, 284.0));
    assert_eq!(placed.x, 16.0);
    let middle = viewer.stage.y + viewer.stage.h / 2.0;
    assert_eq!(placed.y, (middle - 142.0).round());
    assert_eq!(shown.pixel(100, middle as u32)[..3], [200, 40, 40]);
    assert_eq!(shown.pixel(500, middle as u32)[..3], [40, 40, 200]);

    // Every pixel, around a point on the blue half: it stays put.
    let at = (450.0, middle);
    let before = viewer.placement(&photo);
    let point = (
        (at.0 - before.x) / before.w * 800.0,
        (at.1 - before.y) / before.h * 400.0,
    );
    viewer.zoom_to(1.0, Some(at));
    let placed = viewer.placement(&photo);
    assert_eq!(placed.w, 800.0);
    let under = (at.0 - placed.x, at.1 - placed.y);
    assert!(
        (under.0 - point.0).abs() < 1.5 && (under.1 - point.1).abs() < 1.5,
        "{under:?} {point:?}"
    );
    // Back to fit; never enlarged past it by fitting.
    viewer.toggle_actual(None);
    assert_eq!(viewer.view, View::FIT);
    frame(&mut viewer, (1600, 1000), now);
    assert_eq!(viewer.placement(&photo).w, 800.0);
}

#[test]
fn turning_swaps_the_sides() {
    let mut viewer = viewer(&["wide.png"], 0, picture());
    viewer.turn(true);
    assert_eq!(viewer.photo().unwrap().size(), (400, 800));
    // Still the one read, so stepping away and back keeps it turned.
    let read = viewer.read.values().next().unwrap().as_ref().unwrap();
    assert_eq!(read.size(), Some((400, 800)));
}

#[test]
fn the_window_as_it_looks() {
    let names = ["a.png", "b.jpg", "wide.png", "d.mp4", "e.png", "f.png"];
    let mut viewer = viewer(&names, 2, picture());
    viewer.info = true;
    let now = Instant::now() + Duration::from_secs(2);
    let shown = frame(&mut viewer, (1100, 640), now);
    shown.snapshot(env!("CARGO_MANIFEST_DIR"), "view");
    let hits: Vec<Hit> = viewer.hits.iter().map(|(_, hit)| *hit).collect();
    for wanted in [
        Hit::Back,
        Hit::Forward,
        Hit::Info,
        Hit::Turn,
        Hit::Trash,
        Hit::Thumb(0),
    ] {
        assert!(hits.contains(&wanted), "{wanted:?}");
    }
    // Half a swipe on: the next comes in beside it, the strip following.
    let width = viewer.stage.w;
    let start = Instant::now();
    for i in 0..10 {
        viewer.slide.scroll(
            (width / 20.0, 0.0, false, false),
            viewer.album.neighbours(),
            width,
            start + Duration::from_millis(i * 10),
        );
    }
    viewer.read.insert(
        PathBuf::from("/pictures/d.mp4"),
        Ok(Arc::new(still_video())),
    );
    frame(&mut viewer, (1100, 640), start + Duration::from_millis(100))
        .snapshot(env!("CARGO_MANIFEST_DIR"), "view-swipe");
}

fn still_video() -> Item {
    let mut rgba = Vec::new();
    for y in 0..360 {
        for x in 0..640 {
            rgba.extend([(x / 3) as u8, (y / 2) as u8, 120, 255]);
        }
    }
    Item::Video {
        media: Media {
            duration: Some(Duration::from_secs(296)),
            video: Some(Pictures {
                width: 640,
                height: 360,
                rate: 30.0,
                codec: "h264".to_owned(),
            }),
            format: "MP4".to_owned(),
            ..Media::default()
        },
        poster: Some(Arc::new(Photo::new(640, 360, rgba))),
    }
}

#[test]
fn a_video_waits_with_its_big_button_and_its_details() {
    let mut viewer = viewer(&["a.png", "d.mp4"], 1, still_video());
    viewer.info = true;
    let now = Instant::now() + Duration::from_secs(2);
    frame(&mut viewer, (1100, 640), now).snapshot(env!("CARGO_MANIFEST_DIR"), "view-video");
    let hits: Vec<Hit> = viewer.hits.iter().map(|(_, hit)| *hit).collect();
    assert!(hits.contains(&Hit::Start), "{hits:?}");
    assert!(!hits.contains(&Hit::Turn), "videos aren't turned");
}

#[test]
fn keeps_in_full_only_the_one_on_screen() {
    let path = |name: &str| PathBuf::from("/pictures").join(name);
    let whole = |viewer: &Viewer, name: &str| {
        viewer.read[&path(name)]
            .as_ref()
            .is_ok_and(|item| item.is_whole())
    };
    let mut viewer = viewer(&["a.png", "b.png", "c.png"], 1, picture());
    // A small window: those either side need only their half.
    viewer.stage = Rect::new(0.0, 0.0, 300.0, 200.0);
    let read = viewer.settle(&path("a.png"), Arc::new(picture()));
    assert!(!read.is_whole());
    assert_eq!(read.size(), Some((800, 400)));
    viewer.read.insert(path("a.png"), Ok(read));
    viewer.read.insert(path("c.png"), Ok(Arc::new(picture())));
    viewer.want();
    assert!(whole(&viewer, "b.png"));
    assert!(!whole(&viewer, "a.png") && !whole(&viewer, "c.png"));

    // Stepped on, it shows at once, small, and the one left is let go of.
    viewer.album.at = 2;
    viewer.want();
    assert!(!whole(&viewer, "b.png"));
    assert!(!viewer.item().unwrap().is_whole());
    // Its full pixels come: the same one, sharper.
    let full = viewer.settle(&path("c.png"), Arc::new(picture()));
    viewer.read.insert(path("c.png"), Ok(full));
    viewer.show_current();
    assert!(viewer.item().unwrap().is_whole());

    // Turned, and read again in full: it comes turned as it was.
    viewer.turn(true);
    let again = viewer.settle(&path("c.png"), Arc::new(picture()));
    assert_eq!(again.size(), Some((400, 800)));
    assert!(again.is_whole());
}
