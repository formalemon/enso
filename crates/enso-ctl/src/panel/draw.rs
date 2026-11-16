//! Painting the panel: the card, the first page's tiles and sliders, the
//! list pages, and the small widgets they share.
//!
//! Everything is drawn from the panel's state as it is this frame; the only
//! thing painting writes back is geometry for hit-testing.

use std::time::Instant;

use enso_core::{
    Icon, Palette, Rgba,
    style::{Shadow, icon, opacity, radius, space, stroke, type_scale},
};
use enso_sys::{
    bluetooth::Device,
    power::BatteryStatus,
    wifi::{Link, Security},
};
use enso_ui::{Align, Canvas, Paint, Rect, Text};

use super::{
    Dial, Hit, Item, NOTICE_HEIGHT, PAD, Page, Panel, ROW_HEIGHT, Section, TILE_HEIGHT,
    bluetooth_summary, wifi_summary,
};
use crate::{
    cli::{Edge, PanelMode},
    system::Hardware,
};

/// Size of the on/off switch.
const SWITCH: (f32, f32) = (34.0, 20.0);
/// Width kept for a chevron at the end of a row.
const CHEVRON: f32 = 18.0;
/// Width kept for a slider's reading.
const READING: f32 = 40.0;

/// A small labelled reading in the summary row.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Tile {
    icon: Icon,
    value: String,
    label: String,
    /// What a click on it does, if anything.
    hit: Option<Hit>,
}

/// The power readings worth a tile, in order.
pub(super) fn tiles(hardware: &Hardware) -> Vec<Tile> {
    let mut tiles = Vec::new();
    if let Some(battery) = &hardware.battery {
        let label = match (battery.status, battery.minutes_left) {
            (BatteryStatus::Charging, Some(minutes)) => format!("{} to full", brief(minutes)),
            (BatteryStatus::Charging, None) => "Charging".to_owned(),
            (BatteryStatus::Discharging, Some(minutes)) => format!("{} left", brief(minutes)),
            (BatteryStatus::Full, _) => "Full".to_owned(),
            (BatteryStatus::NotCharging, _) => "Plugged in".to_owned(),
            _ => "Battery".to_owned(),
        };
        tiles.push(Tile {
            icon: if battery.is_plugged_in() {
                icon::BATTERY_CHARGING
            } else {
                icon::BATTERY
            },
            value: format!("{}%", battery.percent),
            label,
            hit: None,
        });
    }
    if let Some(profile) = &hardware.profile {
        tiles.push(Tile {
            icon: icon::POWER_PROFILE,
            value: capitalised(&profile.current),
            label: "Profile".to_owned(),
            hit: Some(Hit::Profile),
        });
    }
    if let Some(temperature) = hardware.temperature {
        // The fastest fan says how hard the machine is working.
        let label = match hardware.fans.iter().map(|fan| fan.rpm).max() {
            Some(rpm) if rpm > 0 => format!("{rpm} rpm"),
            _ => "CPU".to_owned(),
        };
        tiles.push(Tile {
            icon: icon::THERMOMETER,
            value: format!("{temperature:.0}°"),
            label,
            hit: None,
        });
    }
    tiles
}

/// A duration short enough for a tile: `2h 15m`, `45m`.
fn brief(minutes: u32) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes}m"),
        (hours, 0) => format!("{hours}h"),
        (hours, minutes) => format!("{hours}h {minutes}m"),
    }
}

/// `balanced` → `Balanced`.
fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// What a frame needs to know about the look, gathered once.
struct Look {
    colors: Palette,
    accent: Rgba,
    /// Body text size.
    base: f32,
    /// Caption text size.
    small: f32,
    /// Width of the icon column at the start of a row.
    icon_width: f32,
}

/// What a row shows at its far end.
enum Trailing<'a> {
    Nothing,
    /// A quiet word: what selecting the row would do, or what it is doing.
    Word(&'a str),
    /// The row is the one in use.
    Check,
}

impl Panel {
    pub(super) fn paint(&mut self, canvas: &mut Canvas<'_>, now: Instant) -> Paint {
        let presence = self.presence.get(now);
        let base = self.ctx.config.appearance.font_size;
        let look = Look {
            colors: self.ctx.theme.colors.clone(),
            accent: self.ctx.accent(),
            base,
            small: base * type_scale::SMALL,
            icon_width: if canvas.glyphs() { 26.0 } else { 38.0 },
        };

        // The card eases in from the edge it hangs off.
        let from = match self.mode {
            PanelMode::Dropdown(Edge::Bottom) => 1.0,
            _ => -1.0,
        };
        let card = self
            .card_rect(canvas.bounds(), self.height.get(now))
            .translate(0.0, from * (1.0 - presence) * space::MD);
        self.card = card;
        self.hits.clear();

        canvas.set_opacity(presence);
        canvas.shadow(card, radius::CARD, Shadow::FLOATING, Rgba::rgb(0, 0, 0));
        canvas.fill(card, radius::CARD, look.colors.surface);
        canvas.stroke(
            card,
            radius::CARD,
            stroke::HAIRLINE,
            look.colors.highlight_med,
        );

        // A newly opened page fades in while sliding the last little way.
        let slide = self.slide.get(now);
        let shift = slide * self.slide_from * space::LG;
        canvas.set_opacity(presence * (1.0 - slide));
        let bottom = match self.page {
            Page::Main => self.paint_main(canvas, card, shift, &look, now),
            _ => self.paint_page(canvas, card, shift, &look, now),
        };
        if let Some(notice) = &self.notice {
            let line = Rect::new(
                card.x + PAD + shift,
                bottom,
                card.w - 2.0 * PAD,
                NOTICE_HEIGHT,
            );
            if line.bottom() <= card.bottom() {
                canvas.text_in(
                    &Text::new(notice, look.small, look.colors.love),
                    line,
                    Align::Start,
                );
            }
        }
        canvas.set_opacity(1.0);

        Paint::when(
            self.presence.is_animating(now)
                || [
                    &self.highlight,
                    &self.height,
                    &self.slide,
                    &self.wifi_switch,
                    &self.bluetooth_switch,
                    &self.mic_switch,
                ]
                .into_iter()
                .any(|tween| tween.is_animating(now)),
        )
    }

    /// The first page. Returns where its content ends.
    fn paint_main(
        &mut self,
        canvas: &mut Canvas<'_>,
        card: Rect,
        shift: f32,
        look: &Look,
        now: Instant,
    ) -> f32 {
        let colors = &look.colors;
        let inner = card.inset_xy(PAD, 0.0).translate(shift, 0.0);
        let mut y = card.y + PAD;

        let tiles = tiles(&self.hardware);
        if !tiles.is_empty() {
            let gap = space::SM;
            let width = (inner.w - gap * (tiles.len() - 1) as f32) / tiles.len() as f32;
            for (i, tile) in tiles.iter().enumerate() {
                let rect = Rect::new(inner.x + i as f32 * (width + gap), y, width, TILE_HEIGHT);
                canvas.fill(rect, radius::CONTROL, colors.overlay);
                let content = rect.inset_xy(space::SM + 2.0, space::SM);
                let top = Rect::new(content.x, content.y, content.w, content.h / 2.0);
                let bottom = top.translate(0.0, content.h / 2.0);
                // The reading comes first: its glyph is drawn only if there
                // is room beside it, and a long word steps down a size
                // rather than lose its end.
                let value = Text::new(&tile.value, look.base, colors.text).bold();
                let width = canvas.measure(&value).w;
                let value = if width <= content.w {
                    value
                } else {
                    Text::new(&tile.value, look.small, colors.text).bold()
                };
                if canvas.glyphs() && width + space::XS + look.base <= content.w {
                    canvas.icon(tile.icon, look.base, look.accent, top, Align::End);
                }
                canvas.text_in(&value, top, Align::Start);
                canvas.text_in(
                    &Text::new(&tile.label, look.small, colors.subtle),
                    bottom,
                    Align::Start,
                );
                if let Some(hit) = tile.hit {
                    self.hits.push((rect, hit));
                }
            }
            y += TILE_HEIGHT + space::MD;
        }

        let list = Rect::new(card.x + space::SM + shift, y, card.w - 2.0 * space::SM, 0.0);
        let items = self.items();
        self.paint_highlight(canvas, list, look, now);
        for (index, item) in items.iter().enumerate() {
            let row = list.row(index, ROW_HEIGHT, 0.0);
            if row.bottom() > card.bottom() {
                break;
            }
            match *item {
                Item::Slider(dial) => self.paint_slider(canvas, row, index, dial, look),
                Item::Open(page) => self.paint_entry(canvas, row, index, page, look, now),
                _ => {}
            }
        }
        y + items.len() as f32 * ROW_HEIGHT
    }

    /// A slider row: a glyph (the mute button, for a volume), the track, and
    /// the reading. The output's volume ends in the chevron that opens it
    /// out; an application's starts with its name.
    fn paint_slider(
        &mut self,
        canvas: &mut Canvas<'_>,
        row: Rect,
        index: usize,
        dial: Dial,
        look: &Look,
    ) {
        let Some((percent, muted)) = self.dial(dial) else {
            return;
        };
        let colors = &look.colors;
        let content = row.inset_xy(space::SM, 0.0);
        let glyph = match dial {
            Dial::Brightness => icon::BRIGHTNESS,
            Dial::Mic if muted => icon::MICROPHONE_OFF,
            Dial::Mic => icon::MICROPHONE,
            _ if muted => icon::VOLUME_MUTED,
            _ if percent < 34 => icon::VOLUME_LOW,
            _ if percent < 67 => icon::VOLUME_MEDIUM,
            _ => icon::VOLUME,
        };
        let size = look.base + 2.0;
        let button = Rect::new(content.x, content.y, look.icon_width, content.h);
        canvas.icon(glyph, size, colors.subtle, button, Align::Start);

        // Every slider keeps the chevron's room, so the tracks line up.
        let mut right = content.right() - CHEVRON - space::XS;
        let reading = format!("{}%", percent.min(100));
        canvas.text_in(
            &Text::new(&reading, look.small, colors.subtle),
            Rect::new(right - READING, content.y, READING, content.h),
            Align::End,
        );
        right -= READING + space::SM;

        let mut left = button.right() + space::SM;
        if let Dial::App(app) = dial {
            const NAME: f32 = 76.0;
            if let Some(app) = self.apps().get(app) {
                canvas.text_in(
                    &Text::new(&app.name, look.small, colors.text),
                    Rect::new(left, content.y, NAME, content.h),
                    Align::Start,
                );
            }
            left += NAME + space::SM;
        }

        // The knob travels the whole track, so its centre stops a radius
        // short of each end.
        const KNOB: f32 = 7.0;
        let track = Rect::new(left, content.y + (content.h - 6.0) / 2.0, right - left, 6.0);
        let travel = Rect::new(track.x + KNOB, track.y, track.w - 2.0 * KNOB, track.h);
        let knob = travel.x + travel.w * f32::from(percent.min(100)) / 100.0;
        canvas.pill(track, colors.highlight_med);
        // A muted volume keeps its place, but loses its colour.
        let fill = if muted { colors.muted } else { look.accent };
        canvas.pill(
            Rect::new(track.x, track.y, knob + KNOB - track.x, track.h),
            fill,
        );
        let (_, cy) = track.center();
        canvas.circle(knob, cy, KNOB, colors.text);

        // Specific targets first: the mute button and the chevron, then the
        // row, which is the slider.
        if dial != Dial::Brightness {
            let target = Rect::new(row.x, row.y, button.right() + space::XS - row.x, row.h);
            self.hits.push((target, Hit::Mute(dial)));
        }
        // On the first page the volume leads on to the Sound page.
        if dial == Dial::Volume && self.page == Page::Main {
            let chevron = Rect::new(content.right() - CHEVRON, content.y, CHEVRON, content.h);
            canvas.icon(
                icon::CHEVRON_RIGHT,
                look.base + 2.0,
                colors.muted,
                chevron,
                Align::End,
            );
            let target = Rect::new(
                chevron.x - space::SM,
                row.y,
                row.right() - chevron.x + space::SM,
                row.h,
            );
            self.hits.push((target, Hit::OpenSound));
        }
        self.hits.push((row, Hit::Slider(index, dial, travel)));
    }

    /// A row of the first page that opens another: Wi-Fi, Bluetooth, Theme.
    fn paint_entry(
        &mut self,
        canvas: &mut Canvas<'_>,
        row: Rect,
        index: usize,
        page: Page,
        look: &Look,
        now: Instant,
    ) {
        let colors = &look.colors;
        let content = row.inset_xy(space::SM, 0.0);
        let (glyph, title, detail, switch) = match page {
            Page::Wifi => (
                icon::WIFI,
                "Wi-Fi",
                wifi_summary(self.wifi.as_ref()),
                self.wifi.as_ref().map(|_| self.wifi_switch.get(now)),
            ),
            Page::Bluetooth => (
                icon::BLUETOOTH,
                "Bluetooth",
                bluetooth_summary(self.bluetooth.as_ref()),
                self.bluetooth
                    .as_ref()
                    .map(|_| self.bluetooth_switch.get(now)),
            ),
            _ => (icon::PALETTE, "Theme", self.ctx.theme.name.clone(), None),
        };
        // A page that cannot be opened is shown, but dimmed.
        let available = match page {
            Page::Wifi => self.wifi.is_some(),
            Page::Bluetooth => self.bluetooth.is_some(),
            _ => true,
        };
        let dim = if available { 1.0 } else { opacity::DISABLED };

        canvas.icon(
            glyph,
            look.base + 2.0,
            colors.subtle.alpha(dim),
            content,
            Align::Start,
        );
        let mut right = content.right();
        if available {
            canvas.icon(
                icon::CHEVRON_RIGHT,
                look.base + 2.0,
                colors.muted,
                Rect::new(right - CHEVRON, content.y, CHEVRON, content.h),
                Align::End,
            );
            right -= CHEVRON + space::XS;
        }
        if let Some(on) = switch {
            let rect = Rect::new(
                right - SWITCH.0,
                content.y + (content.h - SWITCH.1) / 2.0,
                SWITCH.0,
                SWITCH.1,
            );
            paint_switch(canvas, rect, on, look);
            // A generous target around the switch, ahead of the row's own.
            self.hits.push((
                Rect::new(rect.x - space::SM, row.y, rect.w + 2.0 * space::SM, row.h),
                Hit::Switch(page),
            ));
            right = rect.x - space::MD;
        }

        let x = content.x + look.icon_width + space::SM;
        let title = Text::new(title, look.base, colors.text.alpha(dim)).medium();
        let used = canvas
            .text_in(
                &title,
                Rect::new(x, content.y, right - x, content.h),
                Align::Start,
            )
            .w;
        let detail_x = x + used + space::MD;
        if right - detail_x > 40.0 {
            canvas.text_in(
                &Text::new(&detail, look.small, colors.subtle.alpha(dim)),
                Rect::new(detail_x, content.y, right - detail_x, content.h),
                Align::Start,
            );
        }
        self.hits.push((row, Hit::Item(index)));
    }

    /// A list page: header, then rows. Returns where its content ends.
    fn paint_page(
        &mut self,
        canvas: &mut Canvas<'_>,
        card: Rect,
        shift: f32,
        look: &Look,
        now: Instant,
    ) -> f32 {
        let colors = &look.colors;
        let header = Rect::new(
            card.x + space::SM + shift,
            card.y + space::SM,
            card.w - 2.0 * space::SM,
            ROW_HEIGHT,
        );
        let content = header.inset_xy(space::SM, 0.0);
        let (title, status, switch) = match self.page {
            Page::Wifi => (
                "Wi-Fi",
                self.wifi
                    .as_ref()
                    .filter(|wifi| wifi.powered && wifi.scanning)
                    .map(|_| "Scanning…"),
                Some(self.wifi_switch.get(now)),
            ),
            Page::Bluetooth => (
                "Bluetooth",
                self.bluetooth
                    .as_ref()
                    .filter(|bluetooth| bluetooth.powered && bluetooth.discovering)
                    .map(|_| "Searching…"),
                Some(self.bluetooth_switch.get(now)),
            ),
            Page::Sound => ("Sound", None, None),
            _ => ("Theme", None, None),
        };

        canvas.icon(
            icon::CHEVRON_LEFT,
            look.base + 2.0,
            colors.subtle,
            content,
            Align::Start,
        );
        let x = content.x + look.icon_width.min(22.0) + space::XS;
        canvas.text_in(
            &Text::new(title, look.base, colors.text).bold(),
            Rect::new(x, content.y, content.w, content.h),
            Align::Start,
        );
        let mut right = content.right();
        if let Some(on) = switch {
            let rect = Rect::new(
                right - SWITCH.0,
                content.y + (content.h - SWITCH.1) / 2.0,
                SWITCH.0,
                SWITCH.1,
            );
            paint_switch(canvas, rect, on, look);
            self.hits.push((
                Rect::new(
                    rect.x - space::SM,
                    header.y,
                    rect.w + 2.0 * space::SM,
                    header.h,
                ),
                Hit::Switch(self.page),
            ));
            right = rect.x - space::MD;
        }
        if let Some(status) = status {
            canvas.text_in(
                &Text::new(status, look.small, colors.muted),
                Rect::new(x, content.y, right - x, content.h),
                Align::End,
            );
        }
        self.hits.push((header, Hit::Back));

        let list = Rect::new(header.x, header.bottom() + space::XS, header.w, 0.0);
        let items = self.items();
        if items.is_empty() {
            let row = list.row(0, ROW_HEIGHT, 0.0).inset_xy(space::SM, 0.0);
            if row.bottom() <= card.bottom() {
                canvas.text_in(
                    &Text::new(self.empty_reason(), look.base, colors.muted),
                    row,
                    Align::Start,
                );
            }
            return list.y + ROW_HEIGHT;
        }

        self.paint_highlight(canvas, list, look, now);
        let prompt = self.prompt_index();
        let rows = self.visible_rows();
        let mut bottom = list.y;
        for (index, item) in items.iter().enumerate().skip(self.offset).take(rows) {
            let row = list.row(self.slot(index), ROW_HEIGHT, 0.0);
            // While the card is still growing, rows below its edge wait.
            if row.bottom() > card.bottom() {
                break;
            }
            let selected = index == self.selected;
            match *item {
                Item::Network(i) => self.paint_network(canvas, row, i, selected, look),
                Item::Device(i) => self.paint_device(canvas, row, i, selected, look),
                Item::Theme(i) => self.paint_theme(canvas, row, i, look),
                Item::Slider(dial) => self.paint_slider(canvas, row, index, dial, look),
                Item::Output(i) => self.paint_sound_device(canvas, row, false, i, selected, look),
                Item::Input(i) => self.paint_sound_device(canvas, row, true, i, selected, look),
                Item::Heading(section) => self.paint_heading(canvas, row, section, look, now),
                Item::Silence => {
                    let line = row.inset_xy(space::SM, 0.0);
                    canvas.text_in(
                        &Text::new("Nothing is playing", look.base, look.colors.muted),
                        Rect::new(line.x + look.icon_width + space::SM, line.y, line.w, line.h),
                        Align::Start,
                    );
                }
                Item::Open(_) => {}
            }
            // Headings are not for choosing; sliders have targets of their own.
            if Self::selectable(*item) && !matches!(item, Item::Slider(_)) {
                self.hits.push((row, Hit::Item(index)));
            }
            bottom = row.bottom();

            if prompt == Some(index) {
                let field = row.translate(0.0, ROW_HEIGHT);
                if field.bottom() <= card.bottom() {
                    self.paint_prompt(canvas, field, look);
                    bottom = field.bottom();
                }
            }
        }
        bottom
    }

    /// Why a list page has nothing to list.
    fn empty_reason(&self) -> &'static str {
        match self.page {
            Page::Wifi => match &self.wifi {
                Some(wifi) if !wifi.powered => "Wi-Fi is off",
                Some(wifi) if wifi.scanning => "Looking for networks…",
                _ => "No networks in range",
            },
            Page::Bluetooth => match &self.bluetooth {
                Some(bluetooth) if !bluetooth.powered => "Bluetooth is off",
                _ => "Looking for devices…",
            },
            Page::Sound => "No sound devices",
            _ => "",
        }
    }

    /// The selection highlight, gliding between rows.
    fn paint_highlight(&self, canvas: &mut Canvas<'_>, list: Rect, look: &Look, now: Instant) {
        if self.items().is_empty() {
            return;
        }
        let highlight = list
            .row(0, ROW_HEIGHT, 0.0)
            .translate(0.0, self.highlight.get(now) * ROW_HEIGHT);
        if highlight.bottom() <= self.card.bottom() {
            canvas.fill(highlight, radius::CONTROL, look.colors.overlay);
        }
    }

    fn paint_network(
        &self,
        canvas: &mut Canvas<'_>,
        row: Rect,
        index: usize,
        selected: bool,
        look: &Look,
    ) {
        let Some(network) = self.networks().get(index) else {
            return;
        };
        let colors = &look.colors;
        let content = row.inset_xy(space::SM, 0.0);
        let lit = if network.connected {
            look.accent
        } else {
            colors.text
        };
        paint_bars(canvas, content, network.bars(), lit, colors.highlight_med);

        let busy = self.busy.as_deref() == Some(network.id.as_str());
        let connecting = busy
            || network.connected
                && self
                    .wifi
                    .as_ref()
                    .is_some_and(|wifi| wifi.link == Link::Connecting);
        let trailing = match (connecting, selected, network.connected) {
            (true, ..) => Trailing::Word("Connecting…"),
            // Shown only where it applies, so the list stays quiet.
            (false, true, true) => Trailing::Word("Disconnect"),
            (false, true, false) if self.prompt.is_none() => Trailing::Word("Connect"),
            (false, false, true) => Trailing::Check,
            _ => Trailing::Nothing,
        };
        let locked = network.security != Security::Open;
        paint_label(
            canvas,
            content,
            look,
            &network.name,
            network.connected,
            locked.then_some(icon::LOCK),
            None,
            &trailing,
        );
    }

    fn paint_device(
        &self,
        canvas: &mut Canvas<'_>,
        row: Rect,
        index: usize,
        selected: bool,
        look: &Look,
    ) {
        let Some(device) = self.devices().get(index) else {
            return;
        };
        let colors = &look.colors;
        let content = row.inset_xy(space::SM, 0.0);
        let tint = if device.connected {
            look.accent
        } else {
            colors.subtle
        };
        canvas.icon(
            icon::BLUETOOTH,
            look.base + 2.0,
            tint,
            content,
            Align::Start,
        );

        let busy = self.busy.as_deref() == Some(device.id.as_str());
        let trailing = match (busy, selected, device.connected, device.paired) {
            (true, ..) => Trailing::Word("Connecting…"),
            (false, true, true, _) => Trailing::Word("Disconnect"),
            (false, true, false, true) => Trailing::Word("Connect"),
            (false, true, false, false) => Trailing::Word("Pair"),
            (false, false, true, _) => Trailing::Check,
            _ => Trailing::Nothing,
        };
        let detail = device_detail(device);
        paint_label(
            canvas,
            content,
            look,
            &device.name,
            device.connected,
            None,
            detail.as_deref(),
            &trailing,
        );
    }

    /// The heading of a part of the Sound page. The microphone's carries
    /// its on/off switch.
    fn paint_heading(
        &mut self,
        canvas: &mut Canvas<'_>,
        row: Rect,
        section: Section,
        look: &Look,
        now: Instant,
    ) {
        let colors = &look.colors;
        let content = row.inset_xy(space::SM, 0.0);
        let title = match section {
            Section::Output => "Output",
            Section::Apps => "Applications",
            Section::Microphone => "Microphone",
        };
        // Set low in its row, so it sits with the rows it heads.
        let label = Rect::new(
            content.x,
            content.y + space::SM,
            content.w,
            content.h - space::SM,
        );
        canvas.text_in(
            &Text::new(title, look.small, colors.subtle).medium(),
            label,
            Align::Start,
        );
        if section == Section::Microphone && self.dial(Dial::Mic).is_some() {
            let rect = Rect::new(
                content.right() - SWITCH.0,
                label.y + (label.h - SWITCH.1) / 2.0,
                SWITCH.0,
                SWITCH.1,
            );
            paint_switch(canvas, rect, self.mic_switch.get(now), look);
            self.hits.push((
                Rect::new(rect.x - space::SM, row.y, rect.w + 2.0 * space::SM, row.h),
                Hit::MicSwitch,
            ));
        }
    }

    /// An output device or a microphone: the one in use is marked, and the
    /// selected one says what choosing it does.
    fn paint_sound_device(
        &self,
        canvas: &mut Canvas<'_>,
        row: Rect,
        input: bool,
        index: usize,
        selected: bool,
        look: &Look,
    ) {
        let Some(device) = self.sound_devices(input).get(index) else {
            return;
        };
        let colors = &look.colors;
        let content = row.inset_xy(space::SM, 0.0);
        let glyph = if input {
            icon::MICROPHONE
        } else {
            icon::VOLUME
        };
        let tint = if device.current {
            look.accent
        } else {
            colors.subtle
        };
        canvas.icon(glyph, look.base + 2.0, tint, content, Align::Start);
        let trailing = match (device.current, selected) {
            (true, _) => Trailing::Check,
            (false, true) => Trailing::Word("Use"),
            (false, false) => Trailing::Nothing,
        };
        paint_label(
            canvas,
            content,
            look,
            &device.name,
            device.current,
            None,
            None,
            &trailing,
        );
    }

    fn paint_theme(&self, canvas: &mut Canvas<'_>, row: Rect, index: usize, look: &Look) {
        let Some(theme) = self.ctx.themes.iter().nth(index) else {
            return;
        };
        let colors = &look.colors;
        let content = row.inset_xy(space::SM, 0.0);

        // Each theme previews itself with its own colours.
        let (_, cy) = content.center();
        let swatches = [
            theme.colors.base,
            theme.colors.rose,
            theme.colors.foam,
            theme.colors.iris,
        ];
        for (i, swatch) in swatches.into_iter().enumerate() {
            let cx = content.x + 7.0 + i as f32 * 11.0;
            canvas.circle(cx, cy, 7.0, colors.highlight_high);
            canvas.circle(cx, cy, 6.0, swatch);
        }

        let active = theme.id == self.ctx.theme.id;
        let name = Text::new(&theme.name, look.base, colors.text);
        let name = if active { name.medium() } else { name };
        canvas.text_in(
            &name,
            Rect::new(
                content.x + 60.0,
                content.y,
                content.w - 60.0 - 28.0,
                content.h,
            ),
            Align::Start,
        );
        if active {
            canvas.icon(
                icon::CHECK,
                look.base + 2.0,
                look.accent,
                content,
                Align::End,
            );
        }
    }

    /// The passphrase field under the network it is for.
    fn paint_prompt(&self, canvas: &mut Canvas<'_>, row: Rect, look: &Look) {
        let Some(prompt) = &self.prompt else { return };
        let colors = &look.colors;
        let field = row.inset_xy(space::SM, 5.0);
        canvas.fill(field, radius::CONTROL, colors.base);
        canvas.stroke(field, radius::CONTROL, stroke::HAIRLINE, look.accent);

        let inner = field.inset_xy(space::MD, 0.0);
        let typed = if prompt.text.is_empty() {
            let (hint, tint) = if prompt.rejected {
                ("Not accepted — try again", colors.love)
            } else {
                ("Password", colors.muted)
            };
            canvas.text_in(&Text::new(hint, look.base, tint), inner, Align::Start);
            0.0
        } else {
            // Never the passphrase itself: one dot a character.
            let dots = "•".repeat(prompt.text.chars().count());
            canvas
                .text_in(
                    &Text::new(&dots, look.base, colors.text),
                    inner,
                    Align::Start,
                )
                .w
        };
        let caret = Rect::new(
            (inner.x + typed + 1.0).min(inner.right()),
            inner.y + (inner.h - look.base * 1.3) / 2.0,
            2.0,
            look.base * 1.3,
        );
        canvas.fill(caret, 1.0, look.accent);
    }
}

/// What to say next to a Bluetooth device's name.
fn device_detail(device: &Device) -> Option<String> {
    match (device.connected, device.battery, device.paired) {
        (true, Some(percent), _) => Some(format!("{percent}%")),
        (false, _, false) => Some("Nearby".to_owned()),
        _ => None,
    }
}

/// The text part of a list row: name, an optional small glyph and detail
/// after it, and what the row ends with.
#[allow(clippy::too_many_arguments)]
fn paint_label(
    canvas: &mut Canvas<'_>,
    content: Rect,
    look: &Look,
    name: &str,
    emphasised: bool,
    badge: Option<Icon>,
    detail: Option<&str>,
    trailing: &Trailing<'_>,
) {
    let colors = &look.colors;
    let mut right = content.right();
    match trailing {
        Trailing::Nothing => {}
        Trailing::Word(word) => {
            let word = Text::new(word, look.small, colors.subtle);
            right -= canvas.text_in(&word, content, Align::End).w + space::MD;
        }
        Trailing::Check => {
            canvas.icon(
                icon::CHECK,
                look.base + 2.0,
                look.accent,
                content,
                Align::End,
            );
            right -= look.base + 2.0 + space::MD;
        }
    }

    let x = content.x + look.icon_width + space::SM;
    let badge = badge.filter(|_| canvas.glyphs());
    let after = if badge.is_some() {
        look.small + space::SM
    } else {
        0.0
    } + detail.map_or(0.0, |_| 56.0);
    let text = Text::new(name, look.base, colors.text).max_width((right - x - after).max(40.0));
    let text = if emphasised { text.medium() } else { text };
    let mut end = x + canvas
        .text_in(
            &text,
            Rect::new(x, content.y, right - x, content.h),
            Align::Start,
        )
        .w;
    if let Some(badge) = badge {
        let slot = Rect::new(end + space::SM, content.y, look.small, content.h);
        canvas.icon(badge, look.small, colors.muted, slot, Align::Start);
        end = slot.right();
    }
    if let Some(detail) = detail
        && right - end > 48.0
    {
        canvas.text_in(
            &Text::new(detail, look.small, colors.subtle),
            Rect::new(
                end + space::MD,
                content.y,
                right - end - space::MD,
                content.h,
            ),
            Align::Start,
        );
    }
}

/// An on/off switch; `on` runs from `0` to `1` as it slides.
fn paint_switch(canvas: &mut Canvas<'_>, rect: Rect, on: f32, look: &Look) {
    let colors = &look.colors;
    canvas.pill(rect, colors.highlight_med.mix(look.accent, on));
    let knob = rect.h - 6.0;
    let travel = rect.w - knob - 6.0;
    let (_, cy) = rect.center();
    canvas.circle(
        rect.x + 3.0 + knob / 2.0 + travel * on,
        cy,
        knob / 2.0,
        colors.subtle.mix(colors.base, on),
    );
}

/// Four rising bars, `lit` of them filled: signal strength without a font.
fn paint_bars(canvas: &mut Canvas<'_>, content: Rect, lit: u8, on: Rgba, off: Rgba) {
    const WIDTH: f32 = 3.0;
    const GAP: f32 = 2.0;
    const TALLEST: f32 = 13.0;
    let (_, cy) = content.center();
    let floor = cy + TALLEST / 2.0;
    for bar in 0..4u8 {
        let height = 4.0 + 3.0 * f32::from(bar);
        let rect = Rect::new(
            content.x + 2.0 + f32::from(bar) * (WIDTH + GAP),
            floor - height,
            WIDTH,
            height,
        );
        canvas.fill(rect, 1.0, if bar < lit { on } else { off });
    }
}
