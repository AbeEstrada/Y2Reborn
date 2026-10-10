//! Settings and its pages: grouped cards, switches, pickers, the radios and
//! the password entry.
use crate::{
    canvas::{Canvas, Icon},
    components::{self, signal_bars},
    pages::{self, Page},
    screens::{screen_title, wrap_lines},
    text::{self, Face},
    theme::{alpha, color, HEIGHT, WIDTH},
    Item, RowKind, Ui, WifiStatus,
};
use reborn_core::{
    platform::{SdCard, UpdatePhase},
    AppModel, Screen, Track,
};

pub fn screen(c: &mut Canvas, ui: &Ui, m: &AppModel, tracks: &[Track], interactive: bool) {
    let page = pages::page(ui, m, tracks);
    components::status_bar(c, ui, m, &screen_title(m).to_uppercase(), true);
    let focus = m.navigation.focus.min(page.rows.len().saturating_sub(1));
    match m.screen {
        Screen::Settings => root(c, ui, &page, focus, interactive),
        Screen::Wifi => radio(c, ui, m, &page, focus, interactive, Radio::Wifi),
        Screen::Bluetooth => radio(c, ui, m, &page, focus, interactive, Radio::Bluetooth),
        Screen::SettingsAudio => sound(c, ui, m, &page, focus, interactive),
        Screen::Equalizer if m.navigation.filter.is_empty() => {
            equalizer(c, ui, m, &page, focus, interactive)
        }
        Screen::Equalizer => group_page(c, ui, &page, focus, interactive, 38.),
        Screen::Crossfade | Screen::ScreenTimeout => picker(c, ui, &page, focus, interactive),
        Screen::SettingsDisplay => display(c, ui, m, &page, focus, interactive),
        Screen::Storage => storage(c, ui, m, &page, focus, interactive),
        Screen::Update => update(c, ui, m, &page, focus, interactive),
        _ => group_page(c, ui, &page, focus, interactive, 38.),
    }
}

// ------------------------------------------------------------------ root

fn root(c: &mut Canvas, ui: &Ui, page: &Page, focus: usize, interactive: bool) {
    let _ = ui;
    let tiles: [(u32, Icon); 6] = [
        (0x3E82FFFF, Icon::Bluetooth),
        (0x3CBE78FF, Icon::Wifi),
        (0xFF6E5AFF, Icon::Speaker),
        (0xFFBE3CFF, Icon::Sun),
        (0x9678FFFF, Icon::Drive),
        (0x828CA0FF, Icon::Info),
    ];
    components::card(c, 10., 38., WIDTH - 20., 6. * 52. + 4.);
    for (i, row) in page.rows.iter().enumerate().take(6) {
        let y = 40. + i as f32 * 52.;
        let selected = interactive && i == focus;
        if selected {
            components::glass_row(c, 14., y + 3., WIDTH - 28., 46.);
        } else if i > 0 && !(interactive && i - 1 == focus) {
            c.rect(68., y + 1., WIDTH - 92., 1., color::RULE);
        }
        let (tint, icon) = tiles[i];
        c.rrect(24., y + 11., 30., 30., 8., tint);
        c.icon(icon, 39., y + 26., 18., color::TEXT);
        let face = if selected { Face::S17 } else { Face::R17 };
        c.text_mid(68., y + 26., 230., &row.label, face, color::TEXT);
        c.text_mid_right(
            WIDTH - 42.,
            y + 26.,
            &text::fit(Face::R11, &row.secondary, 150.),
            Face::R11,
            color::MUTED,
        );
        components::chevron(c, WIDTH - 22., y + 26.);
    }
}

// ----------------------------------------------------------- grouped rows

/// Draw `rows` as one grouped card starting at `top`; returns its bottom edge.
/// `first` is the index of the first row in the page, for focus.
fn group(
    c: &mut Canvas,
    ui: &Ui,
    rows: &[Item],
    first: usize,
    focus: usize,
    interactive: bool,
    top: f32,
) -> f32 {
    let accent = ui.accent();
    let h = 50.;
    components::card(c, 10., top, WIDTH - 20., rows.len() as f32 * h + 4.);
    for (i, row) in rows.iter().enumerate() {
        let y = top + 2. + i as f32 * h;
        let selected = interactive && first + i == focus && row.enabled;
        if selected {
            components::glass_row(c, 14., y + 2., WIDTH - 28., h - 4.);
        } else if i + 1 < rows.len() && !(interactive && first + i + 1 == focus) {
            c.rect(28., y + h - 1., WIDTH - 52., 1., color::RULE);
        }
        let face = if selected { Face::S15 } else { Face::R15 };
        let tint = if row.enabled { color::TEXT } else { color::DIM };
        let cy = y + h / 2.;
        match row.kind {
            RowKind::Toggle(on) => {
                c.text_mid(28., cy, 300., &row.label, face, tint);
                components::toggle(c, WIDTH - 24., cy, on);
            }
            RowKind::Check(on) => {
                c.text_mid(28., cy, 340., &row.label, face, tint);
                if on {
                    components::check(c, WIDTH - 32., cy, accent);
                }
            }
            RowKind::Chevron | RowKind::Plain => {
                let value = text::fit(Face::R11, &row.secondary, 170.);
                let right = if row.kind == RowKind::Chevron {
                    WIDTH - 40.
                } else {
                    WIDTH - 28.
                };
                let reserve = text::width(Face::R11, &value) + 16.;
                c.text_mid(28., cy, right - 28. - reserve, &row.label, face, tint);
                c.text_mid_right(right, cy, &value, Face::R11, color::MUTED);
                if row.kind == RowKind::Chevron {
                    components::chevron(c, WIDTH - 22., cy);
                }
            }
        }
    }
    top + rows.len() as f32 * h + 4.
}

fn hint(c: &mut Canvas, page: &Page) {
    if let Some(status) = &page.status {
        let lines = wrap_lines(status, 420., Face::R11, 2);
        let mut y = HEIGHT - 12. - 15. * lines.len() as f32;
        for line in lines {
            c.text_center(
                240.,
                y + Face::R11.mid_offset(),
                &line,
                Face::R11,
                color::DIM,
            );
            y += 15.;
        }
    }
}

/// A plain page: optional message, then grouped rows.
pub(crate) fn group_page(
    c: &mut Canvas,
    ui: &Ui,
    page: &Page,
    focus: usize,
    interactive: bool,
    top: f32,
) {
    let mut y = top;
    if let Some(h) = &page.hero {
        y = message(c, &h.title, &h.body, y + 6.) + 10.;
    }
    for (label, value) in &page.facts {
        c.text_mid(28., y + 12., 150., label, Face::R13, color::MUTED);
        c.text_mid_right(
            WIDTH - 28.,
            y + 12.,
            &text::fit(Face::R13, value, 270.),
            Face::R13,
            color::TEXT,
        );
        y += 30.;
    }
    if !page.rows.is_empty() {
        let visible = (((HEIGHT - 40. - y) / 50.).floor() as usize).max(1);
        let start = focus
            .saturating_sub(visible - 1)
            .min(page.rows.len().saturating_sub(visible));
        let end = (start + visible).min(page.rows.len());
        group(c, ui, &page.rows[start..end], start, focus, interactive, y);
    }
    hint(c, page);
}

/// A headline and explanation, left-aligned; returns the y below them.
pub fn message(c: &mut Canvas, title: &str, body: &str, y: f32) -> f32 {
    let mut cy = y + 14.;
    for line in text::wrap(Face::S20, title, 432., 2) {
        c.text(
            24.,
            cy + Face::S20.mid_offset(),
            &line,
            Face::S20,
            color::TEXT,
        );
        cy += 26.;
    }
    if !body.is_empty() {
        cy += 2.;
        for line in text::wrap(Face::R13, body, 432., 3) {
            c.text(
                24.,
                cy + Face::R13.mid_offset(),
                &line,
                Face::R13,
                color::MUTED,
            );
            cy += 19.;
        }
    }
    cy
}

// ----------------------------------------------------------------- radios

#[derive(Clone, Copy, PartialEq, Eq)]
enum Radio {
    Wifi,
    Bluetooth,
}

fn radio(
    c: &mut Canvas,
    ui: &Ui,
    m: &AppModel,
    page: &Page,
    focus: usize,
    interactive: bool,
    kind: Radio,
) {
    let accent = ui.accent();
    let Some(switch) = page
        .rows
        .first()
        .filter(|r| matches!(r.kind, RowKind::Toggle(_)))
    else {
        // Not available yet: a message only.
        group_page(c, ui, page, focus, interactive, 38.);
        return;
    };
    group(
        c,
        ui,
        std::slice::from_ref(switch),
        0,
        focus,
        interactive,
        38.,
    );
    if let Some(h) = page.hero.as_ref() {
        // Off: a calm empty state under the switch.
        let icon = if kind == Radio::Wifi {
            Icon::Wifi
        } else {
            Icon::Bluetooth
        };
        c.ring(240., 170., 30., 1.5, color::GLASS_STRONG);
        c.icon(icon, 240., 170., 26., color::DIM);
        c.text_center(
            240.,
            226. + Face::S20.mid_offset(),
            &h.title,
            Face::S20,
            color::TEXT,
        );
        for (i, line) in text::wrap(Face::R13, &h.body, 330., 2).iter().enumerate() {
            c.text_center(
                240.,
                254. + i as f32 * 19. + Face::R13.mid_offset(),
                line,
                Face::R13,
                color::MUTED,
            );
        }
        return;
    }
    let rest: Vec<(usize, &Item)> = page.rows.iter().enumerate().skip(1).collect();
    let visible = 4usize;
    let start = focus
        .saturating_sub(visible)
        .min(rest.len().saturating_sub(visible));
    for (slot, (index, row)) in rest.iter().skip(start).take(visible).enumerate() {
        let y = 100. + slot as f32 * 56.;
        let selected = interactive && *index == focus && row.enabled;
        if selected {
            components::glass_row(c, 10., y, WIDTH - 20., 52.);
        }
        let connected = row.secondary.starts_with("Connected");
        let is_action = row.key == "wifi_scan" || row.key == "bt_scan";
        let face = if selected { Face::S15 } else { Face::R15 };
        if is_action {
            c.text_mid(
                28.,
                y + 26.,
                380.,
                &row.label,
                face,
                if row.enabled {
                    color::TEXT
                } else {
                    color::MUTED
                },
            );
            continue;
        }
        c.text_mid(
            28.,
            y + 19.,
            300.,
            &row.label,
            face,
            if row.enabled { color::TEXT } else { color::DIM },
        );
        let sub_color = if connected {
            color::SUCCESS
        } else {
            color::MUTED
        };
        c.text_mid(28., y + 38., 330., &row.secondary, Face::R11, sub_color);
        if kind == Radio::Wifi {
            if let Some(n) = ui.wifi.networks.iter().find(|n| n.ssid == row.label) {
                signal_bars(c, WIDTH - 24., y + 26., n.bars.min(4));
                if n.secured == Some(true) {
                    c.icon(Icon::Lock, WIDTH - 56., y + 26., 14., color::MUTED);
                }
            }
        } else if connected {
            c.icon(Icon::Check, WIDTH - 34., y + 26., 18., accent);
        }
    }
    let _ = (m, WifiStatus::Disconnected);
    if let Some(status) = &page.status {
        let lines = wrap_lines(status, 430., Face::R11, 2);
        for (i, line) in lines.iter().enumerate() {
            c.text_center(
                240.,
                332. + i as f32 * 15. + Face::R11.mid_offset(),
                line,
                Face::R11,
                color::DIM,
            );
        }
    } else if kind == Radio::Bluetooth {
        c.text_center(
            240.,
            340. + Face::R11.mid_offset(),
            "Select to connect  ·  Hold to forget",
            Face::R11,
            color::DIM,
        );
    }
}

// -------------------------------------------------------------- sound etc.

fn sound(c: &mut Canvas, ui: &Ui, m: &AppModel, page: &Page, focus: usize, interactive: bool) {
    let bottom = group(
        c,
        ui,
        &page.rows[..4.min(page.rows.len())],
        0,
        focus,
        interactive,
        38.,
    );
    components::section_label(c, 28., bottom + 24., "Output");
    if let Some(row) = page.rows.get(4) {
        let y = bottom + 38.;
        if interactive && focus == 4 {
            components::glass_row(c, 10., y, WIDTH - 20., 56.);
        } else {
            components::card(c, 10., y, WIDTH - 20., 56.);
        }
        c.text_mid(28., y + 21., 340., &row.secondary, Face::S15, color::TEXT);
        let detail = match &m.output {
            reborn_core::AudioOutput::Wired => "Headphone jack".to_owned(),
            reborn_core::AudioOutput::Bluetooth(address) => ui
                .bluetooth
                .devices
                .iter()
                .find(|d| &d.address == address)
                .and_then(|d| d.codec.clone())
                .map(|codec| format!("{codec}  ·  Bluetooth"))
                .unwrap_or_else(|| "Bluetooth".into()),
        };
        c.text_mid(28., y + 40., 340., &detail, Face::R11, color::MUTED);
        components::chevron(c, WIDTH - 22., y + 28.);
    }
}

fn picker(c: &mut Canvas, ui: &Ui, page: &Page, focus: usize, interactive: bool) {
    let accent = ui.accent();
    let pitch = if page.rows.len() > 5 { 43. } else { 47. };
    for (i, row) in page.rows.iter().enumerate() {
        let y = 38. + i as f32 * pitch;
        let selected = interactive && i == focus;
        if selected {
            components::glass_row(c, 10., y, WIDTH - 20., pitch - 4.);
        }
        let face = if selected { Face::S15 } else { Face::R15 };
        c.text_mid(
            26.,
            y + (pitch - 4.) / 2.,
            340.,
            &row.label,
            face,
            color::TEXT,
        );
        if row.kind == RowKind::Check(true) {
            components::check(c, WIDTH - 32., y + (pitch - 4.) / 2., accent);
        }
    }
    hint(c, page);
}

fn equalizer(c: &mut Canvas, ui: &Ui, m: &AppModel, page: &Page, focus: usize, interactive: bool) {
    let accent = ui.accent();
    for (i, row) in page.rows.iter().enumerate() {
        let y = 38. + i as f32 * 44.;
        let selected = interactive && i == focus;
        if selected {
            components::glass_row(c, 10., y, WIDTH - 20., 40.);
        }
        let face = if selected { Face::S15 } else { Face::R15 };
        c.text_mid(26., y + 20., 330., &row.label, face, color::TEXT);
        match row.kind {
            RowKind::Check(true) => components::check(c, WIDTH - 32., y + 20., accent),
            RowKind::Chevron => components::chevron(c, WIDTH - 22., y + 20.),
            _ => {}
        }
    }
    // A live curve of the focused preset (or the current settings).
    let focused = page.rows.get(focus).map(|r| r.key.as_str()).unwrap_or("");
    let gains = pages::EQ_PRESETS
        .iter()
        .find(|(_, key, _)| focused == format!("eq_preset:{key}"))
        .map(|(_, _, g)| *g)
        .unwrap_or_else(|| pages::eq_gains(m));
    components::card(c, 10., 266., WIDTH - 20., 76.);
    let xs: Vec<f32> = (0..5)
        .map(|i| 34. + i as f32 * ((WIDTH - 68.) / 4.))
        .collect();
    let ys: Vec<f32> = gains.iter().map(|g| 304. - f32::from(*g) * 2.6).collect();
    for x in &xs {
        c.rect(x.round(), 276., 1., 56., color::RULE);
    }
    for i in 0..4 {
        line(c, xs[i], ys[i], xs[i + 1], ys[i + 1], accent);
    }
    for (x, y) in xs.iter().zip(&ys) {
        c.disc(*x, *y, 4., color::TEXT);
    }
}

/// A 2 px line between two points, as short axis-aligned steps.
fn line(c: &mut Canvas, x0: f32, y0: f32, x1: f32, y1: f32, tint: u32) {
    let steps = ((x1 - x0).abs()).ceil() as usize;
    for s in 0..steps {
        let t = s as f32 / steps as f32;
        let (x, y) = (x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
        c.rect(x.round(), (y - 1.).round(), 2., 2.5, tint);
    }
}

fn display(c: &mut Canvas, ui: &Ui, m: &AppModel, page: &Page, focus: usize, interactive: bool) {
    let mut index = 0;
    let mut y = 38.;
    if m.platform.brightness_available {
        components::card(c, 10., y, WIDTH - 20., 78.);
        if interactive && focus == 0 {
            components::glass_row(c, 14., y + 4., WIDTH - 28., 70.);
        }
        c.text_mid(28., y + 24., 250., "Brightness", Face::S15, color::TEXT);
        c.text_mid_right(
            WIDTH - 28.,
            y + 24.,
            &format!("{}%", m.settings.brightness),
            Face::R11,
            color::MUTED,
        );
        components::slider(
            c,
            28.,
            y + 54.,
            WIDTH - 56.,
            f32::from(m.settings.brightness) / 100.,
            color::TEXT,
            true,
        );
        index = 1;
        y += 92.;
    }
    if page.rows.len() > index {
        group(c, ui, &page.rows[index..], index, focus, interactive, y);
    }
}

fn storage(c: &mut Canvas, ui: &Ui, m: &AppModel, page: &Page, focus: usize, interactive: bool) {
    let snapshot = &m.platform.snapshot;
    let volumes = [
        (
            "Internal Storage",
            Some(&snapshot.storage.internal),
            0x6EA0FFFFu32,
        ),
        (
            "SD Card",
            match &snapshot.storage.sd {
                SdCard::Ready(space) => Some(space),
                _ => None,
            },
            0x3CC878FF,
        ),
    ];
    for (i, (name, space, tint)) in volumes.into_iter().enumerate() {
        let y = 40. + i as f32 * 84.;
        components::card(c, 10., y, WIDTH - 20., 76.);
        c.text_mid(28., y + 22., 250., name, Face::S15, color::TEXT);
        match space {
            Some(space) => {
                let free = space
                    .free_bytes
                    .map(|f| format!("{} free", reborn_core::platform::human_bytes(f)))
                    .unwrap_or_default();
                c.text_mid_right(WIDTH - 28., y + 22., &free, Face::R11, color::MUTED);
                let used = match (space.free_bytes, space.total_bytes) {
                    (Some(f), Some(t)) if t > 0 => 1. - f as f32 / t as f32,
                    _ => 0.,
                };
                c.rrect(28., y + 42., WIDTH - 56., 8., 4., color::GLASS_STRONG);
                c.rrect(
                    28.,
                    y + 42.,
                    ((WIDTH - 56.) * used.clamp(0., 1.)).max(8.),
                    8.,
                    4.,
                    tint,
                );
                if let Some(t) = space.total_bytes {
                    c.text_mid(
                        28.,
                        y + 62.,
                        250.,
                        &format!("{} total", reborn_core::platform::human_bytes(t)),
                        Face::R11,
                        color::MUTED,
                    );
                }
            }
            None => {
                let note = if matches!(snapshot.storage.sd, SdCard::Error) && i == 1 {
                    "Card can't be read"
                } else if i == 1 {
                    "No SD card"
                } else {
                    "Checking…"
                };
                c.text_mid(28., y + 50., 300., note, Face::R13, color::MUTED);
            }
        }
    }
    let songs = ui.library_counts(&m.library.tracks)[2];
    let top = 40. + 2. * 84.;
    components::card(
        c,
        10.,
        top,
        WIDTH - 20.,
        44. * (page.rows.len() as f32 + 1.) + 4.,
    );
    c.text_mid(
        28.,
        top + 24.,
        250.,
        "Songs in library",
        Face::R15,
        color::TEXT,
    );
    c.text_mid_right(
        WIDTH - 28.,
        top + 24.,
        &crate::screens::count_label(songs),
        Face::R11,
        color::MUTED,
    );
    for (i, row) in page.rows.iter().enumerate() {
        let y = top + 2. + 44. + i as f32 * 44.;
        if interactive && i == focus && row.enabled {
            components::glass_row(c, 14., y + 1., WIDTH - 28., 42.);
        }
        let face = if interactive && i == focus {
            Face::S15
        } else {
            Face::R15
        };
        c.text_mid(
            28.,
            y + 22.,
            280.,
            &row.label,
            face,
            if row.enabled { color::TEXT } else { color::DIM },
        );
        let value = text::fit(Face::R11, &row.secondary, 190.);
        c.text_mid_right(WIDTH - 40., y + 22., &value, Face::R11, color::MUTED);
        components::chevron(c, WIDTH - 22., y + 22.);
    }
    let _ = HEIGHT;
}

fn update(c: &mut Canvas, ui: &Ui, m: &AppModel, page: &Page, focus: usize, interactive: bool) {
    let accent = ui.accent();
    let phase = &m.platform.snapshot.update.phase;
    let (tint, icon) = match phase {
        UpdatePhase::UpToDate => (color::SUCCESS, Icon::Check),
        UpdatePhase::Available { .. } | UpdatePhase::ReadyToInstall | UpdatePhase::Downloading => {
            (0x6EA0FFFF, Icon::Download)
        }
        UpdatePhase::Failed(_) => (color::WARNING, Icon::Warning),
        _ => (color::MUTED, Icon::Info),
    };
    let (title, body) = page
        .hero
        .as_ref()
        .map(|h| (h.title.clone(), h.body.clone()))
        .unwrap_or_else(|| ("Software".into(), String::new()));
    c.rrect(10., 38., WIDTH - 20., 86., 16., alpha(tint, 34));
    c.rrect_outline(10., 38., WIDTH - 20., 86., 16., 1., alpha(tint, 110));
    c.disc(46., 81., 18., tint);
    c.icon(icon, 46., 81., 20., color::INK);
    c.text_mid(76., 62., 360., &title, Face::S17, color::TEXT);
    for (i, line) in text::wrap(Face::R11, &body, 360., 3).iter().enumerate() {
        c.text_mid(
            76.,
            84. + i as f32 * 15.,
            360.,
            line,
            Face::R11,
            color::MUTED,
        );
    }
    let _ = accent;
    let start_rows = 138.;
    let visible = (((HEIGHT - 10. - start_rows) / 50.).floor() as usize).max(1);
    let first = focus
        .saturating_sub(visible - 1)
        .min(page.rows.len().saturating_sub(visible));
    let end = (first + visible).min(page.rows.len());
    group(
        c,
        ui,
        &page.rows[first..end],
        first,
        focus,
        interactive,
        start_rows,
    );
}

// ------------------------------------------------------- password entry

/// The character wheel for a Wi-Fi password.
pub fn password_entry(c: &mut Canvas, ui: &Ui, m: &AppModel) {
    let accent = ui.accent();
    c.rect(0., 0., WIDTH, HEIGHT, color::BG);
    components::status_bar(c, ui, m, &ui.ssid.to_uppercase(), true);
    c.text_center(
        240.,
        66. + Face::R13.mid_offset(),
        "Password",
        Face::R13,
        color::MUTED,
    );
    c.rrect(48., 82., WIDTH - 96., 44., 14., color::GLASS);
    let dots = "•".repeat(ui.password.len().min(24));
    let w = text::width(Face::S20, &dots);
    c.text(
        66.,
        104. + Face::S20.mid_offset(),
        &dots,
        Face::S20,
        color::TEXT,
    );
    c.rect((66. + w + 3.).round(), 92., 2., 24., accent);
    // One row of characters with the focused one lifted.
    let count = crate::LETTERS.len() + 3;
    let label = |index: usize| -> String {
        if index < crate::LETTERS.len() {
            if crate::LETTERS[index] == b' ' {
                "_".into()
            } else {
                (crate::LETTERS[index] as char).to_string()
            }
        } else {
            ["Del", "Go", "Esc"][index - crate::LETTERS.len()].into()
        }
    };
    for offset in -3i32..=3 {
        let index = (ui.letter as i32 + offset).rem_euclid(count as i32) as usize;
        let cx = 240. + offset as f32 * 52.;
        if offset == 0 {
            let at = c.draw.len();
            c.rrect(cx - 26., 158., 52., 76., 16., 0xF2F3F5FF);
            c.mark_focus(at);
            let space = ui.letter < crate::LETTERS.len() && crate::LETTERS[ui.letter] == b' ';
            let words = ui.letter >= crate::LETTERS.len() || space;
            let text = if space {
                "Space".to_owned()
            } else if words {
                ["Delete", "Connect", "Cancel"][ui.letter - crate::LETTERS.len()].to_owned()
            } else {
                label(index)
            };
            let face = if words { Face::S13 } else { Face::S36 };
            c.text_center(cx, 196. + face.mid_offset(), &text, face, color::INK);
        } else {
            let tint = if offset.abs() < 3 {
                0xAAAEBAFF
            } else {
                0x6E727EFF
            };
            let face = if offset.abs() == 1 {
                Face::L23
            } else {
                Face::R17
            };
            c.text_center(cx, 196. + face.mid_offset(), &label(index), face, tint);
        }
    }
    c.text_center(
        240.,
        296. + Face::R11.mid_offset(),
        "Turn wheel: letter  ·  Select: add  ·  Hold Select: done",
        Face::R11,
        color::DIM,
    );
    c.text_center(
        240.,
        320. + Face::R11.mid_offset(),
        "Back cancels",
        Face::R11,
        color::DIM,
    );
}
