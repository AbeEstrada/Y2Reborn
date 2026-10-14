//! The skin atlas: one RGBA texture holding a skin's pictures (with a
//! replicated one-texel edge, so stretched and nine-sliced pictures never pick
//! up their neighbours) and its fonts' glyphs (white, coverage in alpha, so a
//! quad's colour tints them like the built-in glyph pages).
use reborn_graphics::SKIN_ATLAS;
use std::collections::HashMap;

/// Shelf packer over a square atlas.
struct Packer {
    x: usize,
    y: usize,
    shelf: usize,
}

impl Packer {
    fn place(&mut self, w: usize, h: usize) -> Option<(usize, usize)> {
        if w > SKIN_ATLAS || h > SKIN_ATLAS {
            return None;
        }
        if self.x + w > SKIN_ATLAS {
            self.x = 0;
            self.y += self.shelf;
            self.shelf = 0;
        }
        if self.y + h > SKIN_ATLAS {
            return None;
        }
        let at = (self.x, self.y);
        self.x += w;
        self.shelf = self.shelf.max(h);
        Some(at)
    }
    fn height(&self) -> usize {
        self.y + self.shelf
    }
}

pub struct Atlas {
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    packer: Packer,
}

impl Default for Atlas {
    fn default() -> Self {
        Self {
            rgba: vec![0; SKIN_ATLAS * SKIN_ATLAS * 4],
            width: SKIN_ATLAS,
            height: SKIN_ATLAS,
            packer: Packer {
                x: 0,
                y: 0,
                shelf: 0,
            },
        }
    }
}

impl Atlas {
    /// Copy a `w` x `h` picture in with a replicated edge; returns its texel
    /// rectangle without the edge.
    pub fn picture(&mut self, w: usize, h: usize, rgba: &[u8]) -> Result<[u16; 4], String> {
        if rgba.len() != w * h * 4 || w == 0 || h == 0 {
            return Err("picture size".into());
        }
        let (x0, y0) = self
            .packer
            .place(w + 2, h + 2)
            .ok_or("the pictures and fonts do not fit the 1024 x 1024 skin atlas")?;
        for y in 0..h + 2 {
            let sy = y.saturating_sub(1).min(h - 1);
            for x in 0..w + 2 {
                let sx = x.saturating_sub(1).min(w - 1);
                let from = (sy * w + sx) * 4;
                let to = ((y0 + y) * SKIN_ATLAS + x0 + x) * 4;
                self.rgba[to..to + 4].copy_from_slice(&rgba[from..from + 4]);
            }
        }
        Ok([x0 as u16 + 1, y0 as u16 + 1, w as u16, h as u16])
    }

    /// Reserve a transparent cell with a one-texel clear border and return
    /// its inner origin; the caller writes coverage with `coverage`.
    fn cell(&mut self, w: usize, h: usize) -> Result<(usize, usize), String> {
        let (x, y) = self
            .packer
            .place(w + 2, h + 2)
            .ok_or("the pictures and fonts do not fit the 1024 x 1024 skin atlas")?;
        Ok((x + 1, y + 1))
    }

    fn coverage(&mut self, x: usize, y: usize, a: u8) {
        let at = (y * SKIN_ATLAS + x) * 4;
        self.rgba[at..at + 4].copy_from_slice(&[255, 255, 255, a]);
    }

    /// Drop the unused rows below the last shelf (smaller upload).
    pub fn finish(&mut self) {
        let used = self.packer.height().clamp(1, SKIN_ATLAS);
        let height = used.next_power_of_two().min(SKIN_ATLAS);
        self.rgba.truncate(SKIN_ATLAS * height * 4);
        self.height = height;
    }
}

/// One rasterised glyph: atlas rectangle, offset of its top-left from the pen
/// on the baseline, and its advance (all in pixels).
#[derive(Clone, Copy, Debug)]
pub struct Glyph {
    pub src: [u16; 4],
    pub left: i16,
    pub top: i16,
    pub advance: f32,
}

/// A TrueType/OpenType font rasterised at one pixel size.
#[derive(Debug)]
pub struct Font {
    pub px: f32,
    /// Baseline to the vertical middle of a line (ascender/descender midpoint).
    pub mid: f32,
    /// Distance between baselines.
    pub line: f32,
    pub glyphs: HashMap<char, Glyph>,
}

/// The characters every skin font carries when the font has them: Latin,
/// Latin-1, Latin Extended-A and common typography. Anything else falls back
/// to Reborn's own fonts.
pub fn charset() -> impl Iterator<Item = char> {
    (0x20u32..0x7f)
        .chain(0xa0..0x180)
        .chain([
            0x2009, 0x2013, 0x2014, 0x2018, 0x2019, 0x201c, 0x201d, 0x2022, 0x2026, 0x20ac, 0x2122,
            0x2190, 0x2192, 0x25b6, 0xfffd,
        ])
        .filter_map(char::from_u32)
}

struct Outline<'a> {
    raster: &'a mut ab_glyph_rasterizer::Rasterizer,
    scale: f32,
    /// Font-unit origin of the cell (left, top).
    x0: f32,
    y0: f32,
    start: ab_glyph_rasterizer::Point,
    last: ab_glyph_rasterizer::Point,
}

impl Outline<'_> {
    fn point(&self, x: f32, y: f32) -> ab_glyph_rasterizer::Point {
        ab_glyph_rasterizer::point((x - self.x0) * self.scale, (self.y0 - y) * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for Outline<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.start = self.point(x, y);
        self.last = self.start;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.point(x, y);
        self.raster.draw_line(self.last, p);
        self.last = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.point(x1, y1), self.point(x, y));
        self.raster.draw_quad(self.last, c, p);
        self.last = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b, p) = (self.point(x1, y1), self.point(x2, y2), self.point(x, y));
        self.raster.draw_cubic(self.last, a, b, p);
        self.last = p;
    }
    fn close(&mut self) {
        if self.last != self.start {
            self.raster.draw_line(self.last, self.start);
        }
        self.last = self.start;
    }
}

/// Rasterise `charset()` from `data` at `px` into the atlas.
pub fn rasterise(atlas: &mut Atlas, data: &[u8], px: f32) -> Result<Font, String> {
    let face = ttf_parser::Face::parse(data, 0).map_err(|e| format!("not a usable font ({e})"))?;
    let units = f32::from(face.units_per_em().max(1));
    let scale = px / units;
    let ascender = f32::from(face.ascender()) * scale;
    let descender = f32::from(face.descender()) * scale;
    let gap = f32::from(face.line_gap()) * scale;
    let mut glyphs = HashMap::new();
    let mut raster = ab_glyph_rasterizer::Rasterizer::new(1, 1);
    for ch in charset() {
        let Some(id) = face.glyph_index(ch) else {
            continue;
        };
        let advance = f32::from(face.glyph_hor_advance(id).unwrap_or(0)) * scale;
        let Some(bbox) = face.glyph_bounding_box(id) else {
            // Blank glyphs (space) only advance.
            glyphs.insert(
                ch,
                Glyph {
                    src: [0; 4],
                    left: 0,
                    top: 0,
                    advance,
                },
            );
            continue;
        };
        let left = (f32::from(bbox.x_min) * scale).floor();
        let top = (f32::from(bbox.y_max) * scale).ceil();
        let right = (f32::from(bbox.x_max) * scale).ceil();
        let bottom = (f32::from(bbox.y_min) * scale).floor();
        let (w, h) = ((right - left) as usize + 1, (top - bottom) as usize + 1);
        if w > 256 || h > 256 {
            return Err(format!("glyph {ch:?} is too large at {px} px"));
        }
        raster.reset(w, h);
        let mut outline = Outline {
            raster: &mut raster,
            scale,
            x0: left / scale,
            y0: top / scale,
            start: ab_glyph_rasterizer::point(0., 0.),
            last: ab_glyph_rasterizer::point(0., 0.),
        };
        face.outline_glyph(id, &mut outline);
        let (x0, y0) = atlas.cell(w, h)?;
        raster.for_each_pixel_2d(|x, y, a| {
            let a = (a.clamp(0., 1.) * 255.).round() as u8;
            if a > 0 {
                atlas.coverage(x0 + x as usize, y0 + y as usize, a);
            }
        });
        glyphs.insert(
            ch,
            Glyph {
                src: [x0 as u16, y0 as u16, w as u16, h as u16],
                left: left as i16,
                top: top as i16,
                advance,
            },
        );
    }
    if !glyphs.contains_key(&'a') {
        return Err("the font has no Latin letters".into());
    }
    Ok(Font {
        px,
        mid: ((ascender + descender) / 2.).round(),
        line: (ascender - descender + gap).round().max(px),
        glyphs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pictures_keep_their_edge_and_overflow_is_refused() {
        let mut atlas = Atlas::default();
        let red = [255u8, 0, 0, 255].repeat(4);
        let src = atlas.picture(2, 2, &red).unwrap();
        assert_eq!(src, [1, 1, 2, 2]);
        // The replicated edge around the picture is red too.
        assert_eq!(&atlas.rgba[0..4], &[255, 0, 0, 255]);
        let big = vec![0u8; 1000 * 600 * 4];
        assert!(atlas.picture(1000, 600, &big).is_ok());
        assert!(atlas.picture(1000, 600, &big).is_err());
        atlas.finish();
        assert_eq!(atlas.height, 1024);
    }
}
