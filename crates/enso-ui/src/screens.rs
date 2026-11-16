//! The runtime for interfaces that cover every output: one surface per
//! screen, all drawn by the same [`Screens`] app.
//!
//! Two tools are shaped like this. A wallpaper sits behind everything on
//! each output and takes no input ([`Role::Wallpaper`]). A lock screen
//! replaces everything on each output and takes all of it ([`Role::Lock`],
//! through `ext-session-lock`, the protocol that makes the compositor itself
//! refuse to show or feed anything else until the client says so).
//!
//! Outputs come and go — a lid closes, a monitor is plugged in — and the
//! runtime follows, so an app only ever answers "what goes on this screen".
//! Like [`shell`](crate::shell) it redraws only when asked. Buffers are sized
//! in the output's real pixels, fractional scales included, so a full-screen
//! picture is shown exactly as it was fitted.

use std::time::{Duration, Instant};

use calloop::{
    EventLoop, LoopHandle, RegistrationToken, channel,
    timer::{TimeoutAction, Timer},
};
use enso_core::config::Appearance;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    delegate_dispatch2, delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, Proxy, QueueHandle, WEnum,
            globals::registry_queue_init,
            protocol::{
                wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard},
                wl_output::{self, WlOutput},
                wl_pointer::{self, WlPointer},
                wl_seat::WlSeat,
                wl_shm,
                wl_surface::WlSurface,
            },
        },
        protocols::wp::{
            fractional_scale::v1::client::{
                wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
                wp_fractional_scale_v1::{self, WpFractionalScaleV1},
            },
            viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{Capability, SeatHandler, SeatState},
    session_lock::{
        SessionLock, SessionLockHandler, SessionLockState, SessionLockSurface,
        SessionLockSurfaceConfigure,
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            self, KeyboardInteractivity, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};

use crate::{
    canvas::Canvas,
    input::{Button, PointerEvent},
    keymap::Keys,
    shell::{Cx, Error, Event, Paint, Proxy as AppProxy},
    text::TextEngine,
};

/// What the surfaces are to the compositor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Role {
    /// Behind every window, taking no input. `namespace` identifies the
    /// surfaces to the compositor (for Niri `layer-rule`s).
    Wallpaper { namespace: String },
    /// The session lock: nothing else is shown, and nothing else gets input,
    /// until the app calls [`Cx::exit`]. If the app dies instead, the
    /// compositor keeps the session locked.
    Lock,
}

/// The output a frame is being drawn for.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    /// Connector name (`eDP-1`), when the compositor gives one.
    pub name: Option<String>,
    /// Size in logical pixels.
    pub width: u32,
    pub height: u32,
    /// Physical pixels per logical pixel; may be fractional.
    pub scale: f32,
}

impl Screen {
    /// Size in the output's own pixels: what a [`Picture`](crate::Picture)
    /// should be fitted to.
    pub fn pixels(&self) -> (u32, u32) {
        let scale120 = (self.scale * 120.0).round() as u32;
        (
            physical(self.width, scale120),
            physical(self.height, scale120),
        )
    }
}

/// Logical length to buffer pixels, rounding half away from zero as the
/// fractional-scale protocol prescribes.
fn physical(logical: u32, scale120: u32) -> u32 {
    ((logical * scale120 + 60) / 120).max(1)
}

/// An interface drawn on every output.
///
/// Of [`Cx`], only [`Cx::redraw`] (every screen), [`Cx::wake_in`],
/// [`Cx::measure`], and [`Cx::exit`] mean anything here; a [`Role::Lock`]
/// app unlocks the session by exiting.
pub trait Screens: 'static {
    /// Messages other threads can send in through a [`Proxy`](AppProxy).
    type Message: Send + 'static;

    /// What the surfaces are. Called once at startup.
    fn role(&self) -> Role;

    /// Called once before the first event; start background work here.
    fn start(&mut self, _proxy: AppProxy<Self::Message>, _cx: &mut Cx<'_>) {}

    /// React to messages and wake-ups and — when locking — to keys and to
    /// the pointer. A change of Caps Lock arrives as a key event too
    /// ([`Key::Other`](crate::Key::Other) with the new modifiers).
    fn event(&mut self, event: Event<Self::Message>, cx: &mut Cx<'_>);

    /// Draw the frame for one screen. Cover all of it: what is left
    /// transparent shows whatever the compositor puts behind.
    fn draw(&mut self, canvas: &mut Canvas<'_>, screen: &Screen, now: Instant) -> Paint;
}

/// Run `app` on every output until it exits.
///
/// With [`Role::Lock`] this returns an error without ever showing anything
/// if the session cannot be locked — the compositor lacks the protocol, or
/// something else holds the lock.
pub fn run_screens<A: Screens>(app: A, appearance: &Appearance) -> Result<(), Error> {
    let conn = Connection::connect_to_env()
        .map_err(|err| Error::new("cannot connect to a Wayland compositor", err))?;
    let (globals, mut queue) =
        registry_queue_init(&conn).map_err(|err| Error::new("Wayland registry", err))?;
    let qh = queue.handle();

    let mut event_loop: EventLoop<'static, Runtime<A>> =
        EventLoop::try_new().map_err(|err| Error::new("event loop", err))?;
    let handle = event_loop.handle();

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|err| Error::new("compositor lacks wl_compositor", err))?;
    let shm = Shm::bind(&globals, &qh).map_err(|err| Error::new("compositor lacks wl_shm", err))?;

    let role = app.role();
    let (layer_shell, locker) = match &role {
        Role::Wallpaper { .. } => {
            let shell = LayerShell::bind(&globals, &qh)
                .map_err(|err| Error::new("compositor lacks wlr-layer-shell", err))?;
            (Some(shell), None)
        }
        Role::Lock => (None, Some(SessionLockState::new(&globals, &qh))),
    };
    // Exact pixels on fractionally scaled outputs need both of these;
    // without either, whole-number scales are used.
    let fractional: Option<(WpFractionalScaleManagerV1, WpViewporter)> = globals
        .bind(&qh, 1..=1, Extension)
        .ok()
        .zip(globals.bind(&qh, 1..=1, Extension).ok());

    let (sender, messages) = channel::channel();
    handle
        .insert_source(messages, |event, (), runtime: &mut Runtime<A>| {
            if let channel::Event::Msg(message) = event {
                runtime.deliver(Event::Message(message));
            }
        })
        .map_err(|err| Error::new("event loop", err))?;

    let mut runtime = Runtime {
        app,
        role,
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        shm,
        layer_shell,
        fractional,
        lock: None,
        locked: false,
        unlock: false,
        ready: false,
        surfaces: Vec::new(),
        qh: qh.clone(),
        handle: handle.clone(),
        text: TextEngine::new(appearance),
        exit: false,
        failure: None,
        wake: None,
        keyboard: None,
        keys: Keys::default(),
        repeating: None,
        pointer: None,
    };

    // Learn the outputs before anything is shown, so that locking covers
    // all of them from its first frame.
    for _ in 0..2 {
        queue
            .roundtrip(&mut runtime)
            .map_err(|err| Error::new("Wayland connection", err))?;
    }
    if let Some(locker) = &locker {
        let lock = locker
            .lock(&qh)
            .map_err(|err| Error::new("compositor lacks ext-session-lock", err))?;
        runtime.lock = Some(lock);
    }
    runtime.ready = true;
    for output in runtime.outputs.outputs().collect::<Vec<_>>() {
        runtime.add_surface(output);
    }

    WaylandSource::new(conn.clone(), queue)
        .insert(handle)
        .map_err(|err| Error::new("event loop", err))?;

    let mut cx = Cx::new((1, 1), &mut runtime.text);
    runtime.app.start(AppProxy(sender), &mut cx);
    let requests = cx.requests;
    runtime.apply(requests);

    while !runtime.exit {
        event_loop
            .dispatch(None, &mut runtime)
            .map_err(|err| Error::new("event loop", err))?;
        runtime.flush();
    }

    if let (true, Some(lock)) = (runtime.unlock, runtime.lock.take()) {
        lock.unlock();
        // Leaving before the compositor has heard the unlock would look to
        // it like a lock screen that died, and it would stay locked.
        conn.roundtrip()
            .map_err(|err| Error::new("cannot unlock the session", err))?;
    }
    runtime.failure.map_or(Ok(()), Err)
}

/// The compositor's side of one surface.
enum Kind {
    Layer(LayerSurface),
    Lock(SessionLockSurface),
}

/// One output's surface.
struct Surface {
    output: WlOutput,
    kind: Kind,
    /// Present when buffers are sized by a fractional scale.
    viewport: Option<(WpViewport, WpFractionalScaleV1)>,
    /// Logical size from the last configure.
    size: (u32, u32),
    /// Scale in 120ths, as the fractional-scale protocol counts.
    scale120: u32,
    configured: bool,
    frame_pending: bool,
    dirty: bool,
    /// Where frames are drawn. A wallpaper lets go of it once it is still.
    pool: Option<SlotPool>,
}

impl Surface {
    fn wl_surface(&self) -> &WlSurface {
        match &self.kind {
            Kind::Layer(layer) => layer.wl_surface(),
            Kind::Lock(lock) => lock.wl_surface(),
        }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        if let Some((viewport, scale)) = self.viewport.take() {
            viewport.destroy();
            scale.destroy();
        }
    }
}

struct Runtime<A: Screens> {
    app: A,
    role: Role,
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    shm: Shm,
    layer_shell: Option<LayerShell>,
    fractional: Option<(WpFractionalScaleManagerV1, WpViewporter)>,
    lock: Option<SessionLock>,
    /// The compositor confirmed the lock.
    locked: bool,
    /// The app asked to exit, which for a lock means unlocking.
    unlock: bool,
    /// Surfaces may be created: the outputs are known, and so is the lock.
    ready: bool,
    surfaces: Vec<Surface>,
    qh: QueueHandle<Self>,
    handle: LoopHandle<'static, Self>,
    text: TextEngine,
    exit: bool,
    failure: Option<Error>,
    wake: Option<RegistrationToken>,
    keyboard: Option<WlKeyboard>,
    keys: Keys,
    /// The key currently auto-repeating and its timer.
    repeating: Option<(u32, RegistrationToken)>,
    pointer: Option<WlPointer>,
}

impl<A: Screens> Runtime<A> {
    fn deliver(&mut self, event: Event<A::Message>) {
        let mut cx = Cx::new((1, 1), &mut self.text);
        self.app.event(event, &mut cx);
        let requests = cx.requests;
        self.apply(requests);
    }

    fn apply(&mut self, requests: crate::shell::Requests) {
        if requests.exit {
            self.exit = true;
            self.unlock = true;
        }
        if requests.redraw {
            for surface in &mut self.surfaces {
                surface.dirty = true;
            }
        }
        if let Some(delay) = requests.wake {
            self.schedule_wake(delay);
        }
    }

    fn schedule_wake(&mut self, delay: Duration) {
        if let Some(token) = self.wake.take() {
            self.handle.remove(token);
        }
        self.wake = self
            .handle
            .insert_source(Timer::from_duration(delay), |_, (), runtime: &mut Self| {
                runtime.wake = None;
                runtime.deliver(Event::Wake);
                TimeoutAction::Drop
            })
            .ok();
    }

    fn fail(&mut self, error: Error) {
        self.failure.get_or_insert(error);
        self.exit = true;
    }

    /// Give `output` its surface, unless it has one.
    fn add_surface(&mut self, output: WlOutput) {
        if !self.ready || self.surfaces.iter().any(|s| s.output == output) {
            return;
        }
        let wl_surface = self.compositor.create_surface(&self.qh);
        let viewport = self.fractional.as_ref().map(|(scales, viewporter)| {
            (
                viewporter.get_viewport(&wl_surface, &self.qh, Extension),
                scales.get_fractional_scale(&wl_surface, &self.qh, ScaleOf(wl_surface.clone())),
            )
        });
        let kind = match (&self.role, &self.layer_shell, &self.lock) {
            (Role::Wallpaper { namespace }, Some(shell), _) => {
                let layer = shell.create_layer_surface(
                    &self.qh,
                    wl_surface,
                    wlr_layer::Layer::Background,
                    Some(namespace.clone()),
                    Some(&output),
                );
                layer.set_anchor(wlr_layer::Anchor::all());
                layer.set_size(0, 0);
                // Under bars and panels too, not just the space they leave.
                layer.set_exclusive_zone(-1);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                // A wallpaper is looked at, never clicked: an empty region.
                if let Ok(region) = Region::new(&self.compositor) {
                    layer
                        .wl_surface()
                        .set_input_region(Some(region.wl_region()));
                }
                layer.commit();
                Kind::Layer(layer)
            }
            (Role::Lock, _, Some(lock)) => {
                Kind::Lock(lock.create_lock_surface(wl_surface, &output, &self.qh))
            }
            _ => return,
        };
        let scale = self
            .outputs
            .info(&output)
            .map_or(1, |info| info.scale_factor.max(1));
        self.surfaces.push(Surface {
            output,
            kind,
            viewport,
            size: (1, 1),
            scale120: scale as u32 * 120,
            configured: false,
            frame_pending: false,
            dirty: false,
            pool: None,
        });
    }

    fn surface_mut(&mut self, wl_surface: &WlSurface) -> Option<&mut Surface> {
        self.surfaces
            .iter_mut()
            .find(|surface| surface.wl_surface() == wl_surface)
    }

    fn configured(&mut self, wl_surface: &WlSurface, size: (u32, u32)) {
        if let Some(surface) = self.surface_mut(wl_surface) {
            surface.size = (size.0.max(1), size.1.max(1));
            surface.configured = true;
            surface.dirty = true;
        }
    }

    /// Draw every screen that changed. Called after every batch of events.
    fn flush(&mut self) {
        for index in 0..self.surfaces.len() {
            let surface = &self.surfaces[index];
            if surface.dirty
                && surface.configured
                && !surface.frame_pending
                && let Err(err) = self.draw(index)
            {
                return self.fail(err);
            }
        }
    }

    fn draw(&mut self, index: usize) -> Result<(), Error> {
        let surface = &mut self.surfaces[index];
        surface.dirty = false;
        // Without a viewport the compositor only takes whole-number scales.
        let scale120 = if surface.viewport.is_some() {
            surface.scale120
        } else {
            surface.scale120.div_ceil(120) * 120
        };
        let (width, height) = (
            physical(surface.size.0, scale120) as i32,
            physical(surface.size.1, scale120) as i32,
        );
        let screen = Screen {
            name: self
                .outputs
                .info(&surface.output)
                .and_then(|info| info.name),
            width: surface.size.0,
            height: surface.size.1,
            scale: scale120 as f32 / 120.0,
        };

        let mut pool = match surface.pool.take() {
            Some(pool) => pool,
            None => SlotPool::new(width as usize * height as usize * 4, &self.shm)
                .map_err(|err| Error::new("shared memory", err))?,
        };
        let (buffer, pixels) = pool
            .create_buffer(width, height, width * 4, wl_shm::Format::Argb8888)
            .map_err(|err| Error::new("cannot allocate a frame buffer", err))?;
        pixels.fill(0);

        let mut canvas = Canvas::new(
            pixels,
            width as u32,
            height as u32,
            screen.scale,
            true,
            &mut self.text,
        );
        let paint = self.app.draw(&mut canvas, &screen, Instant::now());

        let wl_surface = surface.wl_surface();
        match &surface.viewport {
            Some((viewport, _)) => {
                wl_surface.set_buffer_scale(1);
                viewport.set_destination(surface.size.0 as i32, surface.size.1 as i32);
            }
            None => wl_surface.set_buffer_scale((scale120 / 120) as i32),
        }
        wl_surface.damage_buffer(0, 0, width, height);
        if paint == Paint::Animating {
            wl_surface.frame(&self.qh, FrameCallbackData(wl_surface.clone()));
        }
        buffer
            .attach_to(wl_surface)
            .map_err(|err| Error::new("cannot attach a frame buffer", err))?;
        wl_surface.commit();
        surface.frame_pending = paint == Paint::Animating;

        // A wallpaper at rest is drawn once and then only looked at. The
        // compositor has its copy; ours — tens of megabytes on a large
        // screen — goes back to the system until something changes.
        let keep = paint == Paint::Animating || self.role == Role::Lock;
        surface.pool = keep.then_some(pool);
        Ok(())
    }

    fn press(&mut self, keycode: u32, repeat: bool) {
        if let Some(event) = self.keys.translate(keycode, repeat) {
            self.deliver(Event::Key(event));
        }
    }

    fn start_repeat(&mut self, keycode: u32) {
        self.stop_repeat();
        let Some((delay, interval)) = self.keys.repeat_timing(keycode) else {
            return;
        };
        let token = self.handle.insert_source(
            Timer::from_duration(delay),
            move |_, (), runtime: &mut Self| {
                runtime.press(keycode, true);
                TimeoutAction::ToDuration(interval)
            },
        );
        self.repeating = token.ok().map(|token| (keycode, token));
    }

    fn stop_repeat(&mut self) {
        if let Some((_, token)) = self.repeating.take() {
            self.handle.remove(token);
        }
    }
}

/// User data for globals and objects that never send events.
struct Extension;

impl<A: Screens> Dispatch2<WpFractionalScaleManagerV1, Runtime<A>> for Extension {
    fn event(
        &self,
        _: &mut Runtime<A>,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<Runtime<A>>,
    ) {
    }
}

impl<A: Screens> Dispatch2<WpViewporter, Runtime<A>> for Extension {
    fn event(
        &self,
        _: &mut Runtime<A>,
        _: &WpViewporter,
        _: <WpViewporter as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<Runtime<A>>,
    ) {
    }
}

impl<A: Screens> Dispatch2<WpViewport, Runtime<A>> for Extension {
    fn event(
        &self,
        _: &mut Runtime<A>,
        _: &WpViewport,
        _: <WpViewport as Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<Runtime<A>>,
    ) {
    }
}

/// User data for a surface's fractional scale: the surface it speaks for.
struct ScaleOf(WlSurface);

impl<A: Screens> Dispatch2<WpFractionalScaleV1, Runtime<A>> for ScaleOf {
    fn event(
        &self,
        runtime: &mut Runtime<A>,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<Runtime<A>>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && let Some(surface) = runtime.surface_mut(&self.0)
            && surface.scale120 != scale.max(1)
        {
            surface.scale120 = scale.max(1);
            surface.dirty = true;
        }
    }
}

/// User data for our own `wl_keyboard`, handled with kbvm instead of libxkbcommon.
struct KeyboardData;

impl<A: Screens> Dispatch2<WlKeyboard, Runtime<A>> for KeyboardData {
    fn event(
        &self,
        runtime: &mut Runtime<A>,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &Connection,
        _: &QueueHandle<Runtime<A>>,
    ) {
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(KeymapFormat::XkbV1),
                fd,
                size,
            } => runtime.keys.load(fd, size),
            wl_keyboard::Event::Leave { .. } => runtime.stop_repeat(),
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                let caps_changed =
                    runtime
                        .keys
                        .set_modifiers(mods_depressed, mods_latched, mods_locked, group);
                if caps_changed {
                    // The key press that toggled it came before this, with
                    // the old state; say so again with the new one.
                    runtime.deliver(Event::Key(crate::KeyEvent {
                        key: crate::Key::Other,
                        mods: runtime.keys.mods(),
                        repeat: false,
                    }));
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(state),
                ..
            } => match state {
                KeyState::Pressed => {
                    runtime.start_repeat(key);
                    runtime.press(key, false);
                }
                KeyState::Released
                    if runtime
                        .repeating
                        .as_ref()
                        .is_some_and(|(code, _)| *code == key) =>
                {
                    runtime.stop_repeat();
                }
                _ => {}
            },
            wl_keyboard::Event::RepeatInfo { rate, delay } => runtime.keys.set_repeat(rate, delay),
            _ => {}
        }
    }
}

/// User data for our own `wl_pointer`: a lock screen shows no cursor, and
/// only wants to know that the pointer moved or clicked.
struct PointerData;

impl<A: Screens> Dispatch2<WlPointer, Runtime<A>> for PointerData {
    fn event(
        &self,
        runtime: &mut Runtime<A>,
        pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &Connection,
        _: &QueueHandle<Runtime<A>>,
    ) {
        match event {
            wl_pointer::Event::Enter { serial, .. } => pointer.set_cursor(serial, None, 0, 0),
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => runtime.deliver(Event::Pointer(PointerEvent::Move {
                x: surface_x as f32,
                y: surface_y as f32,
            })),
            wl_pointer::Event::Button {
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => runtime.deliver(Event::Pointer(PointerEvent::Press {
                x: 0.0,
                y: 0.0,
                button: Button::Left,
            })),
            _ => {}
        }
    }
}

impl<A: Screens> CompositorHandler for Runtime<A> {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        wl_surface: &WlSurface,
        factor: i32,
    ) {
        // The fractional scale, where there is one, is the better answer.
        if let Some(surface) = self.surface_mut(wl_surface)
            && surface.viewport.is_none()
            && surface.scale120 != factor.max(1) as u32 * 120
        {
            surface.scale120 = factor.max(1) as u32 * 120;
            surface.dirty = true;
        }
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, wl_surface: &WlSurface, _: u32) {
        if let Some(surface) = self.surface_mut(wl_surface) {
            surface.frame_pending = false;
            surface.dirty = true;
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlSurface,
        _: &WlOutput,
    ) {
    }
}

impl<A: Screens> LayerShellHandler for Runtime<A> {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        // The output went away; its surface goes with it.
        self.surfaces
            .retain(|surface| surface.wl_surface() != layer.wl_surface());
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.configured(layer.wl_surface(), configure.new_size);
    }
}

impl<A: Screens> SessionLockHandler for Runtime<A> {
    fn locked(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        self.locked = true;
    }

    fn finished(&mut self, _: &Connection, _: &QueueHandle<Self>, _: SessionLock) {
        if self.locked {
            // The compositor ended the lock itself; there is nothing left
            // to unlock.
            self.lock = None;
            self.exit = true;
        } else {
            self.fail(Error::new(
                "cannot lock the session",
                "the compositor refused — is another lock screen running?",
            ));
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: SessionLockSurface,
        configure: SessionLockSurfaceConfigure,
        _serial: u32,
    ) {
        self.configured(surface.wl_surface(), configure.new_size);
    }
}

impl<A: Screens> SeatHandler for Runtime<A> {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seats
    }

    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}

    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        // A wallpaper takes no input at all.
        if self.role != Role::Lock {
            return;
        }
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                self.keyboard = Some(seat.get_keyboard(qh, KeyboardData));
            }
            Capability::Pointer if self.pointer.is_none() => {
                self.pointer = Some(seat.get_pointer(qh, PointerData));
            }
            _ => {}
        }
    }

    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: WlSeat,
        capability: Capability,
    ) {
        match capability {
            Capability::Keyboard => {
                self.stop_repeat();
                if let Some(keyboard) = self.keyboard.take() {
                    keyboard.release();
                }
            }
            Capability::Pointer => {
                if let Some(pointer) = self.pointer.take() {
                    pointer.release();
                }
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl<A: Screens> OutputHandler for Runtime<A> {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        self.add_surface(output);
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: WlOutput) {
        self.surfaces.retain(|surface| surface.output != output);
    }
}

impl<A: Screens> ShmHandler for Runtime<A> {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl<A: Screens> ProvidesRegistryState for Runtime<A> {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(@<A: Screens> Runtime<A>);
delegate_dispatch2!(@<A: Screens> Runtime<A>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_are_sized_in_real_pixels() {
        let screen = |width, height, scale| Screen {
            name: None,
            width,
            height,
            scale,
        };
        assert_eq!(screen(1920, 1080, 1.0).pixels(), (1920, 1080));
        assert_eq!(screen(1280, 720, 2.0).pixels(), (2560, 1440));
        // 2560×1600 at 1.25 is 2048×1280 logical, and comes back exactly.
        assert_eq!(screen(2048, 1280, 1.25).pixels(), (2560, 1600));
        // 1.5 on an odd logical size rounds half up.
        assert_eq!(physical(1281, 180), 1922);
        assert_eq!(physical(0, 120), 1);
    }
}
