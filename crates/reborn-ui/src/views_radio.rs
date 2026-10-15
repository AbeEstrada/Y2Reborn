//! Radio: its own section with station logos, an On Air card, coloured
//! category tiles, station details and a LIVE badge on what is playing.
//! Loading, error and empty states are ordinary pages (`views_system`).
use crate::{
    canvas::{Canvas, Icon},
    components::{self, initials_tile, live_pill, station_logo, LIVE_RED},
    pages,
    screens::{count_label, screen_title},
    text::{self, Face},
    theme::{alpha, color, mix, HEIGHT, WIDTH},
    views_system, Item, Ui,
};
use reborn_core::{AppModel, RadioStation, Screen, Track};

/// Radio's own light: a faint red glow in the top corner, "on air".
fn ambience(c: &mut Canvas) {
    c.glow(70., 10., 260., 150., alpha(LIVE_RED, 0x1C));
}

pub fn screen(c: &mut Canvas, ui: &Ui, m: &AppModel, tracks: &[Track], interactive: bool) {
    let page = pages::page(ui, m, tracks);
    // Station Info's heading is for skins; Reborn draws the station itself.
    let station_info = m.screen == Screen::RadioInfo && ui.radio.info.is_some();
    if page.hero.is_some() && !station_info {
        views_system::page(c, ui, m, tracks, interactive);
        return;
    }
    ambience(c);
    match m.screen {
        Screen::Radio => home(c, ui, m, &page.rows, interactive),
        Screen::RadioStations => stations(c, ui, m, &page.rows, interactive),
        Screen::RadioCategories => categories(c, ui, m, &page.rows, interactive),
        Screen::RadioInfo => info(c, ui, m, &page.rows, interactive),
        _ => {}
    }
}

/// The colour and icon of each Radio menu entry.
fn entry_style(key: &str) -> (u32, Icon) {
    match key {
        "radio_favorites" => (0xFFC94DFF, Icon::StarOn),
        "radio_popular" => (0xFF7A59FF, Icon::Trend),
        "radio_countries" => (0x38C9B4FF, Icon::Globe),
        "radio_genres" => (0xA07BFFFF, Icon::Note),
        "radio_search" => (0x6EA0FFFF, Icon::Search),
        "radio_own" => (0x5CCB7AFF, Icon::Folder),
        "radio_wifi" => (color::WARNING, Icon::Wifi),
        _ => (color::MUTED, Icon::Broadcast),
    }
}

const ON_AIR_H: f32 = 78.;
const ENTRY_H: f32 = 48.;

fn home(c: &mut Canvas, ui: &Ui, m: &AppModel, rows: &[Item], interactive: bool) {
    let accent = ui.accent();
    components::status_bar(c, ui, m, "RADIO", true);
    let focus = m.navigation.focus.min(rows.len().saturating_sub(1));
    // Rows have two heights; scroll so the focused one ends above the bottom.
    let heights: Vec<f32> = rows
        .iter()
        .map(|r| {
            if r.key == "radio_now" {
                ON_AIR_H
            } else {
                ENTRY_H
            }
        })
        .collect();
    let bottom = HEIGHT - 6.;
    let mut first = m.navigation.scroll.min(focus);
    while first < focus && 42. + heights[first..=focus].iter().sum::<f32>() > bottom {
        first += 1;
    }
    let mut y = 42.;
    for (i, row) in rows.iter().enumerate().skip(first) {
        let h = heights[i];
        if y + h > bottom + 1. {
            break;
        }
        let selected = interactive && i == focus;
        if row.key == "radio_now" {
            on_air(c, ui, row, y, selected, accent);
        } else {
            entry(c, row, y, selected, accent);
        }
        y += h;
    }
    let total: f32 = heights.iter().sum();
    if total > bottom - 42. {
        components::scroll_rail(c, 44., HEIGHT - 52., first, rows.len(), 5, accent);
    }
}

/// The station playing now: its logo, ON AIR and the song it announces.
fn on_air(c: &mut Canvas, ui: &Ui, row: &Item, y: f32, selected: bool, accent: u32) {
    let (x, w, h) = (12., WIDTH - 24., ON_AIR_H - 8.);
    let at = c.draw.len();
    c.rrect(
        x,
        y + 2.,
        w,
        h,
        16.,
        if selected {
            mix(color::BG, accent, 0.22)
        } else {
            0xFFFFFF14
        },
    );
    if selected {
        c.mark_focus(at);
        c.rrect_outline(x, y + 2., w, h, 16., 1., alpha(accent, 150));
    }
    if let Some(now) = &ui.radio.now {
        station_logo(c, ui, now, x + 10., y + 10., 54.);
    }
    let tx = x + 78.;
    let pill = live_pill(c, tx, y + 20., "ON AIR");
    c.text_mid(
        tx + pill + 8.,
        y + 20.,
        w - 100. - pill,
        "Now playing",
        Face::R11,
        color::MUTED,
    );
    c.text_mid(tx, y + 42., w - 92., &row.label, Face::S17, color::TEXT);
    let line = match &ui.radio.song {
        Some((title, artist)) if !artist.is_empty() => format!("{artist} – {title}"),
        Some((title, _)) => title.clone(),
        None => ui
            .radio
            .now
            .as_ref()
            .map(station_detail)
            .unwrap_or_default(),
    };
    c.text_mid(tx, y + 61., w - 92., &line, Face::R13, color::MUTED);
}

/// A menu entry: a coloured icon tile, the name and what it holds.
fn entry(c: &mut Canvas, row: &Item, y: f32, selected: bool, accent: u32) {
    let (tint, icon) = entry_style(&row.key);
    if selected {
        components::accent_row(c, 12., y + 2., WIDTH - 24., ENTRY_H - 4., accent);
    }
    let tile = 34.;
    components::icon_tile(
        c,
        22.,
        y + (ENTRY_H - tile) / 2.,
        tile,
        10.,
        tint,
        icon,
        20.,
    );
    let x = 70.;
    c.text_mid(
        x,
        y + 17.,
        300.,
        &row.label,
        if selected { Face::S15 } else { Face::R15 },
        color::TEXT,
    );
    c.text_mid(
        x,
        y + 34.,
        330.,
        &row.secondary,
        Face::R11,
        if selected { accent } else { color::MUTED },
    );
    c.icon(
        Icon::Chevron,
        WIDTH - 36.,
        y + ENTRY_H / 2.,
        16.,
        color::DIM,
    );
}

/// "Switzerland · Jazz · MP3 · 128 kbps".
pub fn station_detail(s: &RadioStation) -> String {
    let mut parts = vec![];
    if !s.country.is_empty() {
        parts.push(s.country.clone());
    }
    if let Some(genre) = s.genres(1).pop() {
        parts.push(genre);
    }
    let format = s.format();
    if !format.is_empty() {
        parts.push(format);
    }
    parts.join(" · ")
}

const STATION_H: f32 = 53.;
const STATION_ROWS: usize = 6;

fn stations(c: &mut Canvas, ui: &Ui, m: &AppModel, rows: &[Item], interactive: bool) {
    let accent = ui.accent();
    let title = screen_title(m).to_uppercase();
    components::status_bar(c, ui, m, &title, true);
    let focus = m.navigation.focus.min(rows.len().saturating_sub(1));
    let first = components::list_window(m, focus, STATION_ROWS);
    let bg_selected = mix(color::BG, accent, 0.18);
    for (i, row) in rows.iter().enumerate().skip(first).take(STATION_ROWS) {
        let y = 38. + (i - first) as f32 * STATION_H;
        let selected = interactive && i == focus;
        let station = row
            .key
            .strip_prefix("station:")
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| ui.radio.stations.get(i));
        let Some(station) = station else {
            continue;
        };
        if selected {
            components::accent_row(c, 10., y + 1., WIDTH - 24., STATION_H - 3., accent);
        }
        station_logo(c, ui, station, 20., y + 7., 38.);
        // The most listened stations carry their rank on the logo.
        if i < ui.radio.ranked {
            let rank = (i + 1).to_string();
            let w = text::width(Face::S10, &rank) + 10.;
            c.rrect(14., y + 3., w.max(17.), 17., 8.5, accent);
            c.text_mid_center(
                14. + w.max(17.) / 2.,
                y + 11.5,
                &rank,
                Face::S10,
                color::INK,
            );
        }
        let _ = bg_selected;
        let playing = ui
            .radio
            .now
            .as_ref()
            .is_some_and(|n| n.key() == station.key());
        let favorite = ui.radio.is_favorite(station);
        // Right side: LIVE on the station playing, a star on favorites.
        let mut right = WIDTH - 36.;
        if favorite {
            c.icon(
                Icon::StarOn,
                right - 6.,
                y + STATION_H / 2.,
                15.,
                0xFFC94DFF,
            );
            right -= 22.;
        }
        if playing {
            let w = text::width_tracked(Face::S10, "LIVE", 1.2) + 26.;
            live_pill(c, right - w, y + STATION_H / 2., "LIVE");
            right -= w + 8.;
        }
        let room = right - 72.;
        c.text_mid(
            72.,
            y + 18.,
            room,
            &station.name,
            if selected { Face::S15 } else { Face::R15 },
            if playing && !selected {
                accent
            } else {
                color::TEXT
            },
        );
        let detail = if i < ui.radio.ranked {
            format!("Top {} · {}", ui.radio.ranked, station_detail(station))
        } else {
            station_detail(station)
        };
        c.text_mid(
            72.,
            y + 36.,
            room,
            &detail,
            Face::R11,
            if selected { accent } else { color::MUTED },
        );
    }
    if rows.len() > STATION_ROWS {
        components::scroll_rail(c, 44., 286., first, rows.len(), STATION_ROWS, accent);
    }
}

const CATEGORY_H: f32 = 48.;
const CATEGORY_ROWS: usize = 6;

fn categories(c: &mut Canvas, ui: &Ui, m: &AppModel, rows: &[Item], interactive: bool) {
    let accent = ui.accent();
    let genres = ui.radio.genres;
    components::status_bar(c, ui, m, if genres { "GENRES" } else { "COUNTRIES" }, true);
    let focus = m.navigation.focus.min(rows.len().saturating_sub(1));
    let first = components::list_window(m, focus, CATEGORY_ROWS);
    for (i, row) in rows.iter().enumerate().skip(first).take(CATEGORY_ROWS) {
        let y = 42. + (i - first) as f32 * CATEGORY_H;
        let selected = interactive && i == focus;
        if selected {
            components::accent_row(c, 12., y + 2., WIDTH - 24., CATEGORY_H - 4., accent);
        }
        let category = row
            .key
            .strip_prefix("category:")
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| ui.radio.categories.get(i));
        let tile = 34.;
        let (tx, ty) = (22., y + (CATEGORY_H - tile) / 2.);
        if genres {
            components::icon_tile(c, tx, ty, tile, 10., 0xA07BFFFF, Icon::Note, 18.);
        } else {
            // The country's code on a tile tinted from it ("DE", "FR").
            let code = category
                .map(|c| c.id.to_ascii_uppercase())
                .unwrap_or_default();
            let tints = [
                0x38C9B4FFu32,
                0x6EA0FFFF,
                0xFF7A59FF,
                0xA07BFFFF,
                0x5CCB7AFF,
                0xFFC94DFF,
            ];
            let tint = tints[reborn_core::logo_key(&code) as usize % tints.len()];
            c.rrect(tx, ty, tile, tile, 10., alpha(tint, 0x30));
            c.text_mid_center(tx + tile / 2., ty + tile / 2., &code, Face::S13, tint);
        }
        c.text_mid(
            70.,
            y + 17.,
            300.,
            &row.label,
            if selected { Face::S15 } else { Face::R15 },
            color::TEXT,
        );
        let count = category.map_or(0, |c| c.stations as usize);
        let detail = components::count(count, "station", "stations");
        c.text_mid(
            70.,
            y + 34.,
            300.,
            &detail,
            Face::R11,
            if selected { accent } else { color::MUTED },
        );
        c.icon(
            Icon::Chevron,
            WIDTH - 36.,
            y + CATEGORY_H / 2.,
            16.,
            color::DIM,
        );
    }
    if rows.len() > CATEGORY_ROWS {
        components::scroll_rail(c, 44., 286., first, rows.len(), CATEGORY_ROWS, accent);
    }
}

/// Station Info: the logo large, the facts the directory has, two actions.
fn info(c: &mut Canvas, ui: &Ui, m: &AppModel, rows: &[Item], interactive: bool) {
    components::status_bar(c, ui, m, "STATION INFO", true);
    let Some(s) = &ui.radio.info else {
        return;
    };
    let size = 128.;
    let (lx, ly) = (24., 50.);
    components::cover_shadow(c, lx, ly, size);
    match s.logo_key().filter(|k| ui.thumb_resident(*k)) {
        Some(_) => station_logo(c, ui, s, lx, ly, size),
        None => {
            station_logo(c, ui, s, -1000., -1000., 1.); // asks for the logo
            initials_tile(c, &s.name, lx, ly, size);
        }
    }
    let x = lx + size + 20.;
    let room = WIDTH - x - 18.;
    let playing = ui.radio.now.as_ref().is_some_and(|n| n.key() == s.key());
    let mut y = 62.;
    if playing {
        live_pill(c, x, y, "ON AIR");
        y += 22.;
    }
    for line in text::wrap(Face::S20, &s.name, room, 2) {
        c.text(x, y + 14., &line, Face::S20, color::TEXT);
        y += 25.;
    }
    if ui.radio.is_favorite(s) {
        c.icon(Icon::StarOn, x + 7., y + 6., 14., 0xFFC94DFF);
        c.text_mid(
            x + 20.,
            y + 6.,
            room - 20.,
            "Favorite",
            Face::R13,
            0xFFC94DFF,
        );
        y += 20.;
    }
    // Facts, label over value, in two columns under the logo.
    let facts = station_facts(s);
    let _ = y;
    let top = 192.;
    for (i, (label, value)) in facts.iter().take(6).enumerate() {
        let (col, line) = ((i % 2) as f32, (i / 2) as f32);
        let fx = 24. + col * 220.;
        let fy = top + line * 34.;
        c.text_tracked(
            fx,
            fy + Face::S10.mid_offset(),
            &label.to_uppercase(),
            Face::S10,
            color::DIM,
            1.4,
        );
        c.text_mid(fx, fy + 16., 206., value, Face::R13, color::TEXT);
    }
    // Actions as two buttons side by side at the bottom.
    let focus = m.navigation.focus.min(rows.len().saturating_sub(1));
    let (bw, bh, by) = (212., 34., HEIGHT - 48.);
    for (i, row) in rows.iter().enumerate().take(2) {
        let bx = 24. + i as f32 * (bw + 8.);
        let selected = interactive && i == focus;
        let at = c.draw.len();
        if selected {
            c.rrect(bx, by, bw, bh, 17., color::TEXT);
            c.mark_focus(at);
        } else {
            c.rrect(bx, by, bw, bh, 17., color::GLASS);
        }
        c.text_mid_center(
            bx + bw / 2.,
            by + bh / 2.,
            &row.label,
            Face::S13,
            if selected { color::INK } else { color::TEXT },
        );
    }
}

/// What the directory knows about a station, as label and value (Station
/// Info draws them; skins get them as the page's facts).
pub(crate) fn station_facts(s: &reborn_core::RadioStation) -> Vec<(&'static str, String)> {
    let mut facts: Vec<(&'static str, String)> = vec![];
    if !s.country.is_empty() {
        facts.push(("Country", s.country.clone()));
    }
    let languages = s.languages();
    if !languages.is_empty() {
        facts.push(("Language", languages));
    }
    let format = s.format();
    if !format.is_empty() {
        facts.push(("Format", format));
    }
    if s.votes > 0 {
        facts.push(("Votes", count_label(s.votes as usize)));
    }
    let genres = s.genres(3).join(", ");
    if !genres.is_empty() {
        facts.push(("Genres", genres));
    }
    if !s.homepage.is_empty() {
        let host = s
            .homepage
            .split("://")
            .nth(1)
            .unwrap_or(&s.homepage)
            .split('/')
            .next()
            .unwrap_or("")
            .trim_start_matches("www.")
            .to_owned();
        facts.push(("Homepage", host));
    }
    facts
}
