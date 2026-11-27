use std::time::{Duration, Instant};

use enso_core::Paths;
use enso_media::{Media, Tags, probe::Audio};
use enso_ui::{Offscreen, TextEngine};

use super::*;

fn listener(count: usize, at: usize) -> Listener {
    let root = std::env::temp_dir().join("enso-listen-test");
    let paths = Paths::rooted(&root);
    let _ = enso_core::theme::set_active(&paths, "rose-pine-moon");
    let ctx = Context::load_from(paths);
    let titles = [
        "Weightless",
        "Electra",
        "Mellomaniac",
        "Watermark",
        "Strawberry Swing",
    ];
    let tracks = (0..count)
        .map(|index| Track {
            path: PathBuf::from(format!(
                "/music/{:02} {}.flac",
                index + 1,
                titles[index % 5]
            )),
            media: Some(Media {
                duration: Some(Duration::from_secs(240 + index as u64 * 17)),
                audio: Some(Audio {
                    rate: 44_100,
                    channels: 2,
                    codec: "flac".to_owned(),
                }),
                tags: Tags {
                    title: Some(titles[index % 5].to_owned()),
                    artist: Some("Marconi Union".to_owned()),
                    album: Some("Ambient Transmissions".to_owned()),
                    ..Tags::default()
                },
                ..Media::default()
            }),
        })
        .collect();
    Listener::new(ctx, Queue { tracks, at })
}

#[test]
fn the_window_as_it_looks() {
    let mut listener = listener(5, 1);
    let mut text = TextEngine::new(&listener.ctx.config.appearance);
    let now = Instant::now() + Duration::from_secs(1);
    let image = Offscreen::render(&mut text, (440, 720), 1, |canvas| {
        listener.paint(canvas, now);
    });
    image.snapshot(env!("CARGO_MANIFEST_DIR"), "listen");
    let layout = listener.layout(listener.bounds);
    let centre = |rect: Rect| (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
    let (x, y) = centre(layout.play);
    assert_eq!(listener.hit(x, y, now), Some(Hit::PlayPause));
    let list = layout.list.unwrap();
    let row = list.y + space::SM + ROW * 2.5;
    assert_eq!(listener.hit(list.x + 60.0, row, now), Some(Hit::Row(2)));
}

#[test]
fn previous_a_little_way_in_starts_it_again() {
    let mut listener = listener(3, 1);
    assert_eq!(listener.queue.neighbours(), (true, true));
    // Not playing anything yet: previous goes to the one before.
    listener.skip(false);
    assert_eq!(listener.queue.at, 0);
    listener.skip(true);
    assert_eq!(listener.queue.at, 1);
}
