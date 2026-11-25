//! The program the firmware runs: the menu on the screen, the keys, and
//! starting what was chosen.
//!
//! This is the one place in enso that uses `unsafe` (allowed here alone,
//! by the crate's lints): a UEFI program is started by the firmware through
//! an exported entry point, and hands a kernel its command line as a raw
//! pointer the firmware keeps. Each use says why it is sound.
//!
//! Anything that goes wrong ends the program with an error. The firmware
//! then goes on to its next boot option (systemd-boot), so a broken menu
//! never leaves the machine unable to start.

#![allow(unsafe_code)]

use alloc::{string::String, vec::Vec};

use uefi::{
    CString16, Handle, Status, boot,
    boot::{LoadImageSource, ScopedProtocol},
    cstr16, entry,
    fs::FileSystem,
    proto::{
        BootPolicy,
        console::{
            gop::{BltOp, BltPixel, BltRegion, GraphicsOutput},
            text::{Key as Pressed, ScanCode},
        },
        device_path::{
            DevicePath,
            build::{DevicePathBuilder, media::FilePath},
        },
        loaded_image::LoadedImage,
    },
    system,
};

use enso_scene::{
    life::{Order, Rect},
    orbit::Orbit,
    pagoda::Model,
};

use crate::menu::{self, Act, Choice, Key, Menu, Picture, Screen};

static SKY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/sky.bin"));
static PAGODA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/pagoda.bin"));
static FRAME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/frame.bin"));
static CARDS: [&[u8]; 2] = [
    include_bytes!(concat!(env!("OUT_DIR"), "/card-0.bin")),
    include_bytes!(concat!(env!("OUT_DIR"), "/card-1.bin")),
];

/// The most frames a second the island turns at: no more than the eye
/// needs, and the processor idles between.
const FPS: u64 = 60;
/// The clock goes back to naught this often, in seconds, so it never
/// grows too long to count in a float; once an hour, the stars' twinkle
/// skips a beat.
const WRAP: u64 = 3600;

/// Where Windows' own boot manager is, on the same boot partition.
const WINDOWS: &str = "\\EFI\\Microsoft\\Boot\\bootmgfw.efi";

#[entry]
fn efi_main() -> Status {
    match run() {
        Ok(()) => Status::SUCCESS,
        Err(status) => status,
    }
}

fn run() -> Result<(), Status> {
    let fail = |err: uefi::Error| err.status();
    // The firmware's watchdog restarts a machine whose boot program takes
    // more than five minutes; a menu waits as long as it takes.
    let _ = boot::set_watchdog_timer(0, 0x10000, None);

    let gop = open_screen().map_err(fail)?;
    let (width, height) = gop.current_mode_info().resolution();
    let sky = menu::open(SKY).ok_or(Status::LOAD_ERROR)?;
    let cards: Vec<Picture> = CARDS
        .iter()
        .map(|blob| menu::open(blob).ok_or(Status::LOAD_ERROR))
        .collect::<Result<_, _>>()?;

    let mut backdrop = Screen::new(width, height, [0, 0, 0, 255]);
    backdrop.cover(&sky);
    drop(sky);
    let model = menu::unpack(PAGODA).and_then(|bytes| Model::decode(&bytes));
    let mut menu = Menu::default();
    let mut display = Display::new(gop, &backdrop);

    let choice = match &model {
        Some(model) => turn(&mut display, &mut menu, &backdrop, model, &cards)?,
        // Without the island, the sky alone.
        None => still(&mut display, &mut menu, &backdrop, &cards)?,
    };
    drop(model);
    let mut gop = display.gop;
    drop(backdrop);
    match choice {
        Choice::Arch => {
            // The ensō, on the colour the login screen will start from:
            // the quiet boot leaves it on screen while Linux starts.
            let frame = menu::open(FRAME).ok_or(Status::LOAD_ERROR)?;
            let colour = frame
                .bgra
                .get(..4)
                .map_or([0, 0, 0, 255], |px| [px[0], px[1], px[2], 255]);
            let mut screen = Screen::new(width, height, colour);
            screen.centre(&frame);
            show(&mut gop, &screen).map_err(fail)?;
            drop(gop);
            start_arch()
        }
        Choice::Windows => {
            drop(gop);
            start(WINDOWS, None)
        }
    }
}

/// The screen, ours alone until something else is started.
fn open_screen() -> uefi::Result<ScopedProtocol<GraphicsOutput>> {
    let handle = boot::get_handle_for_protocol::<GraphicsOutput>()?;
    boot::open_protocol_exclusive::<GraphicsOutput>(handle)
}

/// The screen, and what is on it: changed a rectangle at a time.
struct Display {
    gop: ScopedProtocol<GraphicsOutput>,
    screen: Screen,
    pixels: Vec<BltPixel>,
}

impl Display {
    fn new(gop: ScopedProtocol<GraphicsOutput>, backdrop: &Screen) -> Self {
        Self {
            gop,
            screen: backdrop.clone(),
            pixels: Vec::from_iter(core::iter::repeat_n(
                BltPixel::new(0, 0, 0),
                backdrop.width * backdrop.height,
            )),
        }
    }

    /// Put `rect` of the screen as it now is on the display.
    fn put(&mut self, rect: Rect) -> Result<(), Status> {
        let width = self.screen.width;
        for y in rect.y as usize..(rect.y + rect.h) as usize {
            for x in rect.x as usize..(rect.x + rect.w) as usize {
                let i = y * width + x;
                let px = &self.screen.bgra[i * 4..i * 4 + 4];
                self.pixels[i] = BltPixel::new(px[2], px[1], px[0]);
            }
        }
        self.gop
            .blt(BltOp::BufferToVideo {
                buffer: &self.pixels,
                src: BltRegion::SubRectangle {
                    coords: (rect.x as usize, rect.y as usize),
                    px_stride: width,
                },
                dest: (rect.x as usize, rect.y as usize),
                dims: (rect.w as usize, rect.h as usize),
            })
            .map_err(|e| e.status())
    }

    fn whole(&self) -> Rect {
        Rect {
            x: 0,
            y: 0,
            w: self.screen.width as u32,
            h: self.screen.height as u32,
        }
    }
}

/// The menu over the island as the camera goes round it, until a choice
/// is made: each frame, the places that changed redrawn, the card put back
/// over them, and the keys looked at.
fn turn(
    display: &mut Display,
    menu: &mut Menu,
    backdrop: &Screen,
    model: &Model,
    cards: &[Picture],
) -> Result<Choice, Status> {
    let (width, height) = (display.screen.width, display.screen.height);
    let mut orbit = Orbit::new(model, width as u32, height as u32);
    // The clock is set going once the first frame is up, so nothing waits
    // for it.
    let mut clock: Option<Clock> = None;
    loop {
        let began = ticks();
        let card = &cards[menu.chosen.min(cards.len() - 1)];
        let (x, y) = menu::card_at((width, height), (card.width, card.height));
        let changed = orbit.paint(
            &backdrop.bgra,
            &mut display.screen.bgra,
            Order::Bgra,
            clock.as_ref().map_or(0.0, Clock::seconds),
        );
        for rect in changed {
            display.screen.over_part(card, x, y, rect);
            display.put(rect)?;
        }
        if let Some(key) = poll_key()? {
            match menu.key(key) {
                Act::Nothing => {}
                // The other card goes up with the next frame.
                Act::Moved => orbit.owe(Rect {
                    x: x as u32,
                    y: y as u32,
                    w: card.width as u32,
                    h: card.height as u32,
                }),
                Act::Start(choice) => return Ok(choice),
                Act::Leave => return Err(Status::ABORTED),
            }
        }
        // Rest out what is left of the frame.
        let clock = clock.get_or_insert_with(Clock::start);
        let frame = clock.per_second / FPS;
        let spent = ticks().saturating_sub(began);
        if spent < frame {
            let rest = (frame - spent) * 1_000_000 / clock.per_second.max(1);
            boot::stall(core::time::Duration::from_micros(rest));
        }
    }
}

/// The processor's time-stamp counter: ticks at a steady rate.
fn ticks() -> u64 {
    // SAFETY: `rdtsc` only reads the time-stamp counter, which every
    // x86-64 processor has, and touches no memory.
    #[allow(unused_unsafe)]
    unsafe {
        core::arch::x86_64::_rdtsc()
    }
}

/// Seconds since the menu came up, from the time-stamp counter, its rate
/// measured against the firmware's own wait.
struct Clock {
    start: u64,
    per_second: u64,
}

impl Clock {
    fn start() -> Self {
        let before = ticks();
        boot::stall(core::time::Duration::from_millis(10));
        let start = ticks();
        Self {
            start,
            per_second: start.saturating_sub(before).saturating_mul(100).max(1),
        }
    }

    fn seconds(&self) -> f32 {
        let ticks = ticks().saturating_sub(self.start) % (WRAP * self.per_second);
        (ticks as f64 / self.per_second as f64) as f32
    }
}

/// The menu over the still scene, until a choice is made.
fn still(
    display: &mut Display,
    menu: &mut Menu,
    backdrop: &Screen,
    cards: &[Picture],
) -> Result<Choice, Status> {
    let draw = |display: &mut Display, chosen: usize| {
        display.screen = backdrop.clone();
        if let Some(card) = cards.get(chosen) {
            let (x, y) = menu::card_at(
                (display.screen.width, display.screen.height),
                (card.width, card.height),
            );
            display.screen.over(card, x, y);
        }
        let whole = display.whole();
        display.put(whole)
    };
    draw(display, menu.chosen)?;
    loop {
        match menu.key(read_key()?) {
            Act::Nothing => {}
            Act::Moved => draw(display, menu.chosen)?,
            Act::Start(choice) => return Ok(choice),
            Act::Leave => return Err(Status::ABORTED),
        }
    }
}

/// Put a whole screen up at once.
fn show(gop: &mut GraphicsOutput, screen: &Screen) -> uefi::Result {
    let pixels: Vec<BltPixel> = screen
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .map(|px| BltPixel::new(px[2], px[1], px[0]))
        .collect();
    gop.blt(BltOp::BufferToVideo {
        buffer: &pixels,
        src: BltRegion::Full,
        dest: (0, 0),
        dims: (screen.width, screen.height),
    })
}

/// Wait for a key the menu knows.
fn read_key() -> Result<Key, Status> {
    loop {
        let event =
            system::with_stdin(|stdin| stdin.wait_for_key_event()).map_err(|e| e.status())?;
        boot::wait_for_event(&[event]).map_err(|e| e.status())?;
        if let Some(key) = poll_key()? {
            return Ok(key);
        }
    }
}

/// The key pressed since last asked, if any, without waiting.
fn poll_key() -> Result<Option<Key>, Status> {
    loop {
        // Keys the menu doesn't know are let go, and the next looked at.
        let pressed = system::with_stdin(|stdin| stdin.read_key()).map_err(|e| e.status())?;
        let key = match pressed {
            Some(Pressed::Special(ScanCode::UP)) => Key::Up,
            Some(Pressed::Special(ScanCode::DOWN)) => Key::Down,
            Some(Pressed::Special(ScanCode::ESCAPE)) => Key::Escape,
            Some(Pressed::Printable(c)) if u16::from(c) == 0x0d => Key::Enter,
            Some(Pressed::Printable(c)) if u16::from(c) == 0x09 => Key::Down,
            Some(_) => Key::Other,
            None => return Ok(None),
        };
        if key != Key::Other {
            return Ok(Some(key));
        }
    }
}

/// Arch, as systemd-boot's entry for it says: its kernel, initrds and
/// options.
fn start_arch() -> Result<(), Status> {
    let mut files =
        FileSystem::new(boot::get_image_file_system(boot::image_handle()).map_err(|e| e.status())?);
    let dir = cstr16!("\\loader\\entries");
    let mut texts: Vec<(String, String)> = Vec::new();
    for info in files
        .read_dir(dir)
        .map_err(|_| Status::NOT_FOUND)?
        .flatten()
    {
        if info.is_directory() {
            continue;
        }
        let name = String::from(info.file_name());
        if !name.ends_with(".conf") {
            continue;
        }
        let mut path = CString16::from(dir);
        path.push_str(cstr16!("\\"));
        path.push_str(info.file_name());
        if let Ok(bytes) = files.read(path.as_ref())
            && let Ok(text) = String::from_utf8(bytes)
        {
            texts.push((name, text));
        }
    }
    drop(files);
    let entry = menu::arch_entry(texts.iter().map(|(n, t)| (n.as_str(), t.as_str())))
        .ok_or(Status::NOT_FOUND)?;
    let line = menu::command_line(&entry);
    start(&menu::firmware_path(&entry.linux), Some(&line))
}

/// The full device path of `file` on the partition this program was
/// started from.
fn on_this_partition(file: &str, storage: &mut Vec<u8>) -> Result<(), Status> {
    let me = boot::open_protocol_exclusive::<LoadedImage>(boot::image_handle())
        .map_err(|e| e.status())?;
    let device = me.device().ok_or(Status::NOT_FOUND)?;
    drop(me);
    let path = boot::open_protocol_exclusive::<DevicePath>(device).map_err(|e| e.status())?;
    let name = CString16::try_from(file).map_err(|_| Status::INVALID_PARAMETER)?;
    let mut builder = DevicePathBuilder::with_vec(storage);
    for node in path.node_iter() {
        builder = builder.push(&node).map_err(|_| Status::BUFFER_TOO_SMALL)?;
    }
    builder
        .push(&FilePath {
            path_name: name.as_ref(),
        })
        .and_then(DevicePathBuilder::finalize)
        .map_err(|_| Status::BUFFER_TOO_SMALL)?;
    Ok(())
}

/// Load the program at `file` on this partition and run it, with
/// `options` as its command line.
fn start(file: &str, options: Option<&str>) -> Result<(), Status> {
    let mut storage = Vec::new();
    on_this_partition(file, &mut storage)?;
    let path = <&DevicePath>::try_from(storage.as_slice()).map_err(|_| Status::LOAD_ERROR)?;
    let child: Handle = boot::load_image(
        boot::image_handle(),
        LoadImageSource::FromDevicePath {
            device_path: path,
            boot_policy: BootPolicy::ExactMatch,
        },
    )
    .map_err(|e| e.status())?;
    if let Some(options) = options {
        let line = CString16::try_from(options).map_err(|_| Status::INVALID_PARAMETER)?;
        // Kept for good: the started program reads it whenever it likes.
        let line: &'static CString16 = alloc::boxed::Box::leak(alloc::boxed::Box::new(line));
        let bytes = line.num_bytes();
        let size = u32::try_from(bytes).map_err(|_| Status::INVALID_PARAMETER)?;
        let mut image =
            boot::open_protocol_exclusive::<LoadedImage>(child).map_err(|e| e.status())?;
        // SAFETY: the command line is leaked above, so it outlives the
        // started program; `size` is its length in bytes, terminator
        // included, as the field expects.
        unsafe {
            image.set_load_options(line.as_ptr().cast(), size);
        }
    }
    boot::start_image(child).map_err(|e| e.status())
}
