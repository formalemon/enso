//! Fractional scales: drawing in a screen's real pixels when it's scaled
//! by 1.25 or 1.5, not at the next whole number and blurred down.
//!
//! The compositor tells each surface the scale it prefers, in 120ths
//! (`wp_fractional_scale_v1`); the buffer is drawn at that many pixels,
//! and a viewport (`wp_viewporter`) tells the compositor the size it
//! stands for. Without both protocols, whole-number scales are used, as
//! before. Both runtimes — one surface per output, and one surface — share
//! what's here.

use smithay_client_toolkit::{
    dispatch2::Dispatch2,
    reexports::{
        client::{Connection, QueueHandle, protocol::wl_surface::WlSurface},
        protocols::wp::{
            fractional_scale::v1::client::{
                wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
                wp_fractional_scale_v1::{self, WpFractionalScaleV1},
            },
            viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
        },
    },
};

/// What both protocols' globals, and a surface's viewport, carry: nothing.
pub(crate) struct Extension;

/// User data for a surface's fractional scale: the surface it speaks for.
pub(crate) struct ScaleOf(pub(crate) WlSurface);

/// A runtime that hears the scale its surfaces prefer.
pub(crate) trait Scaled {
    /// `surface` prefers `scale120` 120ths of a pixel per logical pixel.
    fn prefer_scale(&mut self, surface: &WlSurface, scale120: u32);
}

/// Pixels for `logical` pixels at `scale120`, as the protocol rounds them.
pub(crate) fn physical(logical: u32, scale120: u32) -> u32 {
    ((logical * scale120 + 60) / 120).max(1)
}

/// The scale to draw at: `scale120` itself with a viewport to show it
/// through, else the next whole number, which is all a buffer scale takes.
pub(crate) fn drawn(scale120: u32, viewport: bool) -> u32 {
    if viewport {
        scale120.max(1)
    } else {
        scale120.div_ceil(120).max(1) * 120
    }
}

impl<D: 'static> Dispatch2<WpFractionalScaleManagerV1, D> for Extension {
    fn event(
        &self,
        _: &mut D,
        _: &WpFractionalScaleManagerV1,
        _: <WpFractionalScaleManagerV1 as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D: 'static> Dispatch2<WpViewporter, D> for Extension {
    fn event(
        &self,
        _: &mut D,
        _: &WpViewporter,
        _: <WpViewporter as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D: 'static> Dispatch2<WpViewport, D> for Extension {
    fn event(
        &self,
        _: &mut D,
        _: &WpViewport,
        _: <WpViewport as smithay_client_toolkit::reexports::client::Proxy>::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
    }
}

impl<D: Scaled + 'static> Dispatch2<WpFractionalScaleV1, D> for ScaleOf {
    fn event(
        &self,
        state: &mut D,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &Connection,
        _: &QueueHandle<D>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.prefer_scale(&self.0, scale.max(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pixels_at_fractional_scales() {
        // 1.25 and 1.5, as niri offers them.
        assert_eq!(physical(1000, 150), 1250);
        assert_eq!(physical(901, 180), 1352);
        assert_eq!(physical(0, 150), 1);
        assert_eq!(drawn(150, true), 150);
        assert_eq!(drawn(150, false), 240);
        assert_eq!(drawn(120, false), 120);
    }
}
