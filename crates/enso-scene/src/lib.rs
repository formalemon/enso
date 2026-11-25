//! The pictures, in voxels, in the theme's colours, computed from a handful
//! of functions so they are the same every time they are built.
//!
//! [`pagoda`] and [`orbit`]: the boot menu's — a five-storeyed pagoda on an
//! island floating in the sky at dusk, which the camera goes round. The
//! island is built and lit on the build machine; the firmware draws its
//! faces a frame at a time, and needs no operating system.
//!
//! [`render`] and [`life`]: the login screen's — rice terraces stepping
//! down to a river, the river winding off between hills to the mountains, a
//! few cherry trees on the banks, and the moon over it all. The land is a
//! height field on a grid; each pixel casts a ray from the camera and steps
//! through the grid a column at a time (a 2-D DDA) until it meets a top or
//! a side, lit by the moon, shadowed if the moon is hidden from it, fading
//! into the dusk with distance. [`render`] notes in a [`Life`] what in it
//! can move, and [`life`] paints that motion.
//!
//! Without the default `render` feature (threads, for the terraces' rays)
//! the rest is built without the standard library.

#![cfg_attr(not(feature = "render"), no_std)]

extern crate alloc;

pub mod life;
mod noise;
pub mod orbit;
pub mod pagoda;
#[cfg(feature = "render")]
mod render;

pub use life::Life;
#[cfg(feature = "render")]
pub use render::{render, render_with_life};

/// The colours the picture is made of: a theme's palette, as RGB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Palette {
    pub base: [u8; 3],
    pub surface: [u8; 3],
    pub overlay: [u8; 3],
    pub muted: [u8; 3],
    pub subtle: [u8; 3],
    pub text: [u8; 3],
    pub love: [u8; 3],
    pub gold: [u8; 3],
    pub rose: [u8; 3],
    pub pine: [u8; 3],
    pub foam: [u8; 3],
    pub iris: [u8; 3],
}

