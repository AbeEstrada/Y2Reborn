//! Cover-art helpers shared by the app and the previews: the accent colour a
//! cover lends the UI, library thumbnails and rounded artwork corners.
use reborn_graphics::THUMB_SIZE;

/// Corner radius as a fraction of the picture's edge, baked into the pixels.
const CORNER: f32 = 0.085;

/// Accent colour for a 160x160 RGBA cover: the most vivid hue, lifted so text
/// in it stays readable on the near-black background. Dull or grey covers get
/// a calm cool white instead of a muddy tint.
pub fn accent_from_rgba(pixels: &[u8]) -> u32 {
    const BUCKETS: usize = 12;
    let mut weight = [0f32; BUCKETS];
    let mut sum = [[0f32; 3]; BUCKETS];
    for px in pixels.chunks_exact(4).step_by(3) {
        if px[3] < 128 {
            continue;
        }
        let (r, g, b) = (f32::from(px[0]), f32::from(px[1]), f32::from(px[2]));
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let value = max / 255.;
        let sat = if max == 0. { 0. } else { (max - min) / max };
        if value < 0.25 || sat < 0.18 {
            continue;
        }
        let hue = hue_of(r, g, b, max, min);
        let bucket = ((hue / 360.) * BUCKETS as f32) as usize % BUCKETS;
        let w = sat * sat * value;
        weight[bucket] += w;
        for (i, c) in [r, g, b].into_iter().enumerate() {
            sum[bucket][i] += c * w;
        }
    }
    let total: f32 = weight.iter().sum();
    let (best, best_weight) = weight
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(i, w)| (i, *w))
        .unwrap_or((0, 0.));
    let samples = (pixels.len() / 12).max(1) as f32;
    if best_weight <= 0. || total / samples < 0.02 {
        return 0xBECDE1FF;
    }
    let mean = |i: usize| sum[best][i] / best_weight;
    lift(mean(0), mean(1), mean(2))
}

fn hue_of(r: f32, g: f32, b: f32, max: f32, min: f32) -> f32 {
    let d = max - min;
    if d == 0. {
        return 0.;
    }
    let h = if max == r {
        ((g - b) / d).rem_euclid(6.)
    } else if max == g {
        (b - r) / d + 2.
    } else {
        (r - g) / d + 4.
    };
    h * 60.
}

/// Raise saturation and brightness into a band where accent text reads well.
fn lift(r: f32, g: f32, b: f32) -> u32 {
    let max = r.max(g).max(b).max(1.);
    let min = r.min(g).min(b);
    let sat = ((max - min) / max).clamp(0.5, 0.78);
    let value = (max / 255.).clamp(0.9, 1.0);
    let top = value * 255.;
    let bottom = top * (1. - sat);
    let scale = |c: f32| {
        let t = ((c - min) / (max - min).max(1.)).clamp(0., 1.);
        (bottom + (top - bottom) * t).round() as u32
    };
    scale(r) << 24 | scale(g) << 16 | scale(b) << 8 | 0xFF
}

/// Coverage (0..=1) of a pixel inside a rounded square of edge `n`.
fn corner_coverage(x: usize, y: usize, n: usize, radius: f32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let n = n as f32;
    let cx = px.clamp(radius, n - radius);
    let cy = py.clamp(radius, n - radius);
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
    (radius - d + 0.5).clamp(0., 1.)
}

/// Give a square RGBA picture rounded corners by clearing alpha outside them.
pub fn round_corners(rgba: &mut [u8], edge: usize) {
    let radius = edge as f32 * CORNER;
    for y in 0..edge {
        for x in 0..edge {
            if x as f32 > radius
                && x as f32 + radius < edge as f32
                && y as f32 > radius
                && y as f32 + radius < edge as f32
            {
                continue;
            }
            let i = (y * edge + x) * 4 + 3;
            rgba[i] = (f32::from(rgba[i]) * corner_coverage(x, y, edge, radius)).round() as u8;
        }
    }
}

/// A 96x96 RGBA thumbnail (box-filtered) of a 160x160 RGBA cover, with rounded corners.
pub fn thumbnail(rgba160: &[u8]) -> Option<Vec<u8>> {
    const SRC: usize = 160;
    if rgba160.len() != SRC * SRC * 4 {
        return None;
    }
    let mut out = vec![0u8; THUMB_SIZE * THUMB_SIZE * 4];
    let scale = SRC as f32 / THUMB_SIZE as f32;
    for y in 0..THUMB_SIZE {
        for x in 0..THUMB_SIZE {
            let (x0, x1) = (
                (x as f32 * scale) as usize,
                (((x + 1) as f32 * scale).ceil() as usize).min(SRC),
            );
            let (y0, y1) = (
                (y as f32 * scale) as usize,
                (((y + 1) as f32 * scale).ceil() as usize).min(SRC),
            );
            let mut acc = [0u32; 4];
            let mut count = 0u32;
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let i = (sy * SRC + sx) * 4;
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += u32::from(rgba160[i + c]);
                    }
                    count += 1;
                }
            }
            let o = (y * THUMB_SIZE + x) * 4;
            for c in 0..4 {
                out[o + c] = (acc[c] / count.max(1)) as u8;
            }
            out[o + 3] = 255;
        }
    }
    round_corners(&mut out, THUMB_SIZE);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(r: u8, g: u8, b: u8) -> Vec<u8> {
        (0..160 * 160).flat_map(|_| [r, g, b, 255]).collect()
    }

    #[test]
    fn accent_follows_the_cover_and_stays_readable() {
        let orange = accent_from_rgba(&flat(210, 110, 40));
        let (r, g, b) = (
            (orange >> 24) as u8,
            (orange >> 16) as u8,
            (orange >> 8) as u8,
        );
        assert!(r > g && g > b, "orange stays orange: {orange:08x}");
        assert!(r >= 220, "lifted bright enough for text");
        let teal = accent_from_rgba(&flat(30, 120, 130));
        assert!(
            (teal >> 8) as u8 > (teal >> 24) as u8,
            "teal stays blue-green"
        );
    }

    #[test]
    fn dull_covers_get_the_neutral_accent() {
        assert_eq!(accent_from_rgba(&flat(110, 112, 114)), 0xBECDE1FF);
        assert_eq!(accent_from_rgba(&flat(8, 8, 8)), 0xBECDE1FF);
        assert_eq!(accent_from_rgba(&[]), 0xBECDE1FF);
    }

    #[test]
    fn thumbnails_are_96_square_with_clear_corners() {
        let thumb = thumbnail(&flat(200, 40, 40)).unwrap();
        assert_eq!(thumb.len(), 96 * 96 * 4);
        assert_eq!(thumb[3], 0, "top-left corner is transparent");
        let centre = (48 * 96 + 48) * 4;
        assert_eq!(&thumb[centre..centre + 4], &[200, 40, 40, 255]);
        assert!(thumbnail(&[0; 10]).is_none());
        let mut art = flat(1, 2, 3);
        round_corners(&mut art, 160);
        assert_eq!(art[3], 0);
        assert_eq!(art[(80 * 160 + 80) * 4 + 3], 255);
    }
}
