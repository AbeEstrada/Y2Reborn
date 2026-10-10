//! Text metrics over the exact-size Open Sans glyph sets (`fontdata`).
//!
//! Every `Face` is one weight at one pixel size, rendered once at build time,
//! so glyphs are drawn 1:1 on the pixel grid. Characters a face lacks fall back
//! to the 24 px page, scaled; characters nobody has become U+FFFD.
use crate::fontdata::{self, Glyph, SetData};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Face {
    R11,
    R13,
    R15,
    R17,
    L23,
    L44,
    S10,
    S13,
    S15,
    S17,
    S20,
    S25,
    S28,
    S36,
}

pub const FALLBACK_PX: f32 = 24.;
pub const FALLBACK_CELL: f32 = 32.;
pub const FALLBACK_PAD: f32 = 2.;
pub const FALLBACK_ASCENT: f32 = 25.;

impl Face {
    pub fn set(self) -> &'static SetData {
        use fontdata::*;
        match self {
            Self::R11 => &R11,
            Self::R13 => &R13,
            Self::R15 => &R15,
            Self::R17 => &R17,
            Self::L23 => &L23,
            Self::L44 => &L44,
            Self::S10 => &S10,
            Self::S13 => &S13,
            Self::S15 => &S15,
            Self::S17 => &S17,
            Self::S20 => &S20,
            Self::S25 => &S25,
            Self::S28 => &S28,
            Self::S36 => &S36,
        }
    }
    pub fn px(self) -> f32 {
        f32::from(self.set().size)
    }
    /// Pixels from the baseline to the vertical middle of a line of this
    /// face (the midpoint of the ascender and descender, as the design
    /// mock-ups centre text).
    pub fn mid_offset(self) -> f32 {
        (self.px() * 0.388).round()
    }
}

/// Where a character is drawn from.
#[derive(Clone, Copy, Debug)]
pub enum Source {
    /// An exact-size glyph in `face`'s page.
    Exact(&'static Glyph),
    /// A scaled 24 px glyph from the fallback page.
    Fallback(&'static Glyph),
}

fn find(glyphs: &'static [Glyph], ch: char) -> Option<&'static Glyph> {
    glyphs
        .binary_search_by_key(&(ch as u32), |g| g.0)
        .ok()
        .map(|i| &glyphs[i])
}

pub fn lookup(face: Face, ch: char) -> Option<Source> {
    if let Some(g) = find(face.set().glyphs, ch) {
        return Some(Source::Exact(g));
    }
    find(fontdata::FALLBACK, ch)
        .or_else(|| find(fontdata::FALLBACK, '\u{FFFD}'))
        .map(Source::Fallback)
}

/// Pen advance of `ch` in pixels.
pub fn advance(face: Face, ch: char) -> f32 {
    match lookup(face, ch) {
        Some(Source::Exact(g)) => f32::from(g.3) / 64.,
        Some(Source::Fallback(g)) => f32::from(g.3) / 64. * face.px() / FALLBACK_PX,
        None => 0.,
    }
}

pub fn width_tracked(face: Face, text: &str, tracking: f32) -> f32 {
    let mut total = 0f32;
    let mut count = 0f32;
    for ch in text.chars().filter(|c| !c.is_control()) {
        total += advance(face, ch);
        count += 1.;
    }
    total + tracking * (count - 1.).max(0.)
}

pub fn width(face: Face, text: &str) -> f32 {
    width_tracked(face, text, 0.)
}

/// `text` cut to `max_width` with a trailing ellipsis when it does not fit.
pub fn fit(face: Face, text: &str, max_width: f32) -> String {
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(1024)
        .collect();
    if width(face, &clean) <= max_width {
        return clean;
    }
    let ellipsis = advance(face, '…');
    let mut out = String::new();
    let mut used = ellipsis;
    for ch in clean.chars() {
        used += advance(face, ch);
        if used > max_width {
            break;
        }
        out.push(ch);
    }
    if max_width >= ellipsis {
        out.push('…');
    }
    out
}

/// Split `text` into at most `lines` lines of at most `max_width`; the last
/// line is cut with an ellipsis when text remains.
pub fn wrap(face: Face, text: &str, max_width: f32, lines: usize) -> Vec<String> {
    let mut out = vec![];
    let mut remaining = text.trim();
    while !remaining.is_empty() && out.len() < lines {
        if out.len() == lines - 1 || width(face, remaining) <= max_width {
            out.push(fit(face, remaining, max_width));
            break;
        }
        let mut split = remaining.len();
        let mut last_space = 0;
        let mut used = 0.;
        for (i, ch) in remaining.char_indices() {
            used += advance(face, ch);
            if ch == ' ' {
                last_space = i;
            }
            if used > max_width {
                split = if last_space > 0 { last_space } else { i };
                break;
            }
        }
        if split == 0 {
            break;
        }
        out.push(remaining[..split].trim_end().to_owned());
        remaining = remaining[split..].trim_start();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_sets_cover_latin_and_fall_back_for_the_rest() {
        assert!(matches!(lookup(Face::R15, 'a'), Some(Source::Exact(_))));
        assert!(matches!(lookup(Face::R15, 'é'), Some(Source::Exact(_))));
        assert!(matches!(lookup(Face::R15, 'ł'), Some(Source::Exact(_))));
        assert!(matches!(lookup(Face::R15, 'Ж'), Some(Source::Fallback(_))));
        assert!(matches!(lookup(Face::S36, 'a'), Some(Source::Fallback(_))));
        // Unsupported scripts show the replacement glyph instead of nothing.
        assert!(matches!(lookup(Face::R15, '音'), Some(Source::Fallback(g)) if g.0 == 0xFFFD));
    }

    #[test]
    fn widths_grow_with_size_and_tracking() {
        assert!(width(Face::R11, "Reborn") < width(Face::R15, "Reborn"));
        assert!(width_tracked(Face::S10, "ARTISTS", 1.5) > width(Face::S10, "ARTISTS"));
        assert_eq!(width(Face::R13, ""), 0.);
    }

    #[test]
    fn fit_and_wrap_respect_the_box() {
        let long = "A very long title that extends well beyond the edge of a small player";
        let cut = fit(Face::R15, long, 200.);
        assert!(cut.ends_with('…') && width(Face::R15, &cut) <= 200.);
        let lines = wrap(Face::S25, long, 220., 2);
        assert_eq!(lines.len(), 2);
        assert!(lines.iter().all(|l| width(Face::S25, l) <= 220.));
        assert_eq!(fit(Face::R15, "Short", 200.), "Short");
    }
}
