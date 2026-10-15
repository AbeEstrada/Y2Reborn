//! Shared widgets of the UI v3 visual system: status bar, list highlights,
//! grouped cards, switches, pills, covers, sheets and toasts.
use crate::{
    canvas::{Canvas, Icon},
    text::{self, Face},
    theme::{alpha, color, fade, mix, HEIGHT, WIDTH},
    Ui,
};
use reborn_core::{
    platform::{ChargingState, LowBattery},
    AppModel, AudioOutput, Track,
};

pub fn time(ms: u64) -> String {
    format!("{}:{:02}", ms / 60_000, (ms / 1000) % 60)
}

pub fn progress(position: u64, duration: u64) -> f32 {
    if duration == 0 {
        0.
    } else {
        (position as f32 / duration as f32).clamp(0., 1.)
    }
}

pub fn output_label(output: &AudioOutput) -> String {
    match output {
        AudioOutput::Wired => "Headphone Jack".into(),
        AudioOutput::Bluetooth(_) => "Bluetooth".into(),
    }
}

/// A stable identity for the cover shared by every track of one album.
pub fn art_key(track: &Track) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    };
    if track.album.is_empty() && track.album_artist.is_empty() {
        feed(
            track
                .path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default()
                .as_bytes(),
        );
    } else {
        feed(track.album.as_bytes());
        feed(if track.album_artist.is_empty() {
            track.artist.as_bytes()
        } else {
            track.album_artist.as_bytes()
        });
    }
    hash | 1
}

// ---------------------------------------------------------------- status bar

/// The top strip: optional back arrow, a tracked-caps title in the middle and
/// the status cluster (radios, charging, battery) on the right.
pub fn status_bar(c: &mut Canvas, ui: &Ui, m: &AppModel, title: &str, back: bool) {
    if back {
        c.icon(Icon::Back, 22., 18., 20., color::MUTED);
    }
    if !title.is_empty() {
        // The clock sits left of the radios; long titles stop short of it.
        let room = if ui.clock(m).is_some() { 180. } else { 250. };
        let mut shown = title.to_owned();
        while shown.chars().count() > 1 && text::width_tracked(Face::S10, &shown, 1.7) > room {
            shown.pop();
            shown = shown.trim_end().to_owned();
            if text::width_tracked(Face::S10, &shown, 1.7) <= room {
                shown.push('…');
                break;
            }
        }
        c.text_center_tracked(
            240.,
            18. + Face::S10.mid_offset(),
            &shown,
            Face::S10,
            color::MUTED,
            1.7,
        );
    }
    status_cluster(c, ui, m, color::MUTED);
}

pub(crate) fn status_cluster(c: &mut Canvas, ui: &Ui, m: &AppModel, tint: u32) {
    let b = m.platform.battery;
    let low = matches!(
        b.level,
        LowBattery::Low | LowBattery::Critical | LowBattery::ShuttingDown
    ) || b.percent.is_some_and(|p| p <= 15);
    let fill = if low { color::DANGER } else { color::TEXT };
    // Battery body, nub and charge level.
    let (x0, y0) = (440., 12.);
    c.rrect_outline(x0, y0, 22., 12., 3., 1., tint);
    c.rect(x0 + 23., y0 + 4., 2., 4., tint);
    if let Some(p) = b.percent.filter(|p| *p <= 100) {
        let w = (18. * f32::from(p) / 100.)
            .round()
            .max(if p > 0 { 2. } else { 0. });
        c.rect(x0 + 2., y0 + 2., w, 8., fill);
    }
    let mut right = x0 - 8.;
    if let Some(p) = b.percent.filter(|p| *p <= 100) {
        let text = p.to_string();
        let w = text::width(Face::S10, &text);
        c.text(
            right - w,
            18. + Face::S10.mid_offset(),
            &text,
            Face::S10,
            if low { color::DANGER } else { tint },
        );
        right -= w + 8.;
    }
    if matches!(b.charging, ChargingState::Charging) {
        c.icon(Icon::Bolt, right - 5., 18., 14., color::SUCCESS);
        right -= 16.;
    }
    if ui.bluetooth.powered {
        let connected = ui.bluetooth.devices.iter().any(|d| d.connected);
        c.icon(
            Icon::Bluetooth,
            right - 7.,
            18.,
            16.,
            if connected { color::TEXT } else { color::DIM },
        );
        right -= 20.;
    }
    if ui.wifi.powered {
        let connected = matches!(ui.wifi.status, crate::WifiStatus::Connected(_));
        c.icon(
            Icon::Wifi,
            right - 7.,
            18.,
            16.,
            if connected { color::TEXT } else { color::DIM },
        );
        right -= 20.;
    }
    if let Some(time) = ui.clock(m) {
        let w = text::width(Face::S10, &time);
        c.text(
            right - w - 2.,
            18. + Face::S10.mid_offset(),
            &time,
            Face::S10,
            tint,
        );
    }
}

// --------------------------------------------------------- highlights, cards

/// The focused row of a list: accent-tinted glass with a hairline.
pub fn accent_row(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32, accent: u32) {
    let start = c.draw.len();
    c.rrect(x, y, w, h, 12., alpha(accent, 46));
    c.mark_focus(start);
    c.rrect_outline(x, y, w, h, 12., 1., alpha(accent, 120));
}

/// The focused row of a settings-style list: neutral glass.
pub fn glass_row(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32) {
    let start = c.draw.len();
    c.rrect(x, y, w, h, 12., color::GLASS);
    c.mark_focus(start);
}

/// A grouped card behind a run of settings rows.
pub fn card(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32) {
    c.rrect(x, y, w, h, 16., color::GLASS_FAINT);
}

pub fn section_label(c: &mut Canvas, x: f32, cy: f32, label: &str) {
    c.text_tracked(
        x,
        cy + Face::S10.mid_offset(),
        &label.to_uppercase(),
        Face::S10,
        color::MUTED,
        1.4,
    );
}

pub fn toggle(c: &mut Canvas, right: f32, cy: f32, on: bool) {
    let (w, h) = (44., 26.);
    let x = right - w;
    let y = cy - h / 2.;
    c.rrect(
        x,
        y,
        w,
        h,
        13.,
        if on {
            color::SUCCESS
        } else {
            color::GLASS_STRONG
        },
    );
    c.disc(x + if on { 31. } else { 13. }, cy, 10., color::TEXT);
}

pub fn check(c: &mut Canvas, cx: f32, cy: f32, tint: u32) {
    c.icon(Icon::Check, cx, cy, 20., tint);
}

pub fn chevron(c: &mut Canvas, right: f32, cy: f32) {
    c.icon(Icon::Chevron, right - 8., cy, 16., color::DIM);
}

/// Four signal bars, `level` of them lit.
pub fn signal_bars(c: &mut Canvas, right: f32, cy: f32, level: u8) {
    for i in 0..4u8 {
        let h = 4. + f32::from(i) * 3.;
        let x = right - 22. + f32::from(i) * 6.;
        let lit = i < level;
        c.rrect(
            x,
            cy + 7. - h,
            4.,
            h,
            1.,
            if lit { color::TEXT } else { color::TRACK },
        );
    }
}

/// A rounded pill button.
#[allow(clippy::too_many_arguments)]
pub fn pill(
    c: &mut Canvas,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    label: &str,
    face: Face,
    fill: u32,
    ink: u32,
) {
    c.rrect(x, y, w, h, h / 2., fill);
    c.text_center(x + w / 2., y + h / 2. + face.mid_offset(), label, face, ink);
}

/// A thin outlined tag such as `LOSSLESS 24/96`.
pub fn tag(c: &mut Canvas, x: f32, cy: f32, label: &str, tint: u32) -> f32 {
    let w = text::width_tracked(Face::S10, label, 0.5) + 20.;
    c.rrect_outline(x, cy - 10., w, 20., 10., 1., alpha(tint, 170));
    c.text_tracked(
        x + 10.,
        cy + Face::S10.mid_offset(),
        label,
        Face::S10,
        tint,
        0.5,
    );
    w
}

/// A thin horizontal track with its lit part and an optional knob.
pub fn slider(c: &mut Canvas, x: f32, cy: f32, w: f32, value: f32, lit: u32, knob: bool) {
    let h = 4.;
    c.rrect(x, cy - h / 2., w, h, 2., color::TRACK);
    let filled = (w * value.clamp(0., 1.)).round();
    c.rrect(x, cy - h / 2., filled, h, 2., lit);
    if knob {
        c.disc(x + filled, cy, 6., color::TEXT);
    }
}

// ------------------------------------------------------------------- covers

/// Where the cover of a track comes from for the current frame.
pub fn cover(c: &mut Canvas, ui: &Ui, track: Option<(usize, &Track)>, x: f32, y: f32, size: f32) {
    cover_rounded(c, ui, track, x, y, size, (size * 0.11).max(4.));
}

pub fn cover_rounded(
    c: &mut Canvas,
    ui: &Ui,
    track: Option<(usize, &Track)>,
    x: f32,
    y: f32,
    size: f32,
    radius: f32,
) {
    let Some((index, t)) = track else {
        placeholder(c, x, y, size, radius, 0);
        return;
    };
    let key = art_key(t);
    if ui.thumb_resident(key) {
        c.draw.push(reborn_graphics::Quad {
            tex: reborn_graphics::Tex::Thumb(key),
            ..reborn_graphics::Quad::rect(x, y, size, size, color::TEXT)
        });
    } else {
        placeholder(c, x, y, size, radius, key);
        ui.want_thumb(key, index);
    }
}

/// The calm tint of a missing or loading cover with identity `key`.
pub fn placeholder_color(key: u64) -> u32 {
    let hue = [
        0x3A4560u32,
        0x4A3A58,
        0x2F5058,
        0x58463A,
        0x3A5846,
        0x50405A,
    ];
    mix(
        hue[(key >> 8) as usize % hue.len()] << 8 | 0xFF,
        color::BG,
        0.35,
    )
}

/// A calm gradient tile standing in for a cover that is missing or loading.
pub fn placeholder(c: &mut Canvas, x: f32, y: f32, size: f32, radius: f32, key: u64) {
    c.rrect(x, y, size, size, radius, placeholder_color(key));
    if size >= 40. {
        c.icon(
            Icon::Note,
            x + size / 2.,
            y + size / 2.,
            (size * 0.34).round(),
            fade(color::TEXT, 0.35),
        );
    }
}

/// A soft shadow under a raised cover.
pub fn cover_shadow(c: &mut Canvas, x: f32, y: f32, size: f32) {
    c.glow(
        x + size / 2.,
        y + size * 0.62,
        size * 0.78,
        size * 0.72,
        0x00000090,
    );
}

// ---------------------------------------------------------- sheets and toasts

/// A dimmed backdrop for a modal sheet.
pub fn scrim(c: &mut Canvas, strong: bool) {
    c.rect(
        0.,
        0.,
        WIDTH,
        HEIGHT,
        if strong {
            color::SCRIM_STRONG
        } else {
            color::SCRIM
        },
    );
}

pub struct SheetRow<'a> {
    pub label: &'a str,
    pub detail: &'a str,
    pub enabled: bool,
}

/// A bottom sheet of actions, as used for song options. The focused row is a
/// white pill with dark text, so focus never depends on colour.
pub fn action_sheet(c: &mut Canvas, title: &str, subtitle: &str, rows: &[SheetRow], focus: usize) {
    scrim(c, false);
    let row_h = 31.;
    let head = if subtitle.is_empty() { 40. } else { 52. };
    let visible = rows.len().min(6);
    let h = head + visible as f32 * row_h + 14.;
    let (x, w) = (8., WIDTH - 16.);
    let y = HEIGHT - h - 8.;
    c.rrect(x, y, w, h, 18., color::SHEET);
    c.rrect_outline(x, y, w, h, 18., 1., color::GLASS);
    c.text_mid(x + 20., y + 22., w - 40., title, Face::S15, color::TEXT);
    if !subtitle.is_empty() {
        c.text_mid(x + 20., y + 40., w - 40., subtitle, Face::R11, color::MUTED);
    }
    let start = focus.saturating_sub(visible.saturating_sub(1));
    for (i, row) in rows.iter().enumerate().skip(start).take(visible) {
        let ry = y + head + (i - start) as f32 * row_h;
        let selected = i == focus && row.enabled;
        if selected {
            let at = c.draw.len();
            c.rrect(x + 8., ry, w - 16., row_h - 3., 10., 0xF2F3F5FF);
            c.mark_focus(at);
        }
        let ink = if selected {
            color::INK
        } else if row.enabled {
            color::TEXT
        } else {
            color::DIM
        };
        c.text_mid(
            x + 22.,
            ry + (row_h - 3.) / 2.,
            w - 150.,
            row.label,
            Face::R15,
            ink,
        );
        if !row.detail.is_empty() {
            c.text_mid_right(
                x + w - 22.,
                ry + (row_h - 3.) / 2.,
                row.detail,
                Face::R11,
                if selected { 0x4A4E58FF } else { color::MUTED },
            );
        }
    }
}

/// A centred dialog with a message and a short row of choices.
pub fn dialog(
    c: &mut Canvas,
    title: &str,
    body: &str,
    rows: &[SheetRow],
    focus: usize,
    danger: bool,
) {
    scrim(c, true);
    let lines = text::wrap(Face::R13, body, 300., 4);
    let h = 74. + lines.len() as f32 * 19. + rows.len() as f32 * 38. + 10.;
    let (w, x) = (340., 70.);
    let y = ((HEIGHT - h) / 2.).max(12.);
    c.rrect(x, y, w, h, 18., color::SHEET);
    c.rrect_outline(x, y, w, h, 18., 1., color::GLASS);
    c.text_center(WIDTH / 2., y + 34., title, Face::S17, color::TEXT);
    let mut ty = y + 62.;
    for line in &lines {
        c.text_center(WIDTH / 2., ty, line, Face::R13, color::MUTED);
        ty += 19.;
    }
    let mut ry = ty + 10.;
    for (i, row) in rows.iter().enumerate() {
        let selected = i == focus && row.enabled;
        if selected {
            let at = c.draw.len();
            let fill = if danger && i > 0 {
                color::DANGER
            } else {
                0xF2F3F5FF
            };
            c.rrect(x + 24., ry, w - 48., 32., 16., fill);
            c.mark_focus(at);
        } else {
            c.rrect(x + 24., ry, w - 48., 32., 16., color::GLASS_FAINT);
        }
        let ink = if selected {
            color::INK
        } else if row.enabled {
            color::TEXT
        } else {
            color::DIM
        };
        c.text_center(
            WIDTH / 2.,
            ry + 16. + Face::S13.mid_offset(),
            row.label,
            Face::S13,
            ink,
        );
        ry += 38.;
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Info,
    Error,
}

/// A short message at the top of the screen.
pub fn toast(c: &mut Canvas, message: &str, tone: Tone) {
    // In the status bar, over the screen's title: a notice never hides the
    // focused row, the cover or a text being typed.
    let w = (text::width(Face::S13, message) + 52.).min(WIDTH - 24.);
    let x = (WIDTH - w) / 2.;
    let y = 2.;
    c.rrect(x, y, w, 34., 17., 0x14161CFC);
    c.rrect_outline(x, y, w, 34., 17., 1., color::GLASS);
    let dot = match tone {
        Tone::Info => color::MUTED,
        Tone::Error => color::DANGER,
    };
    c.disc(x + 18., y + 17., 4., dot);
    c.text_mid(x + 32., y + 17., w - 46., message, Face::S13, color::TEXT);
}

/// A thin position rail beside a long list.
pub fn scroll_rail(
    c: &mut Canvas,
    top: f32,
    height: f32,
    position: usize,
    count: usize,
    visible: usize,
    accent: u32,
) {
    if count <= visible {
        return;
    }
    let thumb = (height * visible as f32 / count as f32).clamp(18., height);
    let y = top + (height - thumb) * position as f32 / (count - visible).max(1) as f32;
    c.rrect(WIDTH - 14., top, 2., height, 1., color::TRACK);
    c.rrect(WIDTH - 14., y.round(), 2., thumb.round(), 1., accent);
}

/// The red of a live broadcast (LIVE and ON AIR badges).
pub const LIVE_RED: u32 = 0xFF4B5CFF;

/// A station's logo, or a tile with its initials until (or unless) a logo
/// arrives. Logos are asked for here; the player fetches and decodes them.
pub fn station_logo(
    c: &mut Canvas,
    ui: &Ui,
    station: &reborn_core::RadioStation,
    x: f32,
    y: f32,
    size: f32,
) {
    if let Some(key) = station.logo_key() {
        if ui.thumb_resident(key) {
            c.draw.push(reborn_graphics::Quad {
                tex: reborn_graphics::Tex::Thumb(key),
                ..reborn_graphics::Quad::rect(x, y, size, size, color::TEXT)
            });
            return;
        }
        ui.want_logo(key, &station.favicon);
    }
    initials_tile(c, &station.name, x, y, size);
}

/// A station without a logo: its initials on a tile tinted from its name.
pub fn initials_tile(c: &mut Canvas, name: &str, x: f32, y: f32, size: f32) {
    let radius = (size * 0.11).max(4.);
    let key = reborn_core::logo_key(name);
    let tints = [
        0x3B5BDBu32,
        0xC2255C,
        0x0B7285,
        0x5F3DC4,
        0xE8590C,
        0x2B8A3E,
        0x1864AB,
        0xA61E4D,
    ];
    let tint = tints[(key >> 5) as usize % tints.len()] << 8 | 0xFF;
    c.rrect(x, y, size, size, radius, mix(tint, color::BG, 0.45));
    // A soft light from the top left gives the tile some depth.
    c.glow(
        x + size * 0.3,
        y + size * 0.25,
        size * 0.7,
        size * 0.6,
        alpha(tint, 0x60),
    );
    let initials: String = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !["the", "radio"].contains(&w.to_lowercase().as_str()))
        .filter_map(|w| w.chars().next())
        .take(2)
        .flat_map(char::to_uppercase)
        .collect();
    let initials = if initials.is_empty() {
        name.chars().take(1).flat_map(char::to_uppercase).collect()
    } else {
        initials
    };
    let face = if size >= 120. && initials.chars().all(|c| c.is_ascii_alphanumeric()) {
        Face::S36
    } else if size >= 60. {
        Face::S25
    } else if size >= 36. {
        Face::S15
    } else {
        Face::S13
    };
    c.text_mid_center(
        x + size / 2.,
        y + size / 2.,
        &initials,
        face,
        fade(color::TEXT, 0.92),
    );
}

/// A red LIVE badge with its dot; returns its width.
pub fn live_pill(c: &mut Canvas, x: f32, cy: f32, label: &str) -> f32 {
    let w = text::width_tracked(Face::S10, label, 1.2) + 26.;
    c.rrect(x, cy - 9., w, 18., 9., LIVE_RED);
    c.disc(x + 9.5, cy, 3., color::TEXT);
    c.text_tracked(
        x + 17.,
        cy + Face::S10.mid_offset(),
        label,
        Face::S10,
        color::TEXT,
        1.2,
    );
    w
}

/// "1 song", "12 songs", "5 124 stations": a count with its noun.
pub fn count(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{} {many}", crate::screens::count_label(n))
    }
}

/// The first row shown so the focused one is in view among `visible`.
pub fn list_window(m: &AppModel, focus: usize, visible: usize) -> usize {
    m.navigation
        .scroll
        .min(focus)
        .max((focus + 1).saturating_sub(visible))
}

/// A rounded square tinted with `tint`, `icon` in the middle in that colour
/// (Radio's categories, Playlists' New, Favorites and empty mosaics).
#[allow(clippy::too_many_arguments)]
pub fn icon_tile(
    c: &mut Canvas,
    x: f32,
    y: f32,
    size: f32,
    radius: f32,
    tint: u32,
    icon: Icon,
    icon_size: f32,
) {
    c.rrect(x, y, size, size, radius, alpha(tint, 0x30));
    c.icon(icon, x + size / 2., y + size / 2., icon_size, tint);
}
