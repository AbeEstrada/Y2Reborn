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
    crate::views_settings::steps(c, 2, color::ACCENT);
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
    components::status_bar(c, ui, m, &screen_title(m), true);
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
        _ => group_page(c, ui, m, &page, focus, interactive, 38.),
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
    components::button(
        c,
        24.,
        HEIGHT - 56.,
        96.,
        34.,
        "Back",
        Face::S13,
        interactive,
    );
}
