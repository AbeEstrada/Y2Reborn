//! Native 480×360 composition for UI v3: one screen at a time, one focus,
//! plus the overlays (sheets, dialogs, toasts, the volume line, the Power
//! hold) that sit above any screen.
use crate::{
    canvas::Canvas,
    components::{self, SheetRow, Tone},
    text::{self, Face},
    theme::{color, HEIGHT, WIDTH},
    views_library, views_playing, views_settings, views_system, Alert, Item, Ui,
};
use reborn_core::{platform::LowBattery, AppModel, Modal, Screen, Track};
use reborn_graphics::Quad;

/// Rows visible in one window for a screen's list area.
pub fn visible_rows_for(screen: Screen) -> usize {
    match screen {
        Screen::Albums => 8,
        Screen::Album | Screen::Artist => 5,
        Screen::Folders | Screen::Queue => 5,
        Screen::Tracks | Screen::Artists | Screen::Genres => 6,
        _ => 6,
    }
}

pub fn draw(ui: &Ui, m: &AppModel, tracks: &[Track], has_art: bool) -> Vec<Quad> {
    let mut c = Canvas::new();
    c.rect(0., 0., WIDTH, HEIGHT, color::BG);
    if matches!(
        m.platform.busy,
        Some(reborn_core::PlatformTask::UpdateApply | reborn_core::PlatformTask::UpdateRollback)
    ) {
        views_system::installing(&mut c);
        return c.finish();
    }
    if m.screen_off {
        return c.finish();
    }
    let interactive = m.navigation.modal.is_none()
        && ui.pairing.is_none()
        && !ui.text_entry
        && ui.alert.is_none();
    if ui.usb_overlay(m) {
        views_system::usb(&mut c, ui, m);
        overlays(&mut c, ui, m, tracks);
        return c.finish();
    }
    match m.screen {
        Screen::NowPlaying if m.current().is_some() => {
            views_playing::now_playing(&mut c, ui, m, tracks, has_art, interactive)
        }
        Screen::Home => views_library::home(&mut c, ui, m, tracks, has_art, interactive),
        Screen::Albums
        | Screen::Artists
        | Screen::Genres
        | Screen::Tracks
        | Screen::Folders
        | Screen::Album
        | Screen::Artist
        | Screen::Queue => views_library::browse(&mut c, ui, m, tracks, has_art, interactive),
        Screen::Settings
        | Screen::Wifi
        | Screen::Bluetooth
        | Screen::SettingsAudio
        | Screen::Equalizer
        | Screen::Crossfade
        | Screen::ScreenTimeout
        | Screen::SettingsDisplay
        | Screen::Storage
        | Screen::Update => views_settings::screen(&mut c, ui, m, tracks, interactive),
        _ => views_system::page(&mut c, ui, m, tracks, interactive),
    }
    overlays(&mut c, ui, m, tracks);
    c.finish()
}

/// The sheet, dialog or entry surface above the screen, then toasts and the
/// volume line.
fn overlays(c: &mut Canvas, ui: &Ui, m: &AppModel, tracks: &[Track]) {
    if let Some(alert) = &ui.alert {
        let ok = |label| SheetRow {
            label,
            detail: "",
            enabled: true,
        };
        match alert {
            Alert::BluetoothFailed { name, .. } => components::dialog(
                c,
                "Couldn't connect",
                &format!("{name} did not answer. Check that it is on and in range."),
                &[ok("Cancel"), ok("Try again")],
                ui.alert_focus(),
                false,
            ),
            Alert::BatteryLow => {
                let body = m
                    .platform
                    .battery
                    .percent
                    .map(|p| format!("{p}% left. Charge soon to keep playing."))
                    .unwrap_or_else(|| "Charge soon to keep playing.".into());
                components::dialog(c, "Battery low", &body, &[ok("OK")], 0, false);
            }
        }
    } else if let Some(pair) = &ui.pairing {
        components::dialog(
            c,
            "Pair with this device?",
            pair,
            &[
                SheetRow {
                    label: "Pair",
                    detail: "",
                    enabled: true,
                },
                SheetRow {
                    label: "Cancel",
                    detail: "",
                    enabled: true,
                },
            ],
            ui.pairing_focus,
            false,
        );
    } else if ui.text_entry {
        views_settings::password_entry(c, ui, m);
    } else if m.navigation.modal.is_some() {
        modal(c, ui, m, tracks);
    }
    if matches!(
        m.platform.battery.level,
        LowBattery::Critical | LowBattery::ShuttingDown
    ) {
        let percent = m
            .platform
            .battery
            .percent
            .map(|p| format!("Battery low · {p}%"))
            .unwrap_or_else(|| "Battery low".into());
        components::toast(c, &percent, Tone::Error);
    } else if !ui.notice.is_empty()
        && ui
            .notice_until
            .is_some_and(|t| t > std::time::Instant::now())
    {
        let tone = if is_problem(&ui.notice) {
            Tone::Error
        } else {
            Tone::Info
        };
        components::toast(c, &ui.notice, tone);
    }
    views_playing::volume_line(c, ui, m);
    views_playing::power_hold(c, ui);
}

fn is_problem(message: &str) -> bool {
    let lower = message.to_lowercase();
    ["can't", "couldn't", "failed", "no audio", "error"]
        .iter()
        .any(|w| lower.contains(w))
}

fn modal(c: &mut Canvas, ui: &Ui, m: &AppModel, tracks: &[Track]) {
    let (title, body) = ui.modal_copy(m);
    let rows = ui.modal_rows_public(m, tracks);
    let focus = m.navigation.modal_focus.min(rows.len().saturating_sub(1));
    let sheet: Vec<SheetRow> = rows
        .iter()
        .map(|r| SheetRow {
            label: &r.label,
            detail: &r.secondary,
            enabled: r.enabled,
        })
        .collect();
    match &m.navigation.modal {
        Some(Modal::Confirm(action)) => {
            let danger = !matches!(action, reborn_core::ConfirmAction::Platform(_));
            components::dialog(c, &title, &body, &sheet, focus, danger);
        }
        Some(Modal::ContextMenu) => {
            let (song, subtitle) = context_song(ui, m, tracks);
            let title = song.unwrap_or(title);
            components::action_sheet(c, &title, &subtitle, &sheet, focus);
        }
        _ => components::action_sheet(c, &title, &body, &sheet, focus),
    }
}

/// For a song's options sheet: the song's title and artist.
fn context_song(ui: &Ui, m: &AppModel, tracks: &[Track]) -> (Option<String>, String) {
    let song = if m.screen == Screen::NowPlaying {
        m.current()
    } else {
        let key = m.navigation.context_key.clone().or_else(|| {
            m.navigation
                .context_target
                .and_then(|i| ui.row_at(m, tracks, i).map(|r| r.key))
        });
        key.as_deref()
            .and_then(|k| k.strip_prefix("track:"))
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| tracks.get(i))
    };
    match song {
        Some(t) => (
            Some(crate::track_title(t).to_owned()),
            crate::display_or_unknown(&t.artist).to_owned(),
        ),
        None => (None, String::new()),
    }
}

/// The screen's title in the status bar.
pub fn screen_title(m: &AppModel) -> String {
    match m.screen {
        Screen::Home => String::new(),
        Screen::Music => "Music".into(),
        Screen::Albums => match m.navigation.filter.strip_prefix("genre:") {
            Some("") => "No Genre".into(),
            Some(genre) => genre.into(),
            None => "Albums".into(),
        },
        Screen::Genres => "Genres".into(),
        Screen::Artists => "Artists".into(),
        Screen::Tracks => "Songs".into(),
        Screen::Folders => {
            if m.navigation.filter.is_empty() {
                "Folders".into()
            } else {
                std::path::Path::new(m.navigation.filter.trim_start_matches("folder:"))
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Folders".into())
            }
        }
        Screen::Album => "Album".into(),
        Screen::Artist => "Artist".into(),
        Screen::NowPlaying => "Now Playing".into(),
        Screen::Queue => "Up next".into(),
        Screen::TrackInfo => "Song info".into(),
        Screen::Settings => "Settings".into(),
        Screen::Wifi => "Wi-Fi".into(),
        Screen::Bluetooth => "Bluetooth".into(),
        Screen::PcTransfer => "Computer".into(),
        Screen::SettingsAudio => "Sound".into(),
        Screen::Equalizer => "Equalizer".into(),
        Screen::SettingsPlayback => "Playback".into(),
        Screen::Crossfade => "Crossfade".into(),
        Screen::ScreenTimeout => "Screen off after".into(),
        Screen::SettingsLibrary => "Library".into(),
        Screen::SettingsDisplay => "Display".into(),
        Screen::SettingsSystem => "Service".into(),
        Screen::Battery => "Battery".into(),
        Screen::Sleep => "Sleep".into(),
        Screen::Storage => "Storage".into(),
        Screen::Update => "About & Updates".into(),
        Screen::About => "About".into(),
        Screen::Maintenance => "Reset & maintenance".into(),
        Screen::Diagnostics => "Diagnostics".into(),
        Screen::DiagnosticSection => crate::diagnostics::title(&m.navigation.filter).into(),
        Screen::ValueDetail | Screen::TextEntry | Screen::Pairing => "Reborn".into(),
    }
}

/// Split `text` into at most `lines` lines that fit `width` at `size` px.
pub(crate) fn wrap_lines(text: &str, width: f32, face: Face, lines: usize) -> Vec<String> {
    text::wrap(face, text, width, lines)
}

/// The number of songs, as the Home list shows it.
pub(crate) fn count_label(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('\u{2009}');
        }
        out.push(ch);
    }
    out
}

/// A song row's duration, or nothing for rows that are not songs.
pub(crate) fn row_track<'a>(row: &Item, tracks: &'a [Track]) -> Option<(usize, &'a Track)> {
    row.key
        .strip_prefix("track:")
        .and_then(|s| s.parse::<usize>().ok())
        .and_then(|i| tracks.get(i).map(|t| (i, t)))
}

pub(crate) fn is_current(m: &AppModel, t: &Track) -> bool {
    m.current()
        .is_some_and(|c| c.id == t.id && c.source_id == t.source_id)
}
