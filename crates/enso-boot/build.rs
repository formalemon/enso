//! Draws the boot menu's pictures and packs them into the program: only
//! when building the program the firmware runs (`--target
//! x86_64-unknown-uefi`). On the build machine itself `enso-boot` is the
//! installer, which carries no pictures.

extern crate alloc;

#[allow(dead_code)]
#[path = "src/art.rs"]
mod art;
#[allow(dead_code)]
#[path = "src/menu.rs"]
mod menu;

use std::{env, fs, path::PathBuf};

use enso_ui::TextEngine;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/art.rs");
    println!("cargo:rerun-if-changed=src/menu.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("uefi") {
        return;
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("cargo sets OUT_DIR"));
    // The words are set in the interface font of whoever builds.
    let user = enso_core::Context::load();
    println!(
        "cargo:rerun-if-changed={}",
        user.paths.config_file().display()
    );
    let ctx = art::context(Some(user.config.appearance.font.clone()));
    let mut text = TextEngine::new(&ctx.config.appearance);
    let (width, height) = art::SCREEN;

    let save = |name: &str, width: u32, height: u32, mut rgba: Vec<u8>| {
        art::to_bgra(&mut rgba);
        let picture = menu::Picture {
            width: width as usize,
            height: height as usize,
            bgra: rgba,
        };
        fs::write(out.join(name), menu::keep(&picture)).expect("writing a picture");
    };
    save("sky.bin", width, height, art::sky(&ctx, art::SCREEN));
    fs::write(out.join("pagoda.bin"), menu::pack(&art::pagoda(&ctx).encode()))
        .expect("writing the pagoda");
    let scene = art::scene(&ctx, art::SCREEN);
    let frame = art::frame(&ctx, &mut text, &scene);
    save("frame.bin", frame.width, frame.height, frame.rgba);
    for chosen in 0..art::LABELS.len() {
        let card = art::card(&ctx, &mut text, chosen);
        save(
            &format!("card-{chosen}.bin"),
            card.width,
            card.height,
            card.rgba,
        );
    }
}
