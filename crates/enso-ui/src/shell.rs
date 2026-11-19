//! The runtime: one Wayland surface, an event loop, and the [`App`] trait
//! each tool implements.
//!
//! The surface is a layer-shell one — a bar, a toast, an overlay — unless
//! the app asks for a window ([`SurfaceSpec::window`]): an ordinary
//! `xdg-shell` toplevel that the compositor tiles, moves, and resizes like
//! any other, and that can belong to another program's window (a file
//! chooser to the browser that asked for it).
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
use smithay_client_toolkit::reexports::protocols::{
    wp::{
        fractional_scale::v1::client::{
            wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
            wp_fractional_scale_v1::WpFractionalScaleV1,
        },
        text_input::zv3::client::{
            zwp_text_input_manager_v3::ZwpTextInputManagerV3, zwp_text_input_v3::ZwpTextInputV3,
        },
        viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
    },
    xdg::foreign::zv2::client::{
        zxdg_imported_v2::{self, ZxdgImportedV2},
        zxdg_importer_v2::ZxdgImporterV2,
    },
};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, FrameCallbackData, Region},
    data_device_manager::{
        DataDeviceManagerState, WritePipe,
        data_device::{DataDevice, DataDeviceHandler},
        data_offer::{DataOfferHandler, DragOffer},
        data_source::{CopyPasteSource, DataSourceHandler, DragSource},
    },
    delegate_dispatch2, delegate_registry,
    dispatch2::Dispatch2,
    output::{OutputHandler, OutputState},
    reexports::{
        calloop_wayland_source::WaylandSource,
        client::{
            Connection, Proxy as _, QueueHandle, WEnum,
            globals::registry_queue_init,
            protocol::{
                wl_data_device::WlDataDevice,
                wl_data_device_manager::DndAction,
                wl_data_source::WlDataSource,
                wl_keyboard::{self, KeyState, KeymapFormat, WlKeyboard},
                wl_output::{self, WlOutput},
                wl_pointer::AxisSource,
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
        xdg::{
            XdgShell,
            window::{Window, WindowConfigure, WindowDecorations, WindowHandler},
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};

use crate::{
    canvas::Canvas,
    fractional::{self, Extension, ScaleOf, Scaled},
    geom::{Rect, Size},
    ime::{self, Composing, Composition, Done},
    input::{Button, KeyEvent, Mods, PointerEvent},
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
    /// Be an ordinary window instead of a layer surface. The layer, anchor,
    /// margin, exclusive zone, and keyboard fields are then not used, and
    /// `width`/`height` are only the size to start at: the compositor
    /// decides from there.
    pub window: Option<WindowSpec>,
}

/// What a window is called, and whose it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowSpec {
    /// Identifies the program to the compositor (for Niri `window-rule`s)
    /// and to the desktop (its `.desktop` file name).
    pub app_id: String,
    pub title: String,
    /// The smallest it is any use at, in logical pixels.
    pub min_size: (u32, u32),
    /// Another program's window this one belongs to, as an `xdg-foreign`
    /// handle — what the desktop portal passes as `wayland:<handle>`. The
    /// compositor then keeps this window above that one, as a dialog.
    pub parent: Option<String>,
    /// What can be dropped on it, by MIME type, best first
    /// (`text/uri-list` for files). Empty: nothing.
    pub drops: Vec<String>,
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
            window: None,
        }
    }

    /// An ordinary window of `width × height` to start with.
    pub fn window(window: WindowSpec, width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            keyboard: Keyboard::Exclusive,
            window: Some(window),
            ..Self::new("", Layer::Top, Anchor::NONE)
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
    /// What the clipboard held, as asked for with [`Cx::paste`]: the
    /// first of the asked-for types it offered, and the bytes.
    Paste {
        mime: String,
        data: Vec<u8>,
    },
    /// A window was asked to close (its close binding, or the compositor
    /// going away). It stays open unless the app calls [`Cx::exit`].
    Close,
    /// A window was maximised or made full-screen, or stopped being so.
    /// Apps round corners and draw edges only when it is free.
    WindowState {
        maximized: bool,
        fullscreen: bool,
        activated: bool,
    },
    /// Something the window takes is being dragged over it, here.
    DragOver {
        x: f32,
        y: f32,
    },
    /// It left again without being dropped.
    DragLeave,
    /// It was dropped here: in this format, these bytes. `moving` if the
    /// drag means to move rather than copy.
    Drop {
        mime: String,
        data: Vec<u8>,
        x: f32,
        y: f32,
        moving: bool,
    },
}

/// The pointer's look over a surface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Cursor {
    #[default]
    Default,
    /// Over something to click: a link, a button.
    Pointer,
    /// Over text that can be selected or typed into.
    Text,
    /// Over something that can be picked up and dragged.
    Grab,
    Grabbing,
    /// A drop here copies.
    Copy,
    /// A drop here makes a link.
    Alias,
    NotAllowed,
    /// Busy, but still listening.
    Progress,
    /// Over an edge that resizes something side to side, or up and down.
    ColResize,
    RowResize,
}

impl From<Cursor> for CursorIcon {
    fn from(cursor: Cursor) -> Self {
        match cursor {
            Cursor::Default => Self::Default,
            Cursor::Pointer => Self::Pointer,
            Cursor::Text => Self::Text,
            Cursor::Grab => Self::Grab,
            Cursor::Grabbing => Self::Grabbing,
            Cursor::Copy => Self::Copy,
            Cursor::Alias => Self::Alias,
            Cursor::NotAllowed => Self::NotAllowed,
            Cursor::Progress => Self::Progress,
            Cursor::ColResize => Self::ColResize,
            Cursor::RowResize => Self::RowResize,
        }
    }
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

/// Something to put on the clipboard: the same thing in one or more
/// formats, each a MIME type and its bytes. A pasting program takes the
/// format it understands best.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Clip {
    pub formats: Vec<(String, Vec<u8>)>,
}

impl Clip {
    /// Plain text, under every name programs look for it by.
    pub fn text(text: &str) -> Self {
        let mut clip = Self::default();
        for mime in TEXT_TYPES {
            clip = clip.with(mime, text.as_bytes().to_vec());
        }
        clip
    }

    /// Add a format.
    #[must_use]
    pub fn with(mut self, mime: &str, data: Vec<u8>) -> Self {
        self.formats.push((mime.to_owned(), data));
        self
    }
}

/// The names plain text goes by on a clipboard, best first.
pub const TEXT_TYPES: [&str; 5] = [
    "text/plain;charset=utf-8",
    "text/plain",
    "UTF8_STRING",
    "STRING",
    "TEXT",
];

/// Work the runtime gets back from its own threads.
enum Internal {
    Paste {
        mime: String,
        data: Vec<u8>,
    },
    Drop {
        mime: String,
        data: Vec<u8>,
        x: f32,
        y: f32,
        moving: bool,
    },
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
    cursor: Option<Cursor>,
    title: Option<String>,
    begin_move: bool,
    toggle_maximized: bool,
    copy: Option<Clip>,
    paste: Option<Vec<String>>,
    drag: Option<Clip>,
}

/// Handed to an app while it handles an event: lets it measure text and ask
/// the runtime to redraw, resize, wake it later, or exit.
#[derive(Debug)]
pub struct Cx<'a> {
    size: (u32, u32),
    mods: Mods,
    text: &'a mut TextEngine,
    pub(crate) requests: Requests,
}

impl<'a> Cx<'a> {
    pub(crate) fn new(size: (u32, u32), text: &'a mut TextEngine) -> Self {
        Self {
            size,
            mods: Mods::default(),
            text,
            requests: Requests::default(),
        }
    }

    /// Current surface size in logical pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The modifier keys held as this event happened: what makes a click
    /// a Ctrl-click.
    pub fn mods(&self) -> Mods {
        self.mods
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

    /// Change how the pointer looks while it is over the surface.
    pub fn set_cursor(&mut self, cursor: Cursor) {
        self.requests.cursor = Some(cursor);
    }

    /// Retitle a window.
    pub fn set_title(&mut self, title: &str) {
        self.requests.title = Some(title.to_owned());
    }

    /// Let the pointer carry a window away: call while handling the press
    /// that grabbed it (on a title row, say). Floating windows follow it;
    /// tiled ones are the compositor's to place.
    pub fn begin_move(&mut self) {
        self.requests.begin_move = true;
    }

    /// Put `clip` on the clipboard, for this and every other program.
    /// Call while handling the key or click that asked for it: the
    /// compositor only lets input that just happened change the clipboard.
    pub fn copy(&mut self, clip: Clip) {
        self.requests.copy = Some(clip);
    }

    /// Ask for the clipboard, in the first of `mimes` it offers; it arrives
    /// as [`Event::Paste`], unless the clipboard is empty or has none of
    /// them.
    pub fn paste(&mut self, mimes: &[&str]) {
        self.requests.paste = Some(mimes.iter().map(|mime| (*mime).to_owned()).collect());
    }

    /// Start dragging `clip` out of the window, to drop here or in any
    /// other program. Call while the button that began it is still held.
    pub fn start_drag(&mut self, clip: Clip) {
        self.requests.drag = Some(clip);
    }

    /// Maximise a window, or give it back its own size.
    pub fn toggle_maximized(&mut self) {
        self.requests.toggle_maximized = true;
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
    let shm = Shm::bind(&globals, &qh).map_err(|err| Error::new("compositor lacks wl_shm", err))?;
    let pool =
        SlotPool::new(256 * 256 * 4, &shm).map_err(|err| Error::new("shared memory", err))?;

    let spec = app.surface();
    let wl_surface = compositor.create_surface(&qh);
    // Exact pixels on a fractionally scaled output need both protocols;
    // without either, whole-number scales are used.
    let fractional: Option<(WpFractionalScaleManagerV1, WpViewporter)> = globals
        .bind(&qh, 1..=1, Extension)
        .ok()
        .zip(globals.bind(&qh, 1..=1, Extension).ok());
    let viewport = fractional.map(|(scales, viewporter)| {
        let viewport = viewporter.get_viewport(&wl_surface, &qh, Extension);
        let scale = scales.get_fractional_scale(&wl_surface, &qh, ScaleOf(wl_surface.clone()));
        // The surface keeps its own; the globals aren't needed again.
        scales.destroy();
        viewporter.destroy();
        (viewport, scale)
    });
    let surface = match &spec.window {
        None => {
            let layer_shell = LayerShell::bind(&globals, &qh)
                .map_err(|err| Error::new("compositor lacks wlr-layer-shell", err))?;
            Surface::Layer(layer_shell.create_layer_surface(
                &qh,
                wl_surface,
                spec.layer.into(),
                Some(spec.namespace.clone()),
                None,
            ))
        }
        Some(window_spec) => {
            let xdg = XdgShell::bind(&globals, &qh)
                .map_err(|err| Error::new("compositor lacks xdg-shell", err))?;
            // Decorations are the app's to draw: Niri draws a border and a
            // focus ring around windows, never a title bar.
            let window = xdg.create_window(wl_surface, WindowDecorations::RequestClient, &qh);
            window.set_app_id(window_spec.app_id.clone());
            window.set_title(window_spec.title.clone());
            let (min_w, min_h) = window_spec.min_size;
            if min_w > 0 || min_h > 0 {
                window.set_min_size(Some((min_w.max(1), min_h.max(1))));
            }
            // A parent that can't be found leaves a window of its own, not
            // no window at all.
            let imported = window_spec.parent.as_deref().and_then(|handle| {
                let importer: ZxdgImporterV2 = globals.bind(&qh, 1..=1, ForeignData).ok()?;
                let imported = importer.import_toplevel(handle.to_owned(), &qh, ForeignData);
                imported.set_parent_of(window.wl_surface());
                importer.destroy();
                Some(imported)
            });
            Surface::Window {
                window,
                _xdg: xdg,
                _parent: imported,
            }
        }
    };

    let (internal, internals) = channel::channel();
    handle
        .insert_source(internals, |event, (), runtime: &mut Runtime<A>| {
            match event {
                channel::Event::Msg(Internal::Paste { mime, data }) => {
                    runtime.deliver(Event::Paste { mime, data });
                }
                channel::Event::Msg(Internal::Drop {
                    mime,
                    data,
                    x,
                    y,
                    moving,
                }) => {
                    runtime.deliver(Event::Drop {
                        mime,
                        data,
                        x,
                        y,
                        moving,
                    });
                    // Taken: the drag is over.
                    if let Some(offer) = runtime.dropping.take() {
                        offer.finish();
                        offer.destroy();
                    }
                }
                channel::Event::Closed => {}
            }
        })
        .map_err(|err| Error::new("event loop", err))?;
    // Without one there is no clipboard, and nothing else is lost.
    let data_devices = DataDeviceManagerState::bind(&globals, &qh).ok();
    // Without one, keys type only what the keyboard layout has.
    let text_inputs: Option<ZwpTextInputManagerV3> = globals.bind(&qh, 1..=1, ime::Manager).ok();

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
        surface,
        conn,
        qh,
        handle,
        text: TextEngine::new(appearance),
        size: (spec.width.max(1), spec.height.max(1)),
        scale120: 120,
        viewport,
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
        seat: None,
        press_serial: None,
        input_serial: None,
        data_devices,
        data_device: None,
        text_inputs,
        text_input: None,
        composition: Composition::default(),
        copied: None,
        dragging: None,
        dropping: None,
        drag_mime: None,
        internal,
        cursor: Cursor::Default,
        maximized: false,
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
    surface: Surface,
    conn: Connection,
    qh: QueueHandle<Self>,
    handle: LoopHandle<'static, Self>,
    text: TextEngine,
    spec: SurfaceSpec,
    /// Logical size from the last configure.
    size: (u32, u32),
    /// Scale in 120ths, as the fractional-scale protocol counts.
    scale120: u32,
    /// Present when buffers are sized by a fractional scale.
    viewport: Option<(WpViewport, WpFractionalScaleV1)>,
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
    /// The seat the pointer belongs to, and the serial of its latest press:
    /// what a window move has to show the compositor.
    seat: Option<WlSeat>,
    press_serial: Option<u32>,
    /// The serial of the latest key or button press: what changing the
    /// clipboard has to show.
    input_serial: Option<u32>,
    data_devices: Option<DataDeviceManagerState>,
    data_device: Option<DataDevice>,
    text_inputs: Option<ZwpTextInputManagerV3>,
    /// The seat's text input, for input methods.
    text_input: Option<ZwpTextInputV3>,
    composition: Composition,
    /// What this program has on the clipboard, while it has it.
    copied: Option<(CopyPasteSource, Clip)>,
    /// What this program is dragging, while it is.
    dragging: Option<(DragSource, Clip)>,
    /// A drop whose data is on its way: finished once it's here.
    dropping: Option<DragOffer>,
    /// The format being taken from what's dragged over the window.
    drag_mime: Option<String>,
    internal: Sender<Internal>,
    cursor: Cursor,
    maximized: bool,
}

/// The surface's role: what the compositor treats it as.
enum Surface {
    Layer(LayerSurface),
    Window {
        window: Window,
        /// Kept for as long as the window: it owns the `xdg_wm_base`.
        _xdg: XdgShell,
        /// The other program's window this one belongs to, while it does.
        _parent: Option<ZxdgImportedV2>,
    },
}

impl Surface {
    fn wl_surface(&self) -> &WlSurface {
        match self {
            Self::Layer(layer) => layer.wl_surface(),
            Self::Window { window, .. } => window.wl_surface(),
        }
    }

    fn commit(&self) {
        self.wl_surface().commit();
    }

    fn layer(&self) -> Option<&LayerSurface> {
        match self {
            Self::Layer(layer) => Some(layer),
            Self::Window { .. } => None,
        }
    }

    fn window(&self) -> Option<&Window> {
        match self {
            Self::Layer(_) => None,
            Self::Window { window, .. } => Some(window),
        }
    }
}

impl<A: App> Runtime<A> {
    fn deliver(&mut self, event: Event<A::Message>) {
        let mut cx = Cx::new(self.size, &mut self.text);
        cx.mods = self.keys.mods();
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
            // A window's size is the compositor's call once it is mapped.
            if self.visible
                && let Some(layer) = self.surface.layer()
            {
                layer.set_size(width, height);
                layer.commit();
            }
        }
        if let Some(region) = requests.input_region
            && region != self.input_region
        {
            self.input_region = region;
            if self.visible {
                self.send_input_region();
                // Surface state only takes effect with a commit.
                self.surface.commit();
            }
        }
        if let Some(keyboard) = requests.keyboard
            && keyboard != self.spec.keyboard
        {
            self.spec.keyboard = keyboard;
            if self.visible
                && let Some(layer) = self.surface.layer()
            {
                layer.set_keyboard_interactivity(keyboard.into());
                layer.commit();
            }
        }
        if let Some(cursor) = requests.cursor
            && cursor != self.cursor
        {
            self.cursor = cursor;
            self.show_cursor();
        }
        if let Some(clip) = requests.copy {
            self.copy(clip);
        }
        if let Some(clip) = requests.drag {
            self.start_drag(clip);
        }
        if let Some(mimes) = requests.paste {
            self.paste(&mimes);
        }
        if let Some(window) = self.surface.window() {
            if let Some(title) = requests.title {
                window.set_title(title);
            }
            if requests.begin_move
                && let (Some(seat), Some(serial)) = (&self.seat, self.press_serial)
            {
                window.move_(seat, serial);
            }
            if requests.toggle_maximized {
                if self.maximized {
                    window.unset_maximized();
                } else {
                    window.set_maximized();
                }
            }
        }
        if let Some(visible) = requests.visible
            && visible != self.visible
        {
            self.set_visible(visible);
        }
    }

    fn copy(&mut self, clip: Clip) {
        let (Some(devices), Some(device), Some(serial)) =
            (&self.data_devices, &self.data_device, self.input_serial)
        else {
            return;
        };
        let source = devices
            .create_copy_paste_source(&self.qh, clip.formats.iter().map(|(mime, _)| mime.as_str()));
        source.set_selection(device, serial);
        self.copied = Some((source, clip));
    }

    fn start_drag(&mut self, clip: Clip) {
        let (Some(devices), Some(device), Some(serial)) =
            (&self.data_devices, &self.data_device, self.press_serial)
        else {
            return;
        };
        let source = devices.create_drag_and_drop_source(
            &self.qh,
            clip.formats.iter().map(|(mime, _)| mime.as_str()),
            DndAction::Copy | DndAction::Move,
        );
        source.start_drag(device, self.surface.wl_surface(), None, serial);
        self.dragging = Some((source, clip));
    }

    /// The drag over the window now, if it offers something it takes:
    /// take that format, and say so.
    fn take_drag(&mut self) -> bool {
        let Some(offer) = self
            .data_device
            .as_ref()
            .and_then(|device| device.data().drag_offer())
        else {
            return false;
        };
        let drops: Vec<String> = self
            .spec
            .window
            .as_ref()
            .map(|window| window.drops.clone())
            .unwrap_or_default();
        let mime = offer
            .with_mime_types(|offered| drops.iter().find(|mime| offered.contains(mime)).cloned());
        offer.accept_mime_type(offer.serial, mime.clone());
        // Our own drag moves; one from another program copies, so a
        // drop never takes something away from the program it came from.
        let preferred = if self.dragging.is_some() {
            DndAction::Move
        } else {
            DndAction::Copy
        };
        offer.set_actions(DndAction::Copy | DndAction::Move, preferred);
        let taken = mime.is_some();
        self.drag_mime = mime;
        taken
    }

    fn paste(&mut self, mimes: &[String]) {
        let Some(offer) = self
            .data_device
            .as_ref()
            .and_then(|device| device.data().selection_offer())
        else {
            return;
        };
        let Some(mime) = offer
            .with_mime_types(|offered| mimes.iter().find(|mime| offered.contains(mime)).cloned())
        else {
            return;
        };
        let Ok(mut pipe) = offer.receive(mime.clone()) else {
            return;
        };
        // On a thread: the program writing it may be this one, which can't
        // write while it waits here to read.
        let internal = self.internal.clone();
        let _ = std::thread::Builder::new()
            .name("enso-paste".to_owned())
            .spawn(move || {
                let mut data = Vec::new();
                if std::io::Read::read_to_end(&mut pipe, &mut data).is_ok() {
                    let _ = internal.send(Internal::Paste { mime, data });
                }
            });
    }

    /// Queue the current input region on the surface (applied on commit).
    fn send_input_region(&self) {
        let surface = self.surface.wl_surface();
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
        if !visible {
            self.stop_repeat();
        }
        let layer = match &self.surface {
            Surface::Layer(layer) => layer,
            Surface::Window { window, .. } => {
                if visible {
                    // Mapping starts with a bufferless commit; the
                    // compositor answers with a configure.
                    self.send_input_region();
                } else {
                    window.wl_surface().attach(None, 0, 0);
                }
                window.commit();
                return;
            }
        };
        if visible {
            // (Re)mapping starts with a bufferless commit carrying the full
            // layer state; the compositor answers with a configure.
            let spec = &self.spec;
            // Unmapping resets a layer surface, and Niri then forgets the
            // layer it was created on and falls back to the background one —
            // a re-shown toast would sit behind every window. So the layer is
            // stated again on each map (possible since layer-shell version 2).
            if let SurfaceKind::Wlr(surface) = layer.kind()
                && surface.version() >= 2
            {
                layer.set_layer(spec.layer.into());
            }
            layer.set_anchor(wlr_layer::Anchor::from_bits_truncate(spec.anchor.0));
            layer.set_size(spec.width, spec.height);
            layer.set_exclusive_zone(spec.exclusive_zone);
            let Margin {
                top,
                right,
                bottom,
                left,
            } = spec.margin;
            layer.set_margin(top, right, bottom, left);
            layer.set_keyboard_interactivity(spec.keyboard.into());
            self.send_input_region();
            layer.commit();
        } else {
            layer.wl_surface().attach(None, 0, 0);
            layer.commit();
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
        // Without a viewport the compositor only takes whole-number scales.
        let ((width, height), scale120) =
            fractional::buffer(self.size, self.scale120, self.viewport.is_some());
        let (width, height) = (width as i32, height as i32);
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
            scale120 as f32 / 120.0,
            true,
            &mut self.text,
        );
        canvas.set_preedit(self.composition.preedit().cloned());
        let paint = self.app.draw(&mut canvas, Instant::now());
        let caret = canvas.text_input();
        if let Some(request) = self.composition.drawn(caret) {
            self.request(request);
        }

        let surface = self.surface.wl_surface();
        match &self.viewport {
            Some((viewport, _)) => {
                surface.set_buffer_scale(1);
                viewport.set_destination(self.size.0 as i32, self.size.1 as i32);
            }
            None => surface.set_buffer_scale((scale120 / 120) as i32),
        }
        surface.damage_buffer(0, 0, width, height);
        if paint == Paint::Animating {
            surface.frame(&self.qh, FrameCallbackData(surface.clone()));
            self.frame_pending = true;
        }
        if let Err(err) = buffer.attach_to(surface) {
            return self.fail(Error::new("cannot attach a frame buffer", err));
        }
        self.surface.commit();
    }

    fn fail(&mut self, error: Error) {
        self.failure.get_or_insert(error);
        self.exit = true;
    }

    fn is_own(&self, surface: &WlSurface) -> bool {
        self.surface.wl_surface() == surface
    }

    /// Show the app's chosen cursor. Cosmetic: a missing cursor theme must
    /// not break input.
    fn show_cursor(&self) {
        if let Some(pointer) = &self.pointer {
            let _ = pointer.set_cursor(&self.conn, self.cursor.into());
        }
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

impl<A: App> Composing for Runtime<A> {
    fn is_own(&self, surface: &WlSurface) -> bool {
        Runtime::is_own(self, surface)
    }

    fn composition(&mut self) -> &mut Composition {
        &mut self.composition
    }

    fn request(&mut self, request: ime::Request) {
        if let Some(input) = &self.text_input {
            ime::send(input, request);
        }
    }

    fn composed(&mut self, done: Done) {
        self.dirty |= done.redraw;
        if let Some(text) = done.commit {
            for key in ime::keys(&text) {
                self.deliver(Event::Key(key));
            }
        }
    }
}

impl<A: App> Scaled for Runtime<A> {
    fn prefer_scale(&mut self, surface: &WlSurface, scale120: u32) {
        if self.is_own(surface) && scale120 != self.scale120 {
            self.scale120 = scale120;
            self.dirty = true;
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
                serial,
                state: WEnum::Value(state),
                ..
            } => match state {
                KeyState::Pressed => {
                    runtime.input_serial = Some(serial);
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
        // The fractional scale, where there is one, is the better answer.
        let scale120 = factor.max(1) as u32 * 120;
        if self.is_own(surface) && self.viewport.is_none() && scale120 != self.scale120 {
            self.scale120 = scale120;
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

impl<A: App> WindowHandler for Runtime<A> {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &Window) {
        self.deliver(Event::Close);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        if !self.visible {
            return;
        }
        // No size suggested: the window picks, keeping what it has.
        let current = if self.configured {
            self.size
        } else {
            (self.spec.width.max(1), self.spec.height.max(1))
        };
        let size = (
            configure.new_size.0.map_or(current.0, |w| w.get()),
            configure.new_size.1.map_or(current.1, |h| h.get()),
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
        self.maximized = configure.is_maximized();
        self.deliver(Event::WindowState {
            maximized: self.maximized,
            fullscreen: configure.is_fullscreen(),
            activated: configure.is_activated(),
        });
    }
}

/// User data for the `xdg-foreign` objects that tie a window to another
/// program's.
struct ForeignData;

impl<A: App> Dispatch2<ZxdgImporterV2, Runtime<A>> for ForeignData {
    fn event(
        &self,
        _: &mut Runtime<A>,
        _: &ZxdgImporterV2,
        _: <ZxdgImporterV2 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<Runtime<A>>,
    ) {
    }
}

impl<A: App> Dispatch2<ZxdgImportedV2, Runtime<A>> for ForeignData {
    fn event(
        &self,
        runtime: &mut Runtime<A>,
        imported: &ZxdgImportedV2,
        event: zxdg_imported_v2::Event,
        _: &Connection,
        _: &QueueHandle<Runtime<A>>,
    ) {
        // The parent closed, or the handle was never good: the window
        // carries on as one of its own.
        if let zxdg_imported_v2::Event::Destroyed = event {
            imported.destroy();
            if let Surface::Window { _parent, .. } = &mut runtime.surface {
                *_parent = None;
            }
        }
    }
}

impl<A: App> DataSourceHandler for Runtime<A> {
    fn accept_mime(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataSource,
        _: Option<String>,
    ) {
    }

    fn send_request(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        source: &WlDataSource,
        mime: String,
        mut pipe: WritePipe,
    ) {
        let copied = self
            .copied
            .as_ref()
            .filter(|(copied, _)| copied.inner() == source)
            .map(|(_, clip)| clip);
        let dragged = self
            .dragging
            .as_ref()
            .filter(|(dragged, _)| dragged.inner() == source)
            .map(|(_, clip)| clip);
        let Some(data) = copied
            .or(dragged)
            .and_then(|clip| clip.formats.iter().find(|(offered, _)| *offered == mime))
            .map(|(_, data)| data.clone())
        else {
            return;
        };
        // On a thread, so a slow reader can't stall the window.
        let _ = std::thread::Builder::new()
            .name("enso-copy".to_owned())
            .spawn(move || {
                let _ = std::io::Write::write_all(&mut pipe, &data);
            });
    }

    fn cancelled(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        // Something else took the clipboard.
        if self
            .copied
            .as_ref()
            .is_some_and(|(copied, _)| copied.inner() == source)
        {
            self.copied = None;
        }
        // A drag let go over nothing that takes it.
        if self
            .dragging
            .as_ref()
            .is_some_and(|(dragged, _)| dragged.inner() == source)
        {
            self.dragging = None;
        }
    }

    fn dnd_dropped(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource) {}

    fn dnd_finished(&mut self, _: &Connection, _: &QueueHandle<Self>, source: &WlDataSource) {
        if self
            .dragging
            .as_ref()
            .is_some_and(|(dragged, _)| dragged.inner() == source)
        {
            self.dragging = None;
        }
    }

    fn action(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataSource, _: DndAction) {}
}

impl<A: App> DataDeviceHandler for Runtime<A> {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &WlDataDevice,
        x: f64,
        y: f64,
        _: &WlSurface,
    ) {
        if self.take_drag() {
            self.deliver(Event::DragOver {
                x: x as f32,
                y: y as f32,
            });
        }
    }

    fn leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {
        // A drop also leaves; its data is still on its way.
        if self.drag_mime.take().is_some() && self.dropping.is_none() {
            self.deliver(Event::DragLeave);
        }
    }

    fn motion(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice, x: f64, y: f64) {
        if self.drag_mime.is_some() {
            self.deliver(Event::DragOver {
                x: x as f32,
                y: y as f32,
            });
        }
    }

    fn selection(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {}

    fn drop_performed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &WlDataDevice) {
        let (Some(offer), Some(mime)) = (
            self.data_device
                .as_ref()
                .and_then(|device| device.data().drag_offer()),
            self.drag_mime.clone(),
        ) else {
            return;
        };
        let Ok(mut pipe) = offer.receive(mime.clone()) else {
            return;
        };
        let (x, y) = (offer.x as f32, offer.y as f32);
        let moving = offer.selected_action == DndAction::Move;
        self.dropping = Some(offer);
        // On a thread: the program writing it may be this one.
        let internal = self.internal.clone();
        let _ = std::thread::Builder::new()
            .name("enso-drop".to_owned())
            .spawn(move || {
                let mut data = Vec::new();
                if std::io::Read::read_to_end(&mut pipe, &mut data).is_ok() {
                    let _ = internal.send(Internal::Drop {
                        mime,
                        data,
                        x,
                        y,
                        moving,
                    });
                }
            });
    }
}

impl<A: App> DataOfferHandler for Runtime<A> {
    fn source_actions(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
    }

    fn selected_action(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &mut DragOffer,
        _: DndAction,
    ) {
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
                self.text_input = self
                    .text_inputs
                    .as_ref()
                    .map(|inputs| inputs.get_text_input(&seat, qh, ime::TextInputData));
                if self.data_device.is_none() {
                    self.data_device = self
                        .data_devices
                        .as_ref()
                        .map(|devices| devices.get_data_device(qh, &seat));
                }
            }
            Capability::Pointer if self.pointer.is_none() => {
                self.seat = Some(seat.clone());
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
                if let Some(input) = self.text_input.take() {
                    input.destroy();
                    self.dirty |= self.composition.leave();
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
        _: &Connection,
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
                    self.show_cursor();
                    PointerEvent::Enter { x, y }
                }
                PointerEventKind::Leave { .. } => PointerEvent::Leave,
                PointerEventKind::Motion { .. } => PointerEvent::Move { x, y },
                PointerEventKind::Press {
                    button: code,
                    serial,
                    ..
                } => {
                    self.press_serial = Some(*serial);
                    self.input_serial = Some(*serial);
                    PointerEvent::Press {
                        x,
                        y,
                        button: button(*code),
                    }
                }
                PointerEventKind::Release { button: code, .. } => PointerEvent::Release {
                    x,
                    y,
                    button: button(*code),
                },
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    source,
                    ..
                } => PointerEvent::Scroll {
                    x,
                    y,
                    dx: horizontal.absolute as f32,
                    dy: vertical.absolute as f32,
                    // Wheels click from notch to notch; fingers glide.
                    wheel: matches!(source, Some(AxisSource::Wheel | AxisSource::WheelTilt))
                        || horizontal.value120 != 0
                        || vertical.value120 != 0,
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
