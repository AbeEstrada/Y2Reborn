//! Now Playing, the volume line and the Power hold.
use crate::{
    canvas::{Canvas, Icon},
    components::{self, cover_shadow},
    pages,
    text::{self, Face},
    theme::{alpha, color, fade, HEIGHT, WIDTH},
    Ui,
};
use reborn_core::{platform::SdCard, AppModel, PlaybackState, RepeatMode, Track};
use reborn_graphics::{Quad, Tex};

pub fn now_playing(
    c: &mut Canvas,
    ui: &Ui,
    m: &AppModel,
    tracks: &[Track],
    has_art: bool,
    interactive: bool,
) {
    let _ = tracks;
    let Some(t) = m.current() else {
        return;
    };
    let accent = ui.accent();
    // The cover's colour washes the top of the screen; the bottom stays dark.
    c.glow(150., 90., 300., 210., alpha(accent, 0x3C));
    c.glow(420., 60., 160., 120., alpha(accent, 0x1E));
    c.fade_from_bottom(0., 120., WIDTH, HEIGHT - 120., 0x06070ADC);
    components::status_bar(c, ui, m, "", true);

    cover_shadow(c, 26., 46., 176.);
    if has_art {
        c.draw.push(Quad {
            tex: Tex::Art,
            ..Quad::rect(26., 46., 176., 176., color::TEXT)
        });
    } else {
        components::placeholder(c, 26., 46., 176., 15., 0);
    }

    c.text_tracked(
        222.,
        61. + Face::S10.mid_offset(),
        "NOW PLAYING",
        Face::S10,
        accent,
        1.8,
    );
    let lines = text::wrap(Face::S25, crate::track_title(t), 236., 2);
    let mut baseline = 107.;
    for line in &lines {
        c.text(222., baseline, line, Face::S25, color::TEXT);
        baseline += 28.;
    }
    // `baseline` now sits one pitch below the last title line.
    let artist_baseline = baseline - 1.;
    c.text(
        222.,
        artist_baseline,
        &text::fit(Face::R15, crate::display_or_unknown(&t.artist), 236.),
        Face::R15,
        0xDEE1E8FF,
    );
    c.text(
        222.,
        artist_baseline + 20.,
        &text::fit(Face::R11, crate::display_or_unknown(&t.album), 236.),
        Face::R11,
        color::MUTED,
    );
    let info_cy = artist_baseline + 45.;
    let source_offline =
        !m.sources.is_empty() && !m.sources.iter().any(|s| s.id == t.source_id && s.online);
    if source_offline
        || m.playback == PlaybackState::Error
        || m.playback == PlaybackState::Buffering
    {
        let note = if source_offline {
            "SD card removed · reinsert it"
        } else if m.playback == PlaybackState::Error {
            "No audio output · check headphones"
        } else {
            "Starting…"
        };
        c.text_mid(222., info_cy, 236., note, Face::S13, color::WARNING);
    } else if let Some(label) = quality_label(t) {
        components::tag(c, 222., info_cy, &label, accent);
    }
    status_icons(c, m, 222., info_cy + 33., accent);

    // Progress.
    let (x0, x1) = (26., WIDTH - 26.);
    components::slider(
        c,
        x0,
        263.,
        x1 - x0,
        components::progress(m.position_ms, t.duration_ms),
        color::TEXT,
        true,
    );
    c.text_mid(
        x0,
        281.,
        80.,
        &components::time(m.position_ms),
        Face::R11,
        color::MUTED,
    );
    let remaining = format!(
        "-{}",
        components::time(t.duration_ms.saturating_sub(m.position_ms))
    );
    c.text_mid_right(x1, 281., &remaining, Face::R11, color::MUTED);

    // Transport.
    let cy = 322.;
    let at = c.draw.len();
    c.disc(240., cy, 25., color::TEXT);
    if interactive {
        c.mark_focus(at);
    }
    let playing = matches!(
        m.playback,
        PlaybackState::Playing | PlaybackState::Buffering
    );
    c.icon(
        if playing {
            Icon::PauseLarge
        } else {
            Icon::PlayLarge
        },
        241.,
        cy,
        26.,
        color::INK,
    );
    c.icon(Icon::PrevLarge, 240. - 78., cy, 30., color::TEXT);
    c.icon(Icon::NextLarge, 240. + 78., cy, 30., color::TEXT);
    let output = pages::output_name(ui, m);
    c.text_mid(x0, cy, 96., &output, Face::R11, color::MUTED);
}

/// `FLAC · 96 KHZ`: what the file is, in a sentence the listener understands.
pub(crate) fn quality_label(t: &Track) -> Option<String> {
    if t.codec.is_empty() {
        return None;
    }
    let lossless = ["flac", "alac", "wav", "aiff", "ape", "wv", "pcm"]
        .iter()
        .any(|c| t.codec.eq_ignore_ascii_case(c));
    let rate = if t.sample_rate == 0 {
        String::new()
    } else {
        format!(" · {}", pages::khz(t.sample_rate))
    };
    Some(if lossless {
        format!("LOSSLESS{rate}")
    } else {
        format!("{}{rate}", t.codec.to_uppercase())
    })
}

/// Icons for the playback options that are on; nothing for the ones that are off.
fn status_icons(c: &mut Canvas, m: &AppModel, x: f32, cy: f32, accent: u32) {
    let s = &m.settings;
    let mut icons = vec![];
    if s.shuffle {
        icons.push(Icon::Shuffle);
    }
    match s.repeat {
        RepeatMode::All => icons.push(Icon::Repeat),
        RepeatMode::Track => icons.push(Icon::RepeatOne),
        RepeatMode::Off => {}
    }
    if s.gapless_enabled {
        icons.push(Icon::Gapless);
    }
    if s.crossfade_ms > 0 {
        icons.push(Icon::Crossfade);
    }
    if s.eq_enabled {
        icons.push(Icon::Eq);
    }
    if s.replay_gain != reborn_core::ReplayGainMode::Off {
        icons.push(Icon::Normalise);
    }
    for (i, icon) in icons.into_iter().enumerate() {
        c.icon(icon, x + 10. + i as f32 * 30., cy, 20., accent);
    }
}

/// A bare vertical line at the right edge, visible for about a second after
/// the volume changes. No icon, no number.
pub fn volume_line(c: &mut Canvas, ui: &Ui, m: &AppModel) {
    let visibility = ui.volume_visibility();
    if visibility <= 0. {
        return;
    }
    let (x, top, bottom) = (WIDTH - 15., 110., 250.);
    c.glow(
        x,
        (top + bottom) / 2.,
        20.,
        98.,
        fade(0x000000B4, visibility),
    );
    c.rrect(
        x - 2.,
        top,
        4.,
        bottom - top,
        2.,
        fade(0xFFFFFF46, visibility),
    );
    let height = ((bottom - top) * f32::from(m.settings.volume.min(100)) / 100.).round();
    if height >= 1. {
        c.rrect(
            x - 2.,
            bottom - height,
            4.,
            height,
            2.,
            fade(color::TEXT, visibility),
        );
    }
}

/// The ring that fills while Power is held: release to cancel.
pub fn power_hold(c: &mut Canvas, ui: &Ui) {
    let Some(progress) = ui.power_hold_progress() else {
        return;
    };
    let appear = (progress * 6.).min(1.);
    components::scrim(c, true);
    let (cx, cy) = (240., 148.);
    c.ring(cx, cy, 52., 3., fade(color::GLASS, appear));
    c.arc(cx, cy, 52., progress, fade(color::DANGER, appear));
    c.disc(cx, cy, 40., fade(0x1E2026FF, appear));
    c.icon(Icon::Power, cx, cy, 30., fade(color::TEXT, appear));
    c.text_center(
        240.,
        238. + Face::S17.mid_offset(),
        "Hold to turn off",
        Face::S17,
        fade(color::TEXT, appear),
    );
    c.text_center(
        240.,
        262. + Face::R13.mid_offset(),
        "Your place is saved",
        Face::R13,
        fade(color::MUTED, appear),
    );
    c.text_center(
        240.,
        306. + Face::R11.mid_offset(),
        "Release to cancel",
        Face::R11,
        fade(color::DIM, appear),
    );
    let _ = SdCard::Absent;
}
