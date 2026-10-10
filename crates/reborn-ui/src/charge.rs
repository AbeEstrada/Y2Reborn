//! The charging screen shown while the player is off and charging. It is drawn
//! by Y2Linux's rescue-environment charger (`offline-charge.c`) from images
//! rendered here (`reborn-preview` exports `charge-layout.json`); only the
//! ring's sweeping segment is drawn there. It never shows a percentage or a
//! time to full: the ring only shows that charging is happening.
use crate::{
    canvas::{Canvas, Icon},
    text::Face,
    theme::{color, HEIGHT, WIDTH},
};
use reborn_graphics::Quad;

pub const CHARGE_CX: f32 = 240.;
pub const CHARGE_CY: f32 = 124.;
/// Outer and inner radius of the ring, in pixels.
pub const CHARGE_RING_OUTER: f32 = 60.;
pub const CHARGE_RING_INNER: f32 = 54.;
pub const CHARGE_TITLE_Y: f32 = 224.;
pub const CHARGE_HINT_Y: f32 = 254.;
/// What the ring's lit part is drawn in while charging or charged.
pub const CHARGE_LIT: u32 = color::SUCCESS >> 8;
/// The opaque ring track and the background, as 24-bit RGB.
pub const CHARGE_TRACK: u32 = color::RING_TRACK >> 8;
pub const CHARGE_BG: u32 = color::BG >> 8;

pub const CHARGE_TITLES: [&str; 3] = ["Charging", "Charged", "Charging paused"];
pub const CHARGE_HINTS: [&str; 3] = [
    "Hold Power to turn on",
    "Release to start",
    "Battery too low to start",
];

fn background() -> Canvas {
    let mut c = Canvas::new();
    c.rect(0., 0., WIDTH, HEIGHT, color::BG);
    c
}

/// The ring track and the bolt, without text or the lit segment.
pub fn charge_base_frame() -> Vec<Quad> {
    let mut c = background();
    c.arc(CHARGE_CX, CHARGE_CY, 62., 1., color::RING_TRACK);
    c.icon(Icon::Bolt, CHARGE_CX, CHARGE_CY, 40., 0xC8CDD7FF);
    c.finish()
}

/// A complete frame for previews and tests: the base, the lit part of the ring
/// (`turn` 0..=1 of a circle from 12 o'clock) and the title and hint lines.
pub fn charge_frame(turn: f32, title: usize, hint: usize) -> Vec<Quad> {
    let mut c = Canvas::from_quads(charge_base_frame());
    c.arc(CHARGE_CX, CHARGE_CY, 62., turn, color::SUCCESS);
    let mut quads = c.finish();
    quads.extend(
        charge_label_frame(CHARGE_TITLES[title], CHARGE_TITLE_Y)
            .into_iter()
            .skip(1),
    );
    quads.extend(
        charge_label_frame(CHARGE_HINTS[hint], CHARGE_HINT_Y)
            .into_iter()
            .skip(1),
    );
    quads
}

/// One line of text, centred; the title is larger than the hint.
pub fn charge_label_frame(text: &str, y: f32) -> Vec<Quad> {
    let mut c = background();
    if CHARGE_TITLES.contains(&text) {
        c.text_center(
            240.,
            y + Face::S20.mid_offset(),
            text,
            Face::S20,
            color::TEXT,
        );
    } else {
        c.text_center(
            240.,
            y + Face::R13.mid_offset(),
            text,
            Face::R13,
            color::MUTED,
        );
    }
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn charging_copy_claims_no_percentage_or_time() {
        for text in CHARGE_TITLES.iter().chain(&CHARGE_HINTS) {
            assert!(
                !text.chars().any(|c| c.is_ascii_digit() || c == '%'),
                "{text}"
            );
            assert!(!text.to_lowercase().contains("full"), "{text}");
        }
    }

    #[test]
    fn base_frame_is_centred_and_text_free() {
        let quads = charge_base_frame();
        assert!(quads
            .iter()
            .all(|q| !matches!(q.tex, reborn_graphics::Tex::Font(_))));
        let ring = quads
            .iter()
            .find(|q| matches!(q.tex, reborn_graphics::Tex::Mask) && q.w > 100.)
            .expect("ring");
        assert_eq!(ring.x + ring.w / 2., CHARGE_CX);
        assert_eq!(ring.y + ring.h / 2., CHARGE_CY);
    }
}
