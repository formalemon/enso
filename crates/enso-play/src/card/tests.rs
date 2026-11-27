use std::time::{Duration, Instant};

use enso_core::{Context, Paths};
use enso_sys::mpris::{Player, Status, Track};
use enso_ui::{Offscreen, TextEngine};

use super::*;

fn player(bus: &str, status: Status) -> Player {
    Player {
        bus: format!("org.mpris.MediaPlayer2.{bus}"),
        name: "Firefox".into(),
        status,
        track: Track {
            id: Some("/org/mpris/MediaPlayer2/firefox".into()),
            title: "Rain on a tin roof, three hours".into(),
            artist: "Quiet Hours".into(),
            album: String::new(),
            length: Some(Duration::from_secs(3 * 3600)),
            art: None,
        },
        position: Duration::from_secs(4231),
        read: Instant::now(),
        rate: 1.0,
        can_play: true,
        can_pause: true,
        can_next: false,
        can_previous: false,
        can_seek: true,
    }
}

fn card_in(
    name: &str,
    mode: Mode,
    players: Vec<Player>,
) -> (Card, std::sync::mpsc::Receiver<Request>) {
    // Cleared and reused by the next run, so they don't pile up.
    let root = std::env::temp_dir().join(format!("enso-play-test-{name}"));
    let _ = std::fs::remove_dir_all(&root);
    let paths = Paths::rooted(root);
    enso_core::theme::set_active(&paths, "rose-pine-moon").expect("write theme state");
    let (worker, inbox) = Worker::new();
    let card = Card::new(Context::load_from(paths), mode, players, worker, None);
    (card, inbox.requests)
}

fn surface(card: &Card) -> Rect {
    let spec = card.surface();
    Rect::new(0.0, 0.0, spec.width as f32, spec.height as f32)
}

#[test]
fn the_card_hangs_under_the_clock_and_keeps_its_hands_off_the_keyboard() {
    for edge in [Edge::Top, Edge::Bottom] {
        let (mut card, _) = card_in(
            &format!("hang-{edge:?}"),
            Mode::Dropdown(edge),
            vec![player("a", Status::Playing), player("b", Status::Paused)],
        );
        let spec = card.surface();
        assert_eq!(spec.keyboard, Keyboard::None);
        assert_eq!(spec.exclusive_zone, 0);
        let bounds = surface(&card);
        card.bounds = bounds;
        let rect = card.card_rect(bounds);
        // Across the middle, as the clock is, and inside the surface.
        assert!((rect.x + rect.w / 2.0 - bounds.w / 2.0).abs() <= 1.0);
        assert!(rect.y >= 0.0 && rect.bottom() <= bounds.bottom());
        match edge {
            Edge::Top => assert_eq!(rect.y, DROP_GAP),
            Edge::Bottom => assert_eq!(rect.bottom(), bounds.bottom() - DROP_GAP),
        }
        // Wanted while the bar says the pointer is on the clock…
        assert!(card.wanted());
        card.owner_hovering = false;
        assert!(!card.wanted());
        // …or while the thumb is held.
        card.dragging = Some(0.4);
        assert!(card.wanted());
    }
}

#[test]
fn it_follows_whatever_plays_unless_a_player_was_chosen() {
    let (mut card, _) = card_in(
        "follow",
        Mode::Modal,
        vec![player("a", Status::Playing), player("b", Status::Paused)],
    );
    assert!(card.current().unwrap().bus.ends_with(".a"));
    // b starts playing: the players come back in a new order.
    card.set_players(vec![
        player("b", Status::Playing),
        player("a", Status::Paused),
    ]);
    assert!(card.current().unwrap().bus.ends_with(".b"));
    // Chosen with Tab, a player stays shown as the order changes…
    card.on_key(KeyEvent {
        key: Key::Tab,
        mods: Default::default(),
        repeat: false,
    });
    assert!(card.current().unwrap().bus.ends_with(".a"));
    card.set_players(vec![
        player("b", Status::Playing),
        player("a", Status::Paused),
    ]);
    assert!(card.current().unwrap().bus.ends_with(".a"));
    // …until it goes away.
    card.set_players(vec![player("b", Status::Playing)]);
    assert!(card.current().unwrap().bus.ends_with(".b"));
}

#[test]
fn the_buttons_and_the_timeline_do_what_they_show() {
    let (mut card, requests) = card_in("buttons", Mode::Modal, vec![player("a", Status::Playing)]);
    card.bounds = Rect::new(0.0, 0.0, 1280.0, 800.0);
    let layout = card.layout(card.card_rect(card.bounds));
    let centre = |rect: Rect| (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);

    let (x, y) = centre(layout.play);
    assert_eq!(card.hit(x, y), Some(Hit::PlayPause));
    card.play_pause();
    // It answers at once: paused, the wave lies down.
    assert_eq!(card.current().unwrap().status, Status::Paused);
    assert_eq!(card.playing.target(), 0.0);

    let (x, y) = centre(layout.track);
    assert_eq!(card.hit(x, y), Some(Hit::Track));
    card.seek_to(0.5);
    assert_eq!(
        card.current().unwrap().position,
        Duration::from_secs(90 * 60)
    );

    // Skipping where the player can't is nothing at all.
    card.skip(true);
    let sent: Vec<Request> = requests.try_iter().collect();
    assert!(matches!(sent[0], Request::PlayPause(_)));
    assert!(matches!(sent[1], Request::Seek(_, to) if to == Duration::from_secs(90 * 60)));
    assert_eq!(sent.len(), 2);
}

#[test]
fn times_read_as_clocks_do() {
    assert_eq!(clock(Duration::from_secs(7)), "0:07");
    assert_eq!(clock(Duration::from_secs(187)), "3:07");
    assert_eq!(clock(Duration::from_secs(3765)), "1:02:45");
}

#[test]
fn the_card_as_it_looks() {
    let (mut card, _) = card_in(
        "look",
        Mode::Dropdown(Edge::Top),
        vec![player("a", Status::Playing), player("b", Status::Paused)],
    );
    let spec = card.surface();
    let appearance = card.ctx.config.appearance.clone();
    let mut text = TextEngine::new(&appearance);
    // Fully shown, mid-roll.
    let now = Instant::now() + Duration::from_secs(2);
    card.opened = now - Duration::from_millis(700);
    // The reveal began a moment ago and is over.
    card.presence.get(now - Duration::from_secs(1));
    let image = Offscreen::render(&mut text, (spec.width, spec.height), 2, |canvas| {
        canvas.clear(card.ctx.theme.colors.base);
        card.paint(canvas, now);
    });
    image.check(env!("CARGO_MANIFEST_DIR"), "play-card");
}

#[test]
fn two_fingers_sideways_go_from_player_to_player() {
    let mut other = player("b", Status::Paused);
    other.name = "Spotify".into();
    other.track.title = "Weightless".into();
    other.track.artist = "Marconi Union".into();
    other.track.length = Some(Duration::from_secs(480));
    other.position = Duration::from_secs(95);
    let (mut card, _) = card_in(
        "swipe",
        Mode::Dropdown(Edge::Top),
        vec![player("a", Status::Playing), other],
    );
    card.bounds = surface(&card);
    let width = card.card_rect(card.bounds).w;
    let start = Instant::now();
    let at = |ms: u64| start + Duration::from_millis(ms);
    assert_eq!(card.neighbours(), (false, true));
    // Nothing before the first: it holds its ground.
    for i in 0..6 {
        card.slide.scroll((-30.0, 0.0, false, false), card.neighbours(), width, at(i * 10));
    }
    assert_eq!(
        card.slide.scroll((0.0, 0.0, false, true), card.neighbours(), width, at(70)),
        Swiped::Moved
    );
    // Half way across: the next is coming in beside it.
    for i in 0..8 {
        card.slide.scroll((20.0, 0.0, false, false), card.neighbours(), width, at(1000 + i * 10));
    }
    let spec = card.surface();
    let appearance = card.ctx.config.appearance.clone();
    let mut text = TextEngine::new(&appearance);
    card.presence.get(at(0));
    let image = Offscreen::render(&mut text, (spec.width, spec.height), 2, |canvas| {
        canvas.clear(card.ctx.theme.colors.base);
        card.paint(canvas, at(1080));
    });
    image.snapshot(env!("CARGO_MANIFEST_DIR"), "play-swipe");
    match card.slide.scroll((0.0, 0.0, false, true), card.neighbours(), width, at(1085)) {
        Swiped::Go(direction) => card.swiped(direction),
        other => panic!("{other:?}"),
    }
    assert!(card.current().unwrap().bus.ends_with(".b"));
    // It arrived drawn as it stands: paused, its wave already flat.
    assert_eq!(card.playing.get(at(1085)), 0.0);
    assert_eq!(card.neighbours(), (true, false));
}
