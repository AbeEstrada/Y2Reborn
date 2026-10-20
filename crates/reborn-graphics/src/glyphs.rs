//! The runtime glyph page and the CJK font file behind it.
//!
//! Characters the built-in Open Sans pages lack (Japanese, Chinese, Korean)
//! are rasterised at runtime by `reborn-ui` from Noto Sans CJK into one
//! 1024x1024 coverage page. The UI writes glyph cells into the shared CPU copy
//! here; the renderer uploads the rows that changed before it draws (and the
//! whole page after it was (re)created), so neither crate needs the other's
//! handles. Nothing is allocated until the first such glyph is drawn.
use std::{
    fs::File,
    os::fd::AsRawFd,
    path::PathBuf,
    sync::{Mutex, MutexGuard, OnceLock},
};

/// Edge of the square glyph page, in texels.
pub const GLYPH_PAGE: usize = 1024;

/// Where the image installs the CJK fallback font (Buildroot package
/// `y2-font-noto-cjk`): Noto Sans CJK JP Regular, OpenType/CFF, OFL-1.1.
pub const CJK_FONT_PATH: &str = "/usr/share/fonts/noto-cjk/NotoSansCJKjp-Regular.otf";
/// Overrides `CJK_FONT_PATH` (host previews and tests).
pub const CJK_FONT_ENV: &str = "REBORN_CJK_FONT";
/// Larger files are refused rather than mapped (the OTF is 16.5 MB).
const CJK_FONT_MAX: u64 = 64 << 20;

/// The A8 page as the UI last wrote it, and the rows not yet uploaded.
#[derive(Default)]
pub struct GlyphPage {
    pixels: Vec<u8>,
    dirty: Option<(usize, usize)>,
    writes: u64,
}

impl GlyphPage {
    /// Copy a `w` x `h` coverage cell to (`x`, `y`), replacing what was there.
    /// The first write allocates the page.
    pub fn write(&mut self, x: usize, y: usize, w: usize, h: usize, coverage: &[u8]) {
        if w == 0 || h == 0 || x + w > GLYPH_PAGE || y + h > GLYPH_PAGE {
            return;
        }
        debug_assert_eq!(coverage.len(), w * h);
        if self.pixels.is_empty() {
            self.pixels = vec![0; GLYPH_PAGE * GLYPH_PAGE];
        }
        for (row, line) in coverage.chunks_exact(w).take(h).enumerate() {
            let at = (y + row) * GLYPH_PAGE + x;
            self.pixels[at..at + w].copy_from_slice(line);
        }
        self.dirty = Some(match self.dirty {
            Some((a, b)) => (a.min(y), b.max(y + h)),
            None => (y, y + h),
        });
        self.writes += 1;
    }
    /// Whether any glyph was ever written (the page exists).
    pub fn allocated(&self) -> bool {
        !self.pixels.is_empty()
    }
    /// The whole page, one byte per texel (empty before the first write).
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }
    /// Rows `[start, end)` written since the last call.
    pub fn take_dirty(&mut self) -> Option<(usize, usize)> {
        self.dirty.take()
    }
    /// Cell writes since the page was created (diagnostics).
    pub fn writes(&self) -> u64 {
        self.writes
    }
}

/// The process-wide page shared by the UI (writer) and the renderer (uploader).
pub fn glyph_page() -> MutexGuard<'static, GlyphPage> {
    static PAGE: OnceLock<Mutex<GlyphPage>> = OnceLock::new();
    PAGE.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The CJK font file, mapped read-only on first use and kept for the life of
/// the process; `None` when it is missing (host previews, older images), and
/// then the UI keeps drawing U+FFFD. Only touched pages become resident and
/// they stay reclaimable page cache.
pub fn cjk_font() -> Option<&'static [u8]> {
    static FONT: OnceLock<Option<&'static [u8]>> = OnceLock::new();
    *FONT.get_or_init(|| {
        let path = std::env::var_os(CJK_FONT_ENV)
            .filter(|p| !p.is_empty())
            .map_or_else(|| PathBuf::from(CJK_FONT_PATH), PathBuf::from);
        map_read_only(&path)
    })
}

fn map_read_only(path: &std::path::Path) -> Option<&'static [u8]> {
    let file = File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    let len = meta.len();
    if !meta.is_file() || len == 0 || len > CJK_FONT_MAX {
        return None;
    }
    let len = usize::try_from(len).ok()?;
    // SAFETY: a fresh private read-only mapping of a regular file we opened;
    // the kernel validates fd and length. The mapping is never unmapped, so
    // the 'static slice stays valid; the font lives on the read-only rootfs
    // (OTA replaces the partition from recovery, never under a running UI).
    let data = unsafe {
        let p = libc::mmap(
            std::ptr::null_mut(),
            len,
            libc::PROT_READ,
            libc::MAP_PRIVATE,
            file.as_raw_fd(),
            0,
        );
        if p == libc::MAP_FAILED {
            return None;
        }
        // Glyph outlines are read sparsely; skip readahead around each fault.
        libc::madvise(p, len, libc::MADV_RANDOM);
        std::slice::from_raw_parts(p.cast::<u8>(), len)
    };
    Some(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_allocate_once_and_track_dirty_rows() {
        let mut page = GlyphPage::default();
        assert!(!page.allocated() && page.take_dirty().is_none());
        page.write(10, 20, 2, 3, &[1, 2, 3, 4, 5, 6]);
        assert!(page.allocated());
        assert_eq!(page.pixels().len(), GLYPH_PAGE * GLYPH_PAGE);
        assert_eq!(page.pixels()[20 * GLYPH_PAGE + 10..][..2], [1, 2]);
        assert_eq!(page.pixels()[22 * GLYPH_PAGE + 10..][..2], [5, 6]);
        page.write(0, 100, 1, 1, &[9]);
        assert_eq!(page.take_dirty(), Some((20, 101)));
        assert_eq!(page.take_dirty(), None);
        // A rewrite replaces the cell.
        page.write(10, 20, 2, 3, &[0; 6]);
        assert_eq!(page.pixels()[20 * GLYPH_PAGE + 10], 0);
        assert_eq!(page.take_dirty(), Some((20, 23)));
        assert_eq!(page.writes(), 3);
        // Out of bounds is ignored.
        page.write(GLYPH_PAGE - 1, 0, 2, 1, &[1, 1]);
        assert_eq!(page.take_dirty(), None);
    }

    #[test]
    fn a_missing_font_maps_to_nothing() {
        assert!(map_read_only(std::path::Path::new("/nonexistent/font.otf")).is_none());
        let dir = std::env::temp_dir().join(format!("reborn-font-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = dir.join("empty.otf");
        std::fs::write(&empty, b"").unwrap();
        assert!(map_read_only(&empty).is_none());
        let small = dir.join("small.otf");
        std::fs::write(&small, b"OTTO1234").unwrap();
        assert_eq!(map_read_only(&small), Some(&b"OTTO1234"[..]));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
