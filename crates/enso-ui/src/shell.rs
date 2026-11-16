//! The layer-shell runtime: one Wayland surface, an event loop, and the
//! [`App`] trait each tool implements.
//!
//! The runtime owns everything Wayland-specific — surface configuration,
//! shared-memory buffers, HiDPI scale, keyboard layouts and key repeat,
//! pointer and cursor, frame pacing — and hands the app plain events and a
//! [`Canvas`] to draw on. It only redraws when asked, and paces animations
//! with frame callbacks, so an idle surface costs nothing.

use std::{
    fmt,
    time::{Duration, Instant},
};

use calloop::{
    EventLoop, LoopHandle, RegistrationToken,
    channel::{self, Sender},
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
            Connection, Proxy as _, QueueHandle, WEnum,
            globals::registry_queue_init,
            protocol::{
                wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard},
                wl_output::{self, WlOutput},
                wl_pointer::WlPointer,
                wl_seat::WlSeat,
                wl_shm,
                wl_surface::WlSurface,
            },
        },
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{
            CursorIcon, PointerEvent as SctkPointerEvent, PointerEventKind, PointerHandler,
            ThemeSpec, ThemedPointer,
        },
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            self, KeyboardInteractivity, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure, SurfaceKind,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};

use crate::{
    canvas::Canvas,
    geom::{Rect, Size},
    input::{Button, KeyEvent, PointerEvent},
    keymap::Keys,
    text::{Text, TextEngine},
};

/// Which stacking layer the surface lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    Background,
    Bottom,
    /// Above windows. Bars and docked panels.
    Top,
    /// Above everything, including fullscreen windows. Launchers and toasts.
    Overlay,
}

impl From<Layer> for wlr_layer::Layer {
    fn from(layer: Layer) -> Self {
        match layer {
            Layer::Background => Self::Background,
            Layer::Bottom => Self::Bottom,
            Layer::Top => Self::Top,
            Layer::Overlay => Self::Overlay,
        }
    }
}

/// Screen edges a surface is attached to. Combine with `|`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor(u32);

impl Anchor {
    pub const NONE: Self = Self(0);
    pub const TOP: Self = Self(1);
    pub const BOTTOM: Self = Self(2);
    pub const LEFT: Self = Self(4);
    pub const RIGHT: Self = Self(8);
    /// All four edges: the surface covers the whole output.
    pub const FILL: Self = Self(15);
}

impl std::ops::BitOr for Anchor {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// How the surface receives keyboard focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyboard {
    /// Never focused (bars, toasts).
    None,
    /// Focused when clicked.
    OnDemand,
    /// Grabs the keyboard as soon as it appears (launchers, modal panels).
    Exclusive,
}

impl From<Keyboard> for KeyboardInteractivity {
    fn from(keyboard: Keyboard) -> Self {
        match keyboard {
            Keyboard::None => Self::None,
            Keyboard::OnDemand => Self::OnDemand,
            Keyboard::Exclusive => Self::Exclusive,
        }
    }
}

/// Gap between the surface and the edges it is anchored to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Margin {
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
    pub left: i32,
}

impl Margin {
    pub const fn all(value: i32) -> Self {
        Self {
            top: value,
            right: value,
            bottom: value,
            left: value,
        }
    }
}

/// Where and how a tool's surface sits on screen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfaceSpec {
    /// Identifies the surface to the compositor (for Niri `layer-rule`s).
    pub namespace: String,
    pub layer: Layer,
    pub anchor: Anchor,
    /// Width in logical pixels; `0` stretches between the left and right anchors.
    pub width: u32,
    /// Height in logical pixels; `0` stretches between the top and bottom anchors.
    pub height: u32,
    /// Space reserved along the anchored edge so windows do not overlap it.
    pub exclusive_zone: i32,
    pub margin: Margin,
    pub keyboard: Keyboard,
    /// Whether to show the surface at startup. Hidden surfaces cost nothing.
    pub visible: bool,
}

impl SurfaceSpec {
    /// A visible, non-interactive, zero-size surface to customise.
    pub fn new(namespace: &str, layer: Layer, anchor: Anchor) -> Self {
        Self {
            namespace: namespace.to_owned(),
            layer,
            anchor,
            width: 0,
            height: 0,
            exclusive_zone: 0,
            margin: Margin::default(),
            keyboard: Keyboard::None,
            visible: true,
        }
    }

    /// A full-screen overlay that grabs the keyboard: the base for floating
    /// cards that dismiss on `Esc` or a click outside.
    pub fn overlay(namespace: &str) -> Self {
        Self {
            keyboard: Keyboard::Exclusive,
            ..Self::new(namespace, Layer::Overlay, Anchor::FILL)
        }
    }
}

/// Something that happened, delivered to [`App::event`].
#[derive(Debug)]
pub enum Event<M> {
    /// Sent from another thread through a [`Proxy`].
    Message(M),
    /// A wake-up requested with [`Cx::wake_in`] is due.
    Wake,
    Key(KeyEvent),
    Pointer(PointerEvent),
    /// Keyboard focus was gained or lost.
    Focus(bool),
    /// The surface was (re)sized by the compositor, in logical pixels.
    Resized {
        width: u32,
        height: u32,
    },
    /// The surface is now shown on the output with this connector name.
    Output(String),
}

/// Whether a frame finished an animation or needs a follow-up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paint {
    /// Nothing is moving; redraw only when something changes.
    Idle,
    /// Still animating; draw again on the next frame.
    Animating,
}

impl Paint {
    /// `Animating` if `animating`, else `Idle`.
    pub fn when(animating: bool) -> Self {
        if animating {
            Self::Animating
        } else {
            Self::Idle
        }
    }
}

/// What an app asked for while handling an event.
#[derive(Debug, Default)]
pub(crate) struct Requests {
    pub(crate) redraw: bool,
    pub(crate) exit: bool,
    pub(crate) wake: Option<Duration>,
    resize: Option<(u32, u32)>,
    visible: Option<bool>,
    input_region: Option<Option<Rect>>,
    keyboard: Option<Keyboard>,
}

/// Handed to an app while it handles an event: lets it measure text and ask
/// the runtime to redraw, resize, wake it later, or exit.
#[derive(Debug)]
pub struct Cx<'a> {
    size: (u32, u32),
    text: &'a mut TextEngine,
    pub(crate) requests: Requests,
}

impl<'a> Cx<'a> {
    pub(crate) fn new(size: (u32, u32), text: &'a mut TextEngine) -> Self {
        Self {
            size,
            text,
            requests: Requests::default(),
        }
    }

    /// Current surface size in logical pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Size `text` would occupy, for layout decisions made outside `draw`.
    pub fn measure(&mut self, text: &Text<'_>) -> Size {
        self.text.measure(text)
    }

    /// Draw a new frame.
    pub fn redraw(&mut self) {
        self.requests.redraw = true;
    }

    /// Close the surface and return from [`run`].
    pub fn exit(&mut self) {
        self.requests.exit = true;
    }

    /// Deliver [`Event::Wake`] after `delay`, replacing any pending wake-up.
    pub fn wake_in(&mut self, delay: Duration) {
        self.requests.wake = Some(delay);
    }

    /// Ask for a new surface size (`0` keeps stretching along that axis).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.requests.resize = Some((width, height));
    }

    /// Show or hide the surface. A hidden surface is unmapped entirely.
    pub fn set_visible(&mut self, visible: bool) {
        self.requests.visible = Some(visible);
    }

    /// Limit pointer input to `region` (in logical pixels); elsewhere the
    /// surface is click-through and the pointer counts as having left it.
    /// `None` makes the whole surface take input again, which is the default.
    ///
    /// For surfaces that keep room around what they draw — for a shadow, or
    /// for a card that changes size — without that room getting in the way.
    pub fn set_input_region(&mut self, region: Option<Rect>) {
        self.requests.input_region = Some(region);
    }

    /// Change how the surface receives keyboard focus, e.g. to take the
    /// keyboard only while a text field is being typed into.
    pub fn set_keyboard(&mut self, keyboard: Keyboard) {
        self.requests.keyboard = Some(keyboard);
    }
}

/// Sends messages into the app from other threads (D-Bus, IPC, file watchers).
pub struct Proxy<M>(pub(crate) Sender<M>);

impl<M> Proxy<M> {
    /// Queue a message. Returns `false` if the app has exited.
    pub fn send(&self, message: M) -> bool {
        self.0.send(message).is_ok()
    }
}

impl<M> Clone for Proxy<M> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<M> fmt::Debug for Proxy<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Proxy")
    }
}

/// A tool's interface: one layer-shell surface driven by events.
pub trait App: 'static {
    /// Messages other threads can send in through a [`Proxy`].
    type Message: Send + 'static;

    /// Where the surface goes. Called once at startup.
    fn surface(&self) -> SurfaceSpec;

    /// Called once before the first event; start background work here.
    fn start(&mut self, _proxy: Proxy<Self::Message>, _cx: &mut Cx<'_>) {}

    /// React to input, messages, and wake-ups.
    fn event(&mut self, event: Event<Self::Message>, cx: &mut Cx<'_>);

    /// Draw a frame onto a transparent canvas. `now` is the frame's timestamp;
    /// sample every animation with it so they stay in step.
    fn draw(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint;
}

/// Why the interface could not start or had to stop.
#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub(crate) fn new(context: &str, cause: impl fmt::Display) -> Self {
        Self(format!("{context}: {cause}"))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// Run `app` on its own layer-shell surface until it exits or the compositor
/// closes the surface.
pub fn run<A: App>(app: A, appearance: &Appearance) -> Result<(), Error> {
    let conn = Connection::connect_to_env()
        .map_err(|err| Error::new("cannot connect to a Wayland compositor", err))?;
    let (globals, queue) =
        registry_queue_init(&conn).map_err(|err| Error::new("Wayland registry", err))?;
    let qh = queue.handle();

    let mut event_loop: EventLoop<'static, Runtime<A>> =
        EventLoop::try_new().map_err(|err| Error::new("event loop", err))?;
    let handle = event_loop.handle();
    WaylandSource::new(conn.clone(), queue)
        .insert(handle.clone())
        .map_err(|err| Error::new("event loop", err))?;

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|err| Error::new("compositor lacks wl_compositor", err))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|err| Error::new("compositor lacks wlr-layer-shell", err))?;
    let shm = Shm::bind(&globals, &qh).map_err(|err| Error::new("compositor lacks wl_shm", err))?;
    let pool =
        SlotPool::new(256 * 256 * 4, &shm).map_err(|err| Error::new("shared memory", err))?;

    let spec = app.surface();
    let layer = layer_shell.create_layer_surface(
        &qh,
        compositor.create_surface(&qh),
        spec.layer.into(),
        Some(spec.namespace.clone()),
        None,
    );

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
        registry: RegistryState::new(&globals),
        seats: SeatState::new(&globals, &qh),
        outputs: OutputState::new(&globals, &qh),
        compositor,
        shm,
        pool,
        layer,
        qh,
        handle,
        text: TextEngine::new(appearance),
        size: (spec.width.max(1), spec.height.max(1)),
        scale: 1,
        visible: false,
        input_region: None,
        configured: false,
        frame_pending: false,
        dirty: false,
        exit: false,
        failure: None,
        wake: None,
        keyboard: None,
        keys: Keys::default(),
        repeating: None,
        pointer: None,
        spec,
    };

    let mut cx = Cx::new(runtime.size, &mut runtime.text);
    cx.requests.visible = Some(runtime.spec.visible);
    runtime.app.start(Proxy(sender), &mut cx);
    let requests = cx.requests;
    runtime.apply(requests);

    while !runtime.exit {
        event_loop
            .dispatch(None, &mut runtime)
            .map_err(|err| Error::new("event loop", err))?;
        runtime.flush();
    }
    runtime.failure.map_or(Ok(()), Err)
}

struct Runtime<A: App> {
    app: A,
    registry: RegistryState,
    seats: SeatState,
    outputs: OutputState,
    compositor: CompositorState,
    shm: Shm,
    pool: SlotPool,
    layer: LayerSurface,
    qh: QueueHandle<Self>,
    handle: LoopHandle<'static, Self>,
    text: TextEngine,
    spec: SurfaceSpec,
    /// Logical size from the last configure.
    size: (u32, u32),
    scale: i32,
    visible: bool,
    /// Where the surface takes pointer input; `None` is all of it.
    input_region: Option<Rect>,
    configured: bool,
    frame_pending: bool,
    dirty: bool,
    exit: bool,
    failure: Option<Error>,
    wake: Option<RegistrationToken>,
    keyboard: Option<WlKeyboard>,
    keys: Keys,
    /// The key currently auto-repeating and its timer.
    repeating: Option<(u32, RegistrationToken)>,
    pointer: Option<ThemedPointer>,
}

impl<A: App> Runtime<A> {
    fn deliver(&mut self, event: Event<A::Message>) {
        let mut cx = Cx::new(self.size, &mut self.text);
        self.app.event(event, &mut cx);
        let requests = cx.requests;
        self.apply(requests);
    }

    fn apply(&mut self, requests: Requests) {
        self.exit |= requests.exit;
        self.dirty |= requests.redraw;
        if let Some(delay) = requests.wake {
            self.schedule_wake(delay);
        }
        if let Some((width, height)) = requests.resize
            && (width, height) != (self.spec.width, self.spec.height)
        {
            self.spec.width = width;
            self.spec.height = height;
            if self.visible {
                self.layer.set_size(width, height);
                self.layer.commit();
            }
        }
        if let Some(region) = requests.input_region
            && region != self.input_region
        {
            self.input_region = region;
            if self.visible {
                self.send_input_region();
                // Surface state only takes effect with a commit.
                self.layer.commit();
            }
        }
        if let Some(keyboard) = requests.keyboard
            && keyboard != self.spec.keyboard
        {
            self.spec.keyboard = keyboard;
            if self.visible {
                self.layer.set_keyboard_interactivity(keyboard.into());
                self.layer.commit();
            }
        }
        if let Some(visible) = requests.visible
            && visible != self.visible
        {
            self.set_visible(visible);
        }
    }

    /// Queue the current input region on the surface (applied on commit).
    fn send_input_region(&self) {
        let surface = self.layer.wl_surface();
        let Some(rect) = self.input_region else {
            return surface.set_input_region(None);
        };
        // Without a region object the input region stays as it was: a wider
        // target than asked for, never a dead one.
        if let Ok(region) = Region::new(&self.compositor) {
            region.add(
                rect.x.floor() as i32,
                rect.y.floor() as i32,
                rect.w.ceil() as i32,
                rect.h.ceil() as i32,
            );
            surface.set_input_region(Some(region.wl_region()));
        }
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        self.configured = false;
        self.frame_pending = false;
        if visible {
            // (Re)mapping starts with a bufferless commit carrying the full
            // layer state; the compositor answers with a configure.
            let spec = &self.spec;
            // Unmapping resets a layer surface, and Niri then forgets the
            // layer it was created on and falls back to the background one —
            // a re-shown toast would sit behind every window. So the layer is
            // stated again on each map (possible since layer-shell version 2).
            if let SurfaceKind::Wlr(surface) = self.layer.kind()
                && surface.version() >= 2
            {
                self.layer.set_layer(spec.layer.into());
            }
            self.layer
                .set_anchor(wlr_layer::Anchor::from_bits_truncate(spec.anchor.0));
            self.layer.set_size(spec.width, spec.height);
            self.layer.set_exclusive_zone(spec.exclusive_zone);
            let Margin {
                top,
                right,
                bottom,
                left,
            } = spec.margin;
            self.layer.set_margin(top, right, bottom, left);
            self.layer.set_keyboard_interactivity(spec.keyboard.into());
            self.send_input_region();
            self.layer.commit();
        } else {
            self.stop_repeat();
            self.layer.wl_surface().attach(None, 0, 0);
            self.layer.commit();
        }
    }

    fn schedule_wake(&mut self, delay: Duration) {
        if let Some(token) = self.wake.take() {
            self.handle.remove(token);
        }
        let timer = Timer::from_duration(delay);
        self.wake = self
            .handle
            .insert_source(timer, |_, (), runtime: &mut Self| {
                runtime.wake = None;
                runtime.deliver(Event::Wake);
                TimeoutAction::Drop
            })
            .ok();
    }

    /// Draw if something changed. Called after every batch of events.
    fn flush(&mut self) {
        if self.dirty && self.visible && self.configured && !self.frame_pending {
            self.draw();
        }
    }

    fn draw(&mut self) {
        self.dirty = false;
        let scale = self.scale.max(1);
        let width = self.size.0 as i32 * scale;
        let height = self.size.1 as i32 * scale;
        let (buffer, pixels) =
            match self
                .pool
                .create_buffer(width, height, width * 4, wl_shm::Format::Argb8888)
            {
                Ok(buffer) => buffer,
                Err(err) => return self.fail(Error::new("cannot allocate a frame buffer", err)),
            };
        pixels.fill(0);

        let mut canvas = Canvas::new(
            pixels,
            width as u32,
            height as u32,
            scale as f32,
            true,
            &mut self.text,
        );
        let paint = self.app.draw(&mut canvas, Instant::now());

        let surface = self.layer.wl_surface();
        surface.set_buffer_scale(scale);
        surface.damage_buffer(0, 0, width, height);
        if paint == Paint::Animating {
            surface.frame(&self.qh, FrameCallbackData(surface.clone()));
            self.frame_pending = true;
        }
        if let Err(err) = buffer.attach_to(surface) {
            return self.fail(Error::new("cannot attach a frame buffer", err));
        }
        self.layer.commit();
    }

    fn fail(&mut self, error: Error) {
        self.failure.get_or_insert(error);
        self.exit = true;
    }

    fn is_own(&self, surface: &WlSurface) -> bool {
        self.layer.wl_surface() == surface
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

/// User data for our own `wl_keyboard`, handled with kbvm instead of libxkbcommon.
struct KeyboardData;

impl<A: App> Dispatch2<WlKeyboard, Runtime<A>> for KeyboardData {
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
            } => {
                runtime.keys.load(fd, size);
            }
            wl_keyboard::Event::Enter { surface, .. } if runtime.is_own(&surface) => {
                runtime.deliver(Event::Focus(true));
            }
            wl_keyboard::Event::Leave { surface, .. } if runtime.is_own(&surface) => {
                runtime.stop_repeat();
                runtime.deliver(Event::Focus(false));
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                runtime
                    .keys
                    .set_modifiers(mods_depressed, mods_latched, mods_locked, group);
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
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                runtime.keys.set_repeat(rate, delay);
            }
            _ => {}
        }
    }
}

impl<A: App> CompositorHandler for Runtime<A> {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &WlSurface,
        factor: i32,
    ) {
        if self.is_own(surface) && factor != self.scale {
            self.scale = factor;
            self.dirty = true;
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

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &WlSurface, _: u32) {
        if self.is_own(surface) {
            self.frame_pending = false;
            self.dirty = true;
        }
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &WlSurface,
        output: &WlOutput,
    ) {
        if self.is_own(surface)
            && let Some(name) = self.outputs.info(output).and_then(|info| info.name)
        {
            self.deliver(Event::Output(name));
        }
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

impl<A: App> LayerShellHandler for Runtime<A> {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        if !self.visible {
            return;
        }
        // A zero in the configure means "you choose": use the requested size.
        let pick = |given: u32, wanted: u32| if given == 0 { wanted.max(1) } else { given };
        let size = (
            pick(configure.new_size.0, self.spec.width),
            pick(configure.new_size.1, self.spec.height),
        );
        let changed = size != self.size || !self.configured;
        self.size = size;
        self.configured = true;
        self.dirty = true;
        if changed {
            self.deliver(Event::Resized {
                width: size.0,
                height: size.1,
            });
        }
    }
}

impl<A: App> SeatHandler for Runtime<A> {
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
        match capability {
            Capability::Keyboard if self.keyboard.is_none() => {
                self.keyboard = Some(seat.get_keyboard(qh, KeyboardData));
            }
            Capability::Pointer if self.pointer.is_none() => {
                let cursor_surface = self.compositor.create_surface(qh);
                self.pointer = self
                    .seats
                    .get_pointer_with_theme::<Self, ()>(
                        qh,
                        &seat,
                        self.shm.wl_shm(),
                        cursor_surface,
                        ThemeSpec::default(),
                    )
                    .ok();
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
                    pointer.pointer().release();
                }
            }
            _ => {}
        }
    }

    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlSeat) {}
}

impl<A: App> PointerHandler for Runtime<A> {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _: &QueueHandle<Self>,
        _: &WlPointer,
        events: &[SctkPointerEvent],
    ) {
        for event in events {
            if !self.is_own(&event.surface) {
                continue;
            }
            let (x, y) = (event.position.0 as f32, event.position.1 as f32);
            let button = |code: u32| match code {
                0x110 => Button::Left,
                0x111 => Button::Right,
                0x112 => Button::Middle,
                other => Button::Other(other),
            };
            let event = match &event.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(pointer) = &self.pointer {
                        // Cosmetic: a missing cursor theme must not break input.
                        let _ = pointer.set_cursor(conn, CursorIcon::Default);
                    }
                    PointerEvent::Enter { x, y }
                }
                PointerEventKind::Leave { .. } => PointerEvent::Leave,
                PointerEventKind::Motion { .. } => PointerEvent::Move { x, y },
                PointerEventKind::Press { button: code, .. } => PointerEvent::Press {
                    x,
                    y,
                    button: button(*code),
                },
                PointerEventKind::Release { button: code, .. } => PointerEvent::Release {
                    x,
                    y,
                    button: button(*code),
                },
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => PointerEvent::Scroll {
                    x,
                    y,
                    dx: horizontal.absolute as f32,
                    dy: vertical.absolute as f32,
                },
            };
            self.deliver(Event::Pointer(event));
        }
    }
}

impl<A: App> OutputHandler for Runtime<A> {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: WlOutput) {}
}

impl<A: App> ShmHandler for Runtime<A> {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl<A: App> ProvidesRegistryState for Runtime<A> {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers![OutputState, SeatState];
}

delegate_registry!(@<A: App> Runtime<A>);
delegate_dispatch2!(@<A: App> Runtime<A>);
