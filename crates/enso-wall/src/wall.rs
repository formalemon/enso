//! The wallpaper itself: what is on each screen, and the fade between two.

use std::{
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

use enso_core::{
    Context, Rgba, Transition, Tween,
    wallpaper::{self, Chosen, Fit, WallConfig},
};
use enso_ui::{Canvas, Cx, Event, Paint, Picture, Proxy, Role, Screen, Screens, watch::watch_dir};

/// Messages from the background threads.
#[derive(Debug)]
pub enum Message {
    /// Something in the state directory changed: the theme, or a wallpaper
    /// chosen with `enso-wall set`.
    StateChanged,
}

/// What a screen shows: a picture, or failing that a colour.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub picture: Option<PathBuf>,
    /// When the file was last written, so that replacing a picture in place
    /// counts as a change.
    modified: Option<SystemTime>,
    /// The theme's base colour: the whole scene when there is no picture,
    /// the surround when a picture does not fill the screen.
    pub base: Rgba,
    fit: Fit,
}

impl Scene {
    /// The scene for the active theme.
    pub fn resolve(ctx: &Context, config: &WallConfig) -> Self {
        let chosen = Chosen::load(&ctx.paths);
        let picture = wallpaper::resolve(config, &chosen, &ctx.theme.id).map(|(path, _)| path);
        Self {
            modified: picture.as_deref().and_then(modified),
            picture,
            base: ctx.theme.colors.base,
            fit: config.fit,
        }
    }
}

fn modified(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// A scene's picture fitted to one screen size, kept while a fade needs it
/// frame after frame. `None` is a picture that could not be read.
#[derive(Debug)]
struct Fitted {
    path: PathBuf,
    size: (u32, u32),
    picture: Option<Picture>,
}

#[derive(Debug)]
pub struct Wall {
    ctx: Context,
    config: WallConfig,
    scene: Scene,
    /// What was showing before, while it fades out.
    previous: Option<Scene>,
    /// How far the current scene has faded in.
    fade: Tween<f32>,
    /// Fitted pictures, held only during a fade.
    fitted: Vec<Fitted>,
    /// Files already reported as unreadable, so each is reported once.
    reported: Vec<PathBuf>,
}

impl Wall {
    pub fn new(ctx: Context, config: WallConfig) -> Self {
        Self {
            scene: Scene::resolve(&ctx, &config),
            ctx,
            config,
            previous: None,
            fade: Tween::new(1.0),
            fitted: Vec::new(),
            reported: Vec::new(),
        }
    }

    /// Look again at the theme and the chosen wallpapers. Returns whether
    /// the scene changed — and if it did, the old one starts fading out.
    pub fn refresh(&mut self, now: Instant) -> bool {
        self.ctx.reload_theme();
        let scene = Scene::resolve(&self.ctx, &self.config);
        if scene == self.scene {
            return false;
        }
        self.previous = Some(std::mem::replace(&mut self.scene, scene));
        self.fitted.clear();
        self.fade.jump(0.0);
        self.fade
            .set(1.0, self.ctx.motion.apply(Transition::SCENE), now);
        true
    }

    /// `scene`'s picture at `size`, from the fade's cache or from disk.
    fn picture(&mut self, scene: &Scene, size: (u32, u32), keep: bool) -> Option<Picture> {
        let path = scene.picture.as_deref()?;
        if let Some(at) = self
            .fitted
            .iter()
            .position(|fitted| fitted.path == path && fitted.size == size)
        {
            return if keep {
                self.fitted[at].picture.clone()
            } else {
                self.fitted.swap_remove(at).picture
            };
        }
        let picture = match Picture::open(path) {
            Ok(picture) => Some(picture.fitted(size.0, size.1, scene.fit, scene.base)),
            Err(err) => {
                if !self.reported.iter().any(|seen| seen == path) {
                    eprintln!("enso-wall: {err}; showing the theme colour instead");
                    self.reported.push(path.to_owned());
                }
                None
            }
        };
        if keep {
            self.fitted.push(Fitted {
                path: path.to_owned(),
                size,
                picture: picture.clone(),
            });
        }
        picture
    }

    fn paint(&mut self, canvas: &mut Canvas<'_>, scene: &Scene, size: (u32, u32), keep: bool) {
        match self.picture(scene, size, keep) {
            Some(picture) => canvas.picture(&picture),
            None => canvas.fill(canvas.bounds(), 0.0, scene.base),
        }
    }
}

impl Screens for Wall {
    type Message = Message;

    fn role(&self) -> Role {
        Role::Wallpaper {
            namespace: "enso-wall".to_owned(),
        }
    }

    fn start(&mut self, proxy: Proxy<Message>, _cx: &mut Cx<'_>) {
        if let Err(err) = watch_dir(&self.ctx.paths.state_dir, move || {
            proxy.send(Message::StateChanged)
        }) {
            eprintln!("enso-wall: cannot follow theme changes: {err}");
        }
    }

    fn event(&mut self, event: Event<Message>, cx: &mut Cx<'_>) {
        if let Event::Message(Message::StateChanged) = event
            && self.refresh(Instant::now())
        {
            cx.redraw();
        }
    }

    fn draw(&mut self, canvas: &mut Canvas<'_>, screen: &Screen, now: Instant) -> Paint {
        let size = screen.pixels();
        let scene = self.scene.clone();
        // Under everything, in case a picture is ever smaller than a screen.
        canvas.clear(scene.base);

        let previous = self
            .previous
            .clone()
            .filter(|_| self.fade.is_animating(now));
        let Some(previous) = previous else {
            // At rest: draw once and keep nothing. The compositor holds the
            // frame; holding the pixels here as well would only be weight.
            self.previous = None;
            self.fitted.clear();
            self.paint(canvas, &scene, size, false);
            return Paint::Idle;
        };
        self.paint(canvas, &previous, size, true);
        canvas.set_opacity(self.fade.get(now));
        self.paint(canvas, &scene, size, true);
        canvas.set_opacity(1.0);
        Paint::Animating
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use enso_core::{Paths, theme};
    use enso_ui::{Offscreen, TextEngine};

    use super::*;

    const RED: [u8; 4] = [200, 40, 40, 255];
    const BLUE: [u8; 4] = [40, 40, 200, 255];

    /// A scratch directory holding a red and a blue picture.
    fn fixture(name: &str) -> (PathBuf, Paths) {
        let root = std::env::temp_dir().join(format!("enso-wall-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        for (file, color) in [("red.png", RED), ("blue.png", BLUE)] {
            Offscreen {
                width: 64,
                height: 32,
                rgba: color.repeat(64 * 32),
            }
            .save_png(root.join(file))
            .unwrap();
        }
        let paths = Paths::rooted(&root);
        (root, paths)
    }

    fn config(root: &Path) -> WallConfig {
        let path = |file: &str| root.join(file).to_string_lossy().into_owned();
        WallConfig {
            default: Some(path("red.png")),
            themes: [("rose-pine-dawn".to_owned(), path("blue.png"))].into(),
            fit: Fit::Cover,
        }
    }

    fn render(wall: &mut Wall, now: Instant) -> (Offscreen, Paint) {
        let screen = Screen {
            name: None,
            width: 160,
            height: 90,
            scale: 1.5,
        };
        let mut text = TextEngine::new(&wall.ctx.config.appearance);
        let mut paint = Paint::Idle;
        let (width, height) = screen.pixels();
        // The canvas is the output's pixels; the scale only matters to text.
        let frame = Offscreen::render(&mut text, (width, height), 1, |canvas| {
            paint = wall.draw(canvas, &screen, now);
        });
        (frame, paint)
    }

    #[test]
    fn shows_the_theme_colour_when_no_picture_is_set() {
        let (root, paths) = fixture("plain");
        let ctx = Context::load_from(paths);
        let base = ctx.theme.colors.base;
        let mut wall = Wall::new(ctx, WallConfig::default());
        let (frame, paint) = render(&mut wall, Instant::now());
        assert_eq!((frame.width, frame.height), (240, 135));
        assert_eq!(frame.pixel(120, 60), [base.r, base.g, base.b, 255]);
        assert_eq!(paint, Paint::Idle);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn follows_the_theme_with_a_fade_and_then_holds_nothing() {
        let (root, paths) = fixture("fade");
        let ctx = Context::load_from(paths.clone());
        let mut wall = Wall::new(ctx, config(&root));
        let start = Instant::now();

        // The default theme has no picture of its own: the default shows.
        let (frame, paint) = render(&mut wall, start);
        assert_eq!(frame.pixel(120, 60), RED);
        assert_eq!(frame.pixel(0, 0), RED);
        assert_eq!(paint, Paint::Idle);
        assert!(wall.fitted.is_empty());

        // Nothing changed: nothing to do.
        assert!(!wall.refresh(start));

        // Switching theme brings that theme's picture in gradually.
        theme::set_active(&paths, "rose-pine-dawn").unwrap();
        assert!(wall.refresh(start));
        let (frame, paint) = render(&mut wall, start + Duration::from_millis(210));
        assert_eq!(paint, Paint::Animating);
        let [r, _, b, a] = frame.pixel(120, 60);
        assert!(r > 60 && r < 180 && b > 60 && b < 180, "mid-fade: {r} {b}");
        assert_eq!(a, 255);
        assert_eq!(wall.fitted.len(), 2);

        let (frame, paint) = render(&mut wall, start + Duration::from_secs(2));
        assert_eq!(frame.pixel(120, 60), BLUE);
        assert_eq!(paint, Paint::Idle);
        assert!(wall.fitted.is_empty() && wall.previous.is_none());

        frame
            .save_png(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../target/snapshots/wall.png"
            ))
            .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_chosen_wallpaper_and_an_unreadable_one() {
        let (root, paths) = fixture("chosen");
        let ctx = Context::load_from(paths.clone());
        let base = ctx.theme.colors.base;
        let mut wall = Wall::new(ctx, config(&root));
        let now = Instant::now();

        // `enso-wall set --theme …` wins over the configured default.
        let mut chosen = Chosen::default();
        chosen.set(Some(theme::DEFAULT_THEME), &root.join("blue.png"));
        chosen.save(&paths).unwrap();
        assert!(wall.refresh(now));
        let (frame, _) = render(&mut wall, now + Duration::from_secs(2));
        assert_eq!(frame.pixel(10, 10), BLUE);

        // A file that is not a picture falls back to the theme colour.
        fs::write(root.join("broken.png"), "not a picture").unwrap();
        chosen.set(Some(theme::DEFAULT_THEME), &root.join("broken.png"));
        chosen.save(&paths).unwrap();
        assert!(wall.refresh(now));
        let (frame, _) = render(&mut wall, now + Duration::from_secs(4));
        assert_eq!(frame.pixel(10, 10), [base.r, base.g, base.b, 255]);
        fs::remove_dir_all(root).unwrap();
    }
}
