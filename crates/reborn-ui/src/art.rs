//! Cover-art helpers shared by the app and the previews: the accent colour a
//! cover lends the UI, library thumbnails and rounded artwork corners.
use reborn_graphics::THUMB_SIZE;

/// Corner radius as a fraction of the picture's edge, baked into the pixels.
const CORNER: f32 = 0.085;

/// Accent colour for an RGBA cover: the most vivid hue, lifted so text
/// in it stays readable on the near-black background. Dull or grey covers get
/// a calm cool white instead of a muddy tint.
pub fn accent_from_rgba(pixels: &[u8]) -> u32 {
    const BUCKETS: usize = 12;
    let mut weight = [0f32; BUCKETS];
    let mut sum = [[0f32; 3]; BUCKETS];
    for px in pixels.as_chunks::<4>().0.iter().step_by(3) {
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

/// A library thumbnail from a `THUMB_SIZE` RGBA cover: opaque, with rounded corners.
pub fn thumbnail(cover: &[u8]) -> Option<Vec<u8>> {
    if cover.len() != THUMB_SIZE * THUMB_SIZE * 4 {
        return None;
    }
    let mut out = cover.to_vec();
    out.as_chunks_mut::<4>()
        .0
        .iter_mut()
        .for_each(|p| p[3] = 255);
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
    fn thumbnails_are_opaque_squares_with_clear_corners() {
        let n = THUMB_SIZE;
        let mut cover: Vec<u8> = (0..n * n).flat_map(|_| [200, 40, 40, 90]).collect();
        let thumb = thumbnail(&cover).unwrap();
        assert_eq!(thumb.len(), n * n * 4);
        assert_eq!(thumb[3], 0, "top-left corner is transparent");
        let centre = (n / 2 * n + n / 2) * 4;
        assert_eq!(&thumb[centre..centre + 4], &[200, 40, 40, 255]);
        cover.truncate(10);
        assert!(thumbnail(&cover).is_none());
        let mut art = flat(1, 2, 3);
        round_corners(&mut art, 160);
        assert_eq!(art[3], 0);
        assert_eq!(art[(80 * 160 + 80) * 4 + 3], 255);
    }
}

/// A station logo as a square opaque tile as large as `fitted`: the whole
/// logo centred in a square with transparent margins (`logo_image`). A logo
/// with a plain background of its own fills the tile in that colour; a
/// transparent logo sits at 80 % on a light or dark tile, whichever it
/// stands out on.
pub fn logo_tile(fitted: &[u8]) -> Option<Vec<u8>> {
    let n = (fitted.len() / 4).isqrt();
    if n == 0 || fitted.len() != n * n * 4 {
        return None;
    }
    let alpha = |x: usize, y: usize| fitted[(y * n + x) * 4 + 3];
    // The logo's own extent (its non-transparent pixels).
    let (mut x0, mut y0, mut x1, mut y1) = (n, n, 0, 0);
    for y in 0..n {
        for x in 0..n {
            if alpha(x, y) > 16 {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    if x0 > x1 {
        return None;
    }
    // Its border: opaque and one colour means a background of its own.
    let mut border = vec![];
    for x in x0..=x1 {
        border.push((x, y0));
        border.push((x, y1));
    }
    for y in y0..=y1 {
        border.push((x0, y));
        border.push((x1, y));
    }
    let opaque: Vec<[f32; 3]> = border
        .iter()
        .filter(|&&(x, y)| alpha(x, y) > 230)
        .map(|&(x, y)| {
            let i = (y * n + x) * 4;
            [fitted[i] as f32, fitted[i + 1] as f32, fitted[i + 2] as f32]
        })
        .collect();
    let mean = |v: &[[f32; 3]]| {
        let n = v.len().max(1) as f32;
        let mut m = [0f32; 3];
        for p in v {
            for c in 0..3 {
                m[c] += p[c] / n;
            }
        }
        m
    };
    // The most common border colour (16 levels per channel) and how much of
    // the border is close to it.
    let mut buckets = std::collections::HashMap::new();
    for p in &opaque {
        let key = (p[0] as u8 >> 4, p[1] as u8 >> 4, p[2] as u8 >> 4);
        *buckets.entry(key).or_insert(0usize) += 1;
    }
    let common = buckets.into_iter().max_by_key(|(_, n)| *n).map(|(k, _)| k);
    let near: Vec<[f32; 3]> = opaque
        .iter()
        .copied()
        .filter(|p| common == Some((p[0] as u8 >> 4, p[1] as u8 >> 4, p[2] as u8 >> 4)))
        .collect();
    let edge = mean(&near);
    let close = opaque
        .iter()
        .filter(|p| (0..3).all(|c| (p[c] - edge[c]).abs() < 24.))
        .count();
    let rectangle = opaque.len() * 10 >= border.len() * 9;
    let plain = rectangle && close * 100 >= border.len() * 85;
    let (background, scale) = if plain {
        (edge, 1.0)
    } else if rectangle {
        // A picture (a photo, a square artwork): it fills the tile; beside a
        // wide or tall one, its own colours darkened.
        let all = mean(&opaque);
        (all.map(|c| c * 0.35 + 8.), 1.0)
    } else {
        // Light logos on a dark tile, dark ones on a light tile.
        let (mut lum, mut weight) = (0f32, 0f32);
        for i in (0..n * n * 4).step_by(4) {
            let a = fitted[i + 3] as f32 / 255.;
            lum += a
                * (0.2126 * fitted[i] as f32
                    + 0.7152 * fitted[i + 1] as f32
                    + 0.0722 * fitted[i + 2] as f32);
            weight += a;
        }
        let light = lum / weight.max(1.) > 150.;
        (
            if light {
                [0x20 as f32, 0x23 as f32, 0x2B as f32]
            } else {
                [0xF3 as f32, 0xF3 as f32, 0xF5 as f32]
            },
            0.8,
        )
    };
    let mut out = vec![0u8; n * n * 4];
    let centre = (n as f32 - 1.) / 2.;
    for y in 0..n {
        for x in 0..n {
            // Bilinear sample of the logo shrunk around the centre.
            let sx = centre + (x as f32 - centre) / scale;
            let sy = centre + (y as f32 - centre) / scale;
            let mut px = [0f32; 4];
            if (0.0..(n - 1) as f32).contains(&sx) && (0.0..(n - 1) as f32).contains(&sy) {
                let (ix, iy) = (sx as usize, sy as usize);
                let (fx, fy) = (sx - ix as f32, sy - iy as f32);
                for (dx, dy, w) in [
                    (0, 0, (1. - fx) * (1. - fy)),
                    (1, 0, fx * (1. - fy)),
                    (0, 1, (1. - fx) * fy),
                    (1, 1, fx * fy),
                ] {
                    let i = ((iy + dy) * n + ix + dx) * 4;
                    let a = fitted[i + 3] as f32;
                    for c in 0..3 {
                        px[c] += w * fitted[i + c] as f32 * a / 255.;
                    }
                    px[3] += w * a;
                }
            }
            let a = px[3] / 255.;
            let o = (y * n + x) * 4;
            for c in 0..3 {
                out[o + c] = (px[c] + background[c] * (1. - a)).round().clamp(0., 255.) as u8;
            }
            out[o + 3] = 255;
        }
    }
    Some(out)
}

#[cfg(test)]
mod logo_tests {
    use super::*;
    const N: usize = 160;
    fn fitted(f: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        (0..N * N).flat_map(|i| f(i % N, i / N)).collect()
    }
    #[test]
    fn a_logo_with_its_own_background_fills_the_tile_in_that_colour() {
        // A wide white logo with a red mark, margins transparent above and below.
        let logo = fitted(|x, y| match y {
            0..40 | 120..160 => [0, 0, 0, 0],
            _ if (70..90).contains(&x) => [200, 0, 0, 255],
            _ => [255, 255, 255, 255],
        });
        let tile = logo_tile(&logo).unwrap();
        assert_eq!(
            &tile[0..4],
            &[255, 255, 255, 255],
            "margins take the logo's white"
        );
        let mid = (80 * N + 80) * 4;
        assert_eq!(&tile[mid..mid + 3], &[200, 0, 0]);
    }
    #[test]
    fn a_picture_logo_fills_the_tile() {
        // A square photo-like logo: no plain border, but opaque to its edges.
        let photo = fitted(|x, y| [(x * 255 / 160) as u8, (y * 255 / 160) as u8, 90, 255]);
        let tile = logo_tile(&photo).unwrap();
        assert_eq!(&tile[0..3], &photo[0..3], "drawn to the corner, not shrunk");
    }
    #[test]
    fn a_transparent_logo_sits_on_a_contrasting_tile() {
        let disc = |x: usize, y: usize| (x as f32 - 80.).hypot(y as f32 - 80.) < 40.;
        let white = fitted(|x, y| {
            if disc(x, y) {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        assert_eq!(&logo_tile(&white).unwrap()[0..3], &[0x20, 0x23, 0x2B]);
        let black = fitted(|x, y| {
            if disc(x, y) {
                [10, 10, 10, 255]
            } else {
                [0, 0, 0, 0]
            }
        });
        assert_eq!(&logo_tile(&black).unwrap()[0..3], &[0xF3, 0xF3, 0xF5]);
        assert!(
            logo_tile(&fitted(|_, _| [0, 0, 0, 0])).is_none(),
            "nothing to show"
        );
    }
}
