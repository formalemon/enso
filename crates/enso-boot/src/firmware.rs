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

use crate::menu::{self, Act, Choice, Key, Menu, Picture, Screen};

static SCENE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/scene.bin"));
static FRAME: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/frame.bin"));
static CARDS: [&[u8]; 2] = [
    include_bytes!(concat!(env!("OUT_DIR"), "/card-0.bin")),
    include_bytes!(concat!(env!("OUT_DIR"), "/card-1.bin")),
];

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

    let mut gop = open_screen().map_err(fail)?;
    let (width, height) = gop.current_mode_info().resolution();
    let scene = menu::open(SCENE).ok_or(Status::LOAD_ERROR)?;
    let cards: Vec<Picture> = CARDS
        .iter()
        .map(|blob| menu::open(blob).ok_or(Status::LOAD_ERROR))
        .collect::<Result<_, _>>()?;

    let mut backdrop = Screen::new(width, height, [0, 0, 0, 255]);
    backdrop.cover(&scene);
    drop(scene);
    let mut menu = Menu::default();
    show_menu(&mut gop, &backdrop, &cards, &menu).map_err(fail)?;

    let choice = loop {
        match menu.key(read_key()?) {
            Act::Nothing => {}
            Act::Moved => show_menu(&mut gop, &backdrop, &cards, &menu).map_err(fail)?,
            Act::Start(choice) => break choice,
            Act::Leave => return Err(Status::ABORTED),
        }
    };
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

fn show_menu(
    gop: &mut GraphicsOutput,
    backdrop: &Screen,
    cards: &[Picture],
    menu: &Menu,
) -> uefi::Result {
    let mut screen = backdrop.clone();
    if let Some(card) = cards.get(menu.chosen) {
        let (x, y) = menu::card_at((screen.width, screen.height), (card.width, card.height));
        screen.over(card, x, y);
    }
    show(gop, &screen)
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
        let pressed = system::with_stdin(|stdin| stdin.read_key()).map_err(|e| e.status())?;
        let key = match pressed {
            Some(Pressed::Special(ScanCode::UP)) => Key::Up,
            Some(Pressed::Special(ScanCode::DOWN)) => Key::Down,
            Some(Pressed::Special(ScanCode::ESCAPE)) => Key::Escape,
            Some(Pressed::Printable(c)) if u16::from(c) == 0x0d => Key::Enter,
            Some(Pressed::Printable(c)) if u16::from(c) == 0x09 => Key::Down,
            Some(_) => Key::Other,
            None => continue,
        };
        return Ok(key);
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
