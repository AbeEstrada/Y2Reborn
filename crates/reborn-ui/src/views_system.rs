//! Everything outside the main flows: the service menu, battery, sleep, the
//! computer connection, diagnostics and the installing screen.
use crate::{
    canvas::Canvas,
    components,
    pages::Page,
    screens::screen_title,
    text::{self, Face},
    theme::{color, HEIGHT},
    views_settings::{group_page, message},
    Ui,
};
use reborn_core::{AppModel, Screen, Track};

/// Shown while an update is installed: the wordmark and one calm line.
pub fn installing(c: &mut Canvas) {
    crate::boot::mark(c, 1.);
    c.text_center_tracked(
        240.,
        230. + Face::S10.mid_offset(),
        "PREPARING TO INSTALL…",
        Face::S10,
        color::MUTED,
        1.6,
    );
    c.text_center(
        240.,
        256. + Face::R11.mid_offset(),
        "Keep the player charging",
        Face::R11,
        color::DIM,
    );
}

pub fn page(c: &mut Canvas, ui: &Ui, m: &AppModel, tracks: &[Track], interactive: bool) {
    components::status_bar(c, ui, m, &screen_title(m).to_uppercase(), true);
    let page = match m.screen {
        Screen::Diagnostics | Screen::DiagnosticSection => Page {
            rows: ui.rows(m, tracks),
            ..Default::default()
        },
        _ => crate::pages::page(ui, m, tracks),
    };
    let focus = m.navigation.focus.min(page.rows.len().saturating_sub(1));
    match m.screen {
        Screen::ValueDetail => value_detail(c, m, interactive),
        _ => group_page(c, ui, &page, focus, interactive, 38.),
    }
}

fn value_detail(c: &mut Canvas, m: &AppModel, interactive: bool) {
    let (title, value) = m
        .navigation
        .filter
        .split_once('\u{1f}')
        .unwrap_or(("Detail", &m.navigation.filter));
    let y = message(c, title, "", 44.);
    let mut cy = y + 6.;
    for line in text::wrap(Face::R15, value, 432., 8) {
        c.text(
            24.,
            cy + Face::R15.mid_offset(),
            &line,
            Face::R15,
            color::TEXT,
        );
        cy += 22.;
    }
    let at = c.draw.len();
    components::pill(
        c,
        24.,
        HEIGHT - 56.,
        96.,
        34.,
        "Back",
        Face::S13,
        if interactive {
            0xF2F3F5FF
        } else {
            color::GLASS
        },
        color::INK,
    );
    if interactive {
        c.mark_focus(at);
    }
}

/// The computer connection: what to do when it is ready, and a calm
/// "copying" state with a sweeping ring (no progress is claimed).
pub fn usb(c: &mut Canvas, ui: &Ui, m: &AppModel) {
    use crate::canvas::Icon;
    use reborn_core::platform::{SdCard, UsbTransfer};
    components::status_bar(c, ui, m, "COMPUTER", false);
    let copying = m.platform.snapshot.usb == UsbTransfer::Copying;
    if copying {
        c.spinner(
            240.,
            124.,
            44.,
            ui.accent(),
            color::RING_TRACK,
            ui.spin_phase(),
        );
        c.text_center(
            240.,
            204. + Face::S20.mid_offset(),
            "Copying Files…",
            Face::S20,
            color::TEXT,
        );
        c.text_center(
            240.,
            230. + Face::R13.mid_offset(),
            "Keep the cable connected until your computer finishes.",
            Face::R13,
            color::MUTED,
        );
        c.text_center(
            240.,
            300. + Face::R11.mid_offset(),
            "Don't unplug until it finishes.",
            Face::R11,
            color::WARNING,
        );
        return;
    }
    c.text_center(
        240.,
        84. + Face::S25.mid_offset(),
        "Connected to a Computer",
        Face::S25,
        color::TEXT,
    );
    c.text_center(
        240.,
        112. + Face::R13.mid_offset(),
        "Copy music to the drives that appear.",
        Face::R13,
        color::MUTED,
    );
    let storage = &m.platform.snapshot.storage;
    let free = |space: &reborn_core::platform::VolumeSpace| {
        space
            .free_bytes
            .map(|f| format!("{} free", reborn_core::platform::human_bytes(f)))
            .unwrap_or_default()
    };
    let cards = [
        (
            "Internal Storage",
            Icon::Folder,
            0x6EA0FFFFu32,
            free(&storage.internal),
        ),
        (
            "SD Card",
            Icon::Sd,
            0x3CC878FF,
            match &storage.sd {
                SdCard::Ready(space) => free(space),
                _ => "No SD card".into(),
            },
        ),
    ];
    for (i, (name, icon, tint, detail)) in cards.into_iter().enumerate() {
        let x = 28. + i as f32 * 212.;
        components::card(c, x, 148., 196., 96.);
        c.icon(icon, x + 30., 178., 26., tint);
        c.text_mid(x + 18., 214., 168., name, Face::S15, color::TEXT);
        c.text_mid(x + 18., 232., 168., &detail, Face::R11, color::MUTED);
    }
    c.text_center(
        240.,
        316. + Face::R11.mid_offset(),
        "Your player keeps charging while connected.",
        Face::R11,
        color::DIM,
    );
}
