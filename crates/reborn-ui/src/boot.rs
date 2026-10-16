//! Startup, failure and shutdown screens: the lowercase "reborn" wordmark over
//! a faint cool glow, one hairline progress bar with a leading dot and one
//! tracked-caps status line. The early splash in Y2Linux draws the same group
//! from this module's layout and phase table (`reborn-preview` exports both),
//! so the hand-off frame is identical.
use crate::{
    canvas::Canvas,
    text::Face,
    theme::{color, fade, HEIGHT, WIDTH},
};
use reborn_graphics::Quad;

/// One real startup milestone. `token` is the name the producers report: the
/// initramfs stage names, Reborn's own `startup_phase` names and `ready`.
/// `label` is what is happening from this milestone on; `fill_permille` is a
/// coarse ordinal position of the bar, not a time or work estimate, and it is
/// never shown as a number.
pub struct BootPhase {
    pub token: &'static str,
    pub label: &'static str,
    pub fill_permille: u16,
}

const fn phase(token: &'static str, label: &'static str, fill_permille: u16) -> BootPhase {
    BootPhase {
        token,
        label,
        fill_permille,
    }
}

/// Startup milestones in the order the boot actually reaches them. Fills are
/// strictly increasing, so the bar can only move forward.
pub const BOOT_PHASES: &[BootPhase] = &[
    phase("start", "Starting system", 0),
    phase("storage_discovery", "Preparing storage", 25),
    phase("rescue_update", "Preparing storage", 45),
    phase("storage_preflight", "Preparing storage", 65),
    phase("fsck_start", "Preparing storage", 85),
    phase("fsck_complete", "Preparing storage", 110),
    phase("root_data_mounted", "Starting system", 140),
    phase("switch_root", "Starting system", 160),
    phase("rc_data", "Starting system", 200),
    // Y2Linux STORAGE-SETUP (once): the old storage becomes Internal Storage;
    // the platform reports its progress in tens (copy, format, move).
    phase("media_setup", "Setting up storage", 204),
    phase("media_setup_10", "Keeping your old files", 207),
    phase("media_setup_20", "Keeping your old files", 210),
    phase("media_setup_30", "Keeping your old files", 213),
    phase("media_setup_40", "Keeping your old files", 216),
    phase("media_setup_50", "Preparing Internal Storage", 219),
    phase("media_setup_60", "Moving your old files", 222),
    phase("media_setup_70", "Moving your old files", 225),
    phase("media_setup_80", "Moving your old files", 228),
    phase("media_setup_90", "Moving your old files", 231),
    phase("media_setup_100", "Internal Storage ready", 234),
    phase("rc_time", "Setting the clock", 240),
    phase("rc_power", "Starting power", 270),
    phase("rc_bluetooth", "Starting Bluetooth", 300),
    phase("rc_connectivity", "Starting connectivity", 330),
    phase("rc_readiness", "Starting connectivity", 360),
    phase("platform_ready", "Starting connectivity", 400),
    phase("conn_factory", "Starting connectivity", 440),
    phase("conn_calibration", "Starting connectivity", 480),
    phase("conn_activated", "Starting connectivity", 520),
    phase("conn_wifi_wait", "Starting Wi-Fi", 540),
    phase("conn_wifi", "Starting Wi-Fi", 600),
    phase("conn_bluetooth_wait", "Starting Bluetooth", 620),
    phase("conn_bluetooth", "Starting Bluetooth", 680),
    phase("system_ready", "Starting Reborn", 700),
    phase("model_restored", "Starting Reborn", 730),
    phase("graphics_ready", "Loading music library", 770),
    phase("storage_ready", "Loading music library", 800),
    phase("library_workers_ready", "Starting audio", 850),
    phase("core_services_ready", "Starting audio", 880),
    phase("audio_ready", "Starting connectivity", 910),
    phase("radio_workers_ready", "Starting Reborn", 940),
    phase("runtime_ready", "Starting Reborn", 970),
    phase("ready", "Starting Reborn", 1000),
];

/// A stage that means startup cannot continue (the initramfs rescue shell).
pub const BOOT_FAILURE_TOKENS: &[&str] = &["rescue"];
/// Shown instead of the bar when startup failed or timed out: a headline and
/// two plain lines of advice.
pub const BOOT_FAILURE_LABELS: [&str; 3] = [
    "Couldn't Start",
    "Hold Power for 10 seconds,",
    "then turn it on again.",
];
/// The status line of the last phase; the dissolve into the UI keeps it.
pub const BOOT_FINAL_LABEL: &str = "Starting Reborn";

/// Vertical centre of the wordmark.
pub const MARK_Y: f32 = 164.;
pub const MARK_TRACKING: f32 = 3.;
pub const BAR_W: f32 = 124.;
pub const BAR_X: f32 = 178.;
pub const BAR_Y: f32 = 204.;
pub const BAR_H: f32 = 2.;
/// Centre line of the status text, and the failure headline.
pub const LABEL_Y: f32 = 230.;
pub const FAILURE_LINE_PITCH: f32 = 22.;
const LABEL_TRACKING: f32 = 1.6;
/// The leading dot, per row from the top: (offset, width) in pixels.
const DOT: [(f32, f32); 6] = [(2., 2.), (1., 4.), (0., 6.), (0., 6.), (1., 4.), (2., 2.)];
const MARK_COLOR: u32 = color::TEXT;
const GLOW_COLOR: u32 = 0x464E6228;
const BAR_TRACK_RGB: u32 = 0x2D2E30;
/// Opaque colours: the early splash paints them without blending.
pub const BAR_TRACK: u32 = BAR_TRACK_RGB << 8 | 0xFF;
pub const BAR_FILL: u32 = color::TEXT;

/// Frames of the reveal of the first UI frame (≈0.34 s at the UI's 34 ms
/// cadence). The first frame is the full boot screen, so the hand-off is
/// seamless; the last is the UI alone. The bar and status line finish first,
/// then the wordmark lifts slightly while the UI dissolves in.
pub const BOOT_FADE_FRAMES: u8 = 10;
const REVEAL_LIFT: f32 = 8.;

pub const SAVING_LABEL: &str = "Saving your place";
const CLOSING_LABEL: &str = "Closing";

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0., 1.);
    x * x * (3. - 2. * x)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The wordmark with its glow, `lift` pixels above its place.
pub fn mark(c: &mut Canvas, opacity: f32) {
    mark_at(c, opacity, 0.);
}

fn mark_at(c: &mut Canvas, opacity: f32, lift: f32) {
    c.glow(240., 148. - lift, 170., 44., fade(GLOW_COLOR, opacity));
    c.text_center_tracked(
        240.,
        MARK_Y + Face::L44.mid_offset() - lift,
        "reborn",
        Face::L44,
        fade(MARK_COLOR, opacity),
        MARK_TRACKING,
    );
}

/// The hairline track with `fill` (0..=1) drawn from the left and a leading dot.
fn bar(c: &mut Canvas, fill: f32, opacity: f32) {
    let fill = fill.clamp(0., 1.);
    c.rect(BAR_X, BAR_Y, BAR_W, BAR_H, fade(BAR_TRACK, opacity));
    let lit = (BAR_W * fill).round();
    c.rect(BAR_X, BAR_Y, lit, BAR_H, fade(BAR_FILL, opacity));
    if lit >= 3. && fill < 1. {
        // Hard-edged pixels, so the early splash can draw the identical dot.
        for (row, (offset, width)) in DOT.into_iter().enumerate() {
            c.rect(
                BAR_X + lit - 3. + offset,
                BAR_Y - 2. + row as f32,
                width,
                1.,
                fade(BAR_FILL, opacity),
            );
        }
    }
}

fn status(c: &mut Canvas, y: f32, text: &str, opacity: f32) {
    c.text_center_tracked(
        240.,
        y + Face::S10.mid_offset(),
        &text.to_uppercase(),
        Face::S10,
        fade(color::MUTED, opacity),
        LABEL_TRACKING,
    );
}

/// One failure line: the first is a headline, the rest quiet advice.
fn failure_line(c: &mut Canvas, index: usize, y: f32, text: &str) {
    if index == 0 {
        c.text_center(
            240.,
            y + Face::S17.mid_offset(),
            text,
            Face::S17,
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
}

/// Where failure line `index` sits.
pub fn failure_line_y(index: usize) -> f32 {
    if index == 0 {
        LABEL_Y
    } else {
        LABEL_Y + 8. + FAILURE_LINE_PITCH * index as f32
    }
}

fn group(c: &mut Canvas, fill: f32, text: &str, opacity: f32) {
    mark(c, opacity);
    bar(c, fill, opacity);
    status(c, LABEL_Y, text, opacity);
}

fn background() -> Canvas {
    let mut c = Canvas::new();
    c.rect(0., 0., WIDTH, HEIGHT, color::BG);
    c
}

/// The complete boot screen at `fill` (0..=1) with the status `text`.
pub fn boot_screen(fill: f32, text: &str) -> Vec<Quad> {
    let mut c = background();
    group(&mut c, fill, text, 1.);
    c.finish()
}

/// The splash's failure screen: the wordmark, a headline and two plain lines.
pub fn boot_failure_screen() -> Vec<Quad> {
    let mut c = background();
    mark(&mut c, 1.);
    for (i, line) in BOOT_FAILURE_LABELS.iter().enumerate() {
        failure_line(&mut c, i, failure_line_y(i), line);
    }
    c.finish()
}

/// Wordmark only; the splash decodes it from this frame.
pub fn boot_mark_frame() -> Vec<Quad> {
    let mut c = background();
    mark(&mut c, 1.);
    c.finish()
}

/// One text line only, centred on `y`; the splash decodes it from this frame.
/// Failure lines use their own styles, everything else is a status line.
pub fn boot_label_frame(text: &str, y: f32) -> Vec<Quad> {
    let mut c = background();
    match BOOT_FAILURE_LABELS.iter().position(|line| *line == text) {
        Some(i) => failure_line(&mut c, i, y, text),
        None => status(&mut c, y, text, 1.),
    }
    c.finish()
}

/// Reveal the first UI frame. `remaining` runs from 1 (the boot screen, bar
/// full, identical to the splash's last frame) to 0 (the UI alone); no frame is
/// ever blank. The bar and status line fade out first; then the wordmark lifts
/// a few pixels and dissolves together with the cover, on an ease-in-out curve.
pub fn boot_transition(ui: Vec<Quad>, remaining: f32) -> Vec<Quad> {
    let t = 1. - remaining.clamp(0., 1.);
    let chrome = 1. - smooth(t / 0.4);
    let reveal = smooth((t - 0.2) / 0.8);
    let mut quads = with_overlay(ui, 1. - reveal);
    let mut c = Canvas::new();
    mark_at(&mut c, 1. - reveal, REVEAL_LIFT * reveal);
    if chrome > 0. {
        bar(&mut c, 1., chrome);
        status(&mut c, LABEL_Y, BOOT_FINAL_LABEL, chrome);
    }
    quads.extend(c.finish());
    quads
}

/// Cover `quads` with the background at `alpha` (0 transparent, 1 opaque).
pub fn with_overlay(mut quads: Vec<Quad>, alpha: f32) -> Vec<Quad> {
    let a = (alpha.clamp(0., 1.) * 255.).round() as u32;
    quads.push(Quad::rect(
        0.,
        0.,
        WIDTH,
        HEIGHT,
        (color::BG & 0xFFFF_FF00) | a,
    ));
    for q in &mut quads {
        if alpha >= 1. {
            q.focus_target = false;
        }
    }
    quads
}

/// What the player is doing once its data is saved.
pub fn closing_label(restart: bool, low_battery: bool) -> &'static str {
    if low_battery {
        "Battery Empty"
    } else if restart {
        "Restarting"
    } else {
        "Shutting down"
    }
}

/// Shutdown timeline at the UI's 34 ms cadence. The caller presents frames
/// `0..close_frame` (the UI recedes into the wordmark, "Saving your place"),
/// saves its data, then presents `close_frame..frames` and a black frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShutdownPlan {
    pub close_frame: usize,
    pub frames: usize,
    pub low_battery: bool,
}

const RECEDE_FRAMES: usize = 16;
const CLOSING_FRAMES: usize = 14;
const LAST_STEP_FRAMES: usize = 14;
const VANISH_FRAMES: usize = 8;
const DIM_FRAMES: usize = 14;
const LIGHT_FRAMES: usize = 8;
const LOW_RECEDE_FRAMES: usize = 8;
const LOW_HOLD_FRAMES: usize = 12;
const LOW_FADE_FRAMES: usize = 10;

pub const fn shutdown_plan(low_battery: bool) -> ShutdownPlan {
    if low_battery {
        // The battery is nearly empty: a shorter farewell that still says why.
        ShutdownPlan {
            close_frame: LOW_RECEDE_FRAMES,
            frames: LOW_RECEDE_FRAMES + LOW_HOLD_FRAMES + LOW_FADE_FRAMES,
            low_battery: true,
        }
    } else {
        ShutdownPlan {
            close_frame: RECEDE_FRAMES,
            frames: RECEDE_FRAMES
                + CLOSING_FRAMES
                + LAST_STEP_FRAMES
                + VANISH_FRAMES
                + DIM_FRAMES
                + LIGHT_FRAMES,
            low_battery: false,
        }
    }
}

/// Shrink `quads` about the screen centre.
fn recede(quads: &[Quad], scale: f32) -> Vec<Quad> {
    quads
        .iter()
        .map(|q| Quad {
            x: 240. + (q.x - 240.) * scale,
            y: 180. + (q.y - 180.) * scale,
            w: q.w * scale,
            h: q.h * scale,
            ..q.clone()
        })
        .collect()
}

/// The single point of light the screen ends on.
fn last_light(c: &mut Canvas, strength: f32) {
    c.glow(240., 180., 14., 14., fade(0xC8CDD7FF, 0.7 * strength));
    c.disc(240., 180., 2., fade(0xE6E9F0FF, strength));
}

fn low_battery_screen(c: &mut Canvas, opacity: f32) {
    let danger = fade(color::DANGER, opacity);
    c.rrect_outline(204., 124., 66., 34., 9., 2., danger);
    c.rect(272., 134., 5., 14., danger);
    c.rrect(210., 130., 8., 22., 3., danger);
    c.text_center(
        240.,
        206. + Face::S20.mid_offset(),
        "Battery Empty",
        Face::S20,
        fade(color::TEXT, opacity),
    );
    c.text_center(
        240.,
        232. + Face::R13.mid_offset(),
        "Your place is saved. Charge to continue.",
        Face::R13,
        fade(color::MUTED, opacity),
    );
}

/// One shutdown frame of `plan`. `ui` is the last UI frame; `closing` is the
/// label for the last step (see `closing_label`).
pub fn shutdown_frame(ui: &[Quad], frame: usize, closing: &str, plan: ShutdownPlan) -> Vec<Quad> {
    let frame = frame.min(plan.frames - 1);
    if frame < plan.close_frame {
        let t = smooth((frame + 1) as f32 / plan.close_frame as f32);
        let receded = recede(ui, lerp(1., 0.94, t));
        let mut quads = with_overlay(receded, t);
        let mut c = Canvas::new();
        if plan.low_battery {
            mark(&mut c, t);
            status(&mut c, LABEL_Y, SAVING_LABEL, t);
        } else {
            group(&mut c, 1., SAVING_LABEL, t);
        }
        quads.extend(c.finish());
        return quads;
    }
    let mut step = frame - plan.close_frame;
    let mut c = background();
    if plan.low_battery {
        if step < LOW_HOLD_FRAMES {
            let t = smooth((step + 1) as f32 / 4.);
            low_battery_screen(&mut c, t);
        } else {
            let t = 1. - smooth((step - LOW_HOLD_FRAMES + 1) as f32 / LOW_FADE_FRAMES as f32);
            low_battery_screen(&mut c, t);
        }
        return c.finish();
    }
    // 1: bar drains to 55 % ("Closing"); 2: to 14 % (closing label).
    if step < CLOSING_FRAMES {
        let t = smooth((step + 1) as f32 / CLOSING_FRAMES as f32);
        group(&mut c, lerp(1., 0.55, t), CLOSING_LABEL, 1.);
        return c.finish();
    }
    step -= CLOSING_FRAMES;
    if step < LAST_STEP_FRAMES {
        let t = smooth((step + 1) as f32 / LAST_STEP_FRAMES as f32);
        group(&mut c, lerp(0.55, 0.14, t), closing, 1.);
        return c.finish();
    }
    step -= LAST_STEP_FRAMES;
    if step < VANISH_FRAMES {
        // The bar and its label fade out; the wordmark stays lit.
        let t = 1. - smooth((step + 1) as f32 / VANISH_FRAMES as f32);
        mark(&mut c, 1.);
        bar(&mut c, 0.14, t);
        status(&mut c, LABEL_Y, closing, t);
        return c.finish();
    }
    step -= VANISH_FRAMES;
    if step < DIM_FRAMES {
        let t = 1. - smooth((step + 1) as f32 / DIM_FRAMES as f32);
        mark(&mut c, lerp(0.28, 1., t * t));
        return c.finish();
    }
    step -= DIM_FRAMES;
    let t = (step + 1) as f32 / LIGHT_FRAMES as f32;
    let strength = if t < 0.5 {
        smooth(t * 2.)
    } else {
        smooth((1. - t) * 2.)
    };
    last_light(&mut c, strength);
    c.finish()
}

/// The final frame before the backlight turns off.
pub fn black_frame() -> Vec<Quad> {
    vec![Quad::rect(0., 0., WIDTH, HEIGHT, color::BLACK)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shape(q: &Quad) -> (u32, u32, u32, u32, u32) {
        (
            q.x.to_bits(),
            q.y.to_bits(),
            q.w.to_bits(),
            q.h.to_bits(),
            q.color,
        )
    }

    #[test]
    fn phases_are_ordered_unique_and_user_facing() {
        assert_eq!(BOOT_PHASES[0].fill_permille, 0);
        assert_eq!(BOOT_PHASES.last().unwrap().fill_permille, 1000);
        assert_eq!(BOOT_PHASES.last().unwrap().token, "ready");
        for pair in BOOT_PHASES.windows(2) {
            assert!(pair[0].fill_permille < pair[1].fill_permille);
            assert_ne!(pair[0].token, pair[1].token);
        }
        let mut tokens = BOOT_PHASES.iter().map(|p| p.token).collect::<Vec<_>>();
        tokens.sort_unstable();
        tokens.dedup();
        assert_eq!(tokens.len(), BOOT_PHASES.len());
        let labels = BOOT_PHASES
            .iter()
            .map(|p| p.label)
            .chain(BOOT_FAILURE_LABELS)
            .chain([SAVING_LABEL, CLOSING_LABEL, closing_label(false, false)])
            .chain([closing_label(true, false), closing_label(false, true)]);
        for text in labels {
            let lower = text.to_ascii_lowercase();
            for technical in [
                "switch_root",
                "systemd",
                "drm",
                "alsa",
                "kms",
                "mount",
                "/",
                "_",
                ".ko",
                "init",
                "fsck",
                "%",
            ] {
                assert!(!lower.contains(technical), "{text:?} exposes {technical:?}");
            }
            assert!(text.chars().count() <= 26, "{text:?} is not short");
        }
        // Progress text never carries a number; only the advice names seconds.
        for p in BOOT_PHASES {
            assert!(!p.label.chars().any(|c| c.is_ascii_digit()));
        }
    }

    #[test]
    fn bar_geometry_is_centered_and_hairline() {
        assert_eq!(BAR_X * 2. + BAR_W, 480.);
        let quads = boot_screen(0.5, "Starting audio");
        let fill = quads
            .iter()
            .find(|q| q.color == BAR_FILL && q.h == BAR_H)
            .expect("bar fill");
        assert_eq!((fill.x, fill.w), (BAR_X, BAR_W / 2.));
        assert!(fill.h <= 2., "hairline bar");
        let red = (BAR_FILL >> 24) & 0xFF;
        let blue = (BAR_FILL >> 8) & 0xFF;
        assert!(red > 0xE0 && blue > 0xD0, "near-white fill");
    }

    #[test]
    fn bar_fills_left_to_right_and_never_exceeds_its_track() {
        let widths = [0., 0.25, 0.6, 1., 7.]
            .into_iter()
            .map(|f| {
                boot_screen(f, "x")
                    .iter()
                    .find(|q| q.color == BAR_FILL && q.h == BAR_H)
                    .map_or(0., |q| {
                        assert_eq!(q.x, BAR_X, "fills from the left edge");
                        q.w
                    })
            })
            .collect::<Vec<_>>();
        assert_eq!(widths, [0., 31., 74., 124., 124.]);
    }

    fn glyph_alpha(quads: &[Quad], from: usize) -> u32 {
        quads
            .iter()
            .filter(|q| matches!(q.tex, reborn_graphics::Tex::Font(_)))
            .nth(from)
            .map_or(0, |q| q.color & 0xFF)
    }

    #[test]
    fn reveal_starts_as_the_boot_screen_and_ends_as_the_ui() {
        let ui = vec![Quad::rect(0., 0., 480., 360., color::GLASS_STRONG)];
        let first = boot_transition(ui.clone(), 1.);
        let overlay = first.iter().find(|q| q.w == 480. && q.color == color::BG);
        assert!(overlay.is_some(), "opaque cover on the first reveal frame");
        assert!(first.iter().all(|q| !q.focus_target));
        // The first reveal frame draws the same group as the splash's last.
        let screen = boot_screen(1., BOOT_FINAL_LABEL);
        let first_shapes = first.iter().map(shape).collect::<Vec<_>>();
        assert!(screen
            .iter()
            .skip(1)
            .all(|q| first_shapes.contains(&shape(q))));
        let cover = |quads: &[Quad]| {
            quads
                .iter()
                .find(|q| q.w == 480. && q.color & 0xFFFF_FF00 == color::BG & 0xFFFF_FF00)
                .map_or(0, |q| q.color & 0xFF)
        };
        // "reborn" is the first six glyphs, the status line follows.
        let (mut cover_before, mut mark_before, mut chrome_before) = (256, 256, 256);
        let mut lift_before = 0.;
        for step in (1..=BOOT_FADE_FRAMES).rev() {
            let quads = boot_transition(ui.clone(), f32::from(step) / f32::from(BOOT_FADE_FRAMES));
            let (c, m, s) = (
                cover(&quads),
                glyph_alpha(&quads, 0),
                glyph_alpha(&quads, 6),
            );
            assert!(c <= cover_before && m <= mark_before && s <= chrome_before);
            (cover_before, mark_before, chrome_before) = (c, m, s);
            let y = quads
                .iter()
                .find(|q| matches!(q.tex, reborn_graphics::Tex::Font(_)))
                .unwrap()
                .y;
            let lift = first
                .iter()
                .find(|q| matches!(q.tex, reborn_graphics::Tex::Font(_)))
                .unwrap()
                .y
                - y;
            assert!(
                (0. ..=REVEAL_LIFT + 0.01).contains(&lift) && lift >= lift_before,
                "wordmark only rises, by at most {REVEAL_LIFT} px: {lift}"
            );
            lift_before = lift;
        }
        let mid = boot_transition(ui.clone(), 0.55);
        assert_eq!(glyph_alpha(&mid, 6), 0, "status already faded");
        assert!(glyph_alpha(&mid, 0) > 180 && cover(&mid) > 180);
        let last = boot_transition(ui, 1. / f32::from(BOOT_FADE_FRAMES));
        assert!(cover(&last) < 30 && glyph_alpha(&last, 0) < 30);
        assert_eq!(glyph_alpha(&last, 6), 0);
    }

    #[test]
    fn shutdown_is_saving_then_closing_and_ends_dark() {
        let plan = shutdown_plan(false);
        let ui = vec![Quad::rect(0., 0., 480., 360., color::GLASS_STRONG)];
        let fill = |frame| {
            shutdown_frame(&ui, frame, "Shutting down", plan)
                .iter()
                .find(|q| q.color & 0xFFFF_FF00 == BAR_FILL & 0xFFFF_FF00 && q.h == BAR_H)
                .map_or(0., |q| q.w)
        };
        // The bar is full while saving, then drains monotonically.
        assert_eq!(fill(plan.close_frame - 1), BAR_W);
        let mut last = BAR_W + 1.;
        for frame in plan.close_frame..plan.close_frame + CLOSING_FRAMES + LAST_STEP_FRAMES {
            assert!(fill(frame) <= last, "frame {frame}");
            last = fill(frame);
        }
        assert!(last < BAR_W * 0.2 && last > 0.);
        // The wordmark outlives the bar, then everything dims to a point of light.
        let after_bar = plan.close_frame + CLOSING_FRAMES + LAST_STEP_FRAMES + VANISH_FRAMES;
        assert_eq!(fill(after_bar), 0.);
        assert!(glyph_alpha(&shutdown_frame(&ui, after_bar, "x", plan), 0) > 0);
        let end = shutdown_frame(&ui, plan.frames - 1, "x", plan);
        assert_eq!(
            end.iter()
                .filter(|q| matches!(q.tex, reborn_graphics::Tex::Font(_)))
                .count(),
            0
        );
        assert_eq!(black_frame()[0].color, color::BLACK);
        // Out-of-range frames clamp to the last real one.
        let clamped = shutdown_frame(&ui, 999, "x", plan);
        assert_eq!(
            clamped.iter().map(shape).collect::<Vec<_>>(),
            end.iter().map(shape).collect::<Vec<_>>()
        );
    }

    #[test]
    fn low_battery_farewell_is_short_and_says_why() {
        let normal = shutdown_plan(false);
        let low = shutdown_plan(true);
        assert!(low.frames < normal.frames && low.close_frame < normal.close_frame);
        let ui = vec![Quad::rect(0., 0., 480., 360., color::GLASS_STRONG)];
        let hold = shutdown_frame(&ui, low.close_frame + 6, "Battery Empty", low);
        let glyphs = hold
            .iter()
            .filter(|q| matches!(q.tex, reborn_graphics::Tex::Font(_)))
            .count();
        assert!(glyphs > "Battery Empty".len());
        assert!(hold
            .iter()
            .any(|q| q.color & 0xFFFF_FF00 == color::DANGER & 0xFFFF_FF00));
    }

    #[test]
    fn closing_labels_follow_the_real_action() {
        assert_eq!(closing_label(false, false), "Shutting down");
        assert_eq!(closing_label(true, false), "Restarting");
        assert_eq!(closing_label(true, true), "Battery Empty");
    }
}
