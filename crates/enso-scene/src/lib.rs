//! The boot menu's picture: rice terraces stepping down to a river, the
//! river winding off between hills to the mountains, a few cherry trees on
//! the banks, and the moon over it all at dusk. In voxels, in the theme's
//! colours, computed from a handful of functions so it is the same picture
//! every time it is built.
//!
//! The land is a height field on a grid: each column of voxels has a
//! height and what its top and sides are made of; trees stand on it as
//! short spans of voxels over a column. Each pixel casts a ray from the
//! camera and steps through the grid a column at a time (a 2-D DDA) until
//! it meets a top or a side. The face is lit by the moon, shadowed if the
//! moon is hidden from it, and fades into the dusk with distance.

//!
//! [`render`] draws the picture, and notes in a [`Life`] what in it can
//! move: [`life`] paints that motion, and needs no operating system, so the
//! boot menu can run it in the firmware. Without the default `render`
//! feature only that part is built.

#![cfg_attr(not(feature = "render"), no_std)]

extern crate alloc;

pub mod life;
#[cfg(feature = "render")]
mod render;

pub use life::Life;
#[cfg(feature = "render")]
pub use render::{Palette, render, render_with_life};
