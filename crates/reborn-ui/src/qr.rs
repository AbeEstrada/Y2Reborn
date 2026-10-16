//! A small QR code encoder (ISO/IEC 18004): byte mode, error correction
//! level M, versions 1–10 (up to 213 bytes) — enough for the Last.fm
//! approval address the listener scans with a phone. Follows the structure
//! of Project Nayuki's reference encoder (MIT); no allocation-heavy features.

/// A square of modules, `true` = dark.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Code {
    pub size: usize,
    modules: Vec<bool>,
}
impl Code {
    pub fn dark(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

// Level M, versions 0..=10 (index 0 unused).
const ECC_PER_BLOCK: [usize; 11] = [0, 10, 16, 26, 18, 24, 16, 18, 22, 22, 26];
const BLOCKS: [usize; 11] = [0, 1, 1, 1, 2, 2, 4, 4, 4, 5, 5];
const MAX_VERSION: usize = 10;

fn raw_modules(ver: usize) -> usize {
    let mut result = (16 * ver + 128) * ver + 64;
    if ver >= 2 {
        let align = ver / 7 + 2;
        result -= (25 * align - 10) * align - 55;
        if ver >= 7 {
            result -= 36;
        }
    }
    result
}
fn data_codewords(ver: usize) -> usize {
    raw_modules(ver) / 8 - ECC_PER_BLOCK[ver] * BLOCKS[ver]
}
fn count_bits(ver: usize) -> usize {
    if ver <= 9 {
        8
    } else {
        16
    }
}

/// Encode `data`; `None` when it does not fit version 10-M.
pub fn encode(data: &[u8]) -> Option<Code> {
    let ver =
        (1..=MAX_VERSION).find(|&v| 4 + count_bits(v) + data.len() * 8 <= data_codewords(v) * 8)?;
    let capacity = data_codewords(ver) * 8;
    let mut bits: Vec<bool> = vec![];
    let push = |value: usize, n: usize, bits: &mut Vec<bool>| {
        for i in (0..n).rev() {
            bits.push((value >> i) & 1 == 1);
        }
    };
    push(0b0100, 4, &mut bits);
    push(data.len(), count_bits(ver), &mut bits);
    for &b in data {
        push(b as usize, 8, &mut bits);
    }
    let terminator = (capacity - bits.len()).min(4);
    push(0, terminator, &mut bits);
    let pad = (8 - bits.len() % 8) % 8;
    push(0, pad, &mut bits);
    let mut codewords: Vec<u8> = bits
        .chunks(8)
        .map(|c| c.iter().fold(0u8, |a, &b| a << 1 | b as u8))
        .collect();
    for pad in [0xECu8, 0x11].iter().cycle() {
        if codewords.len() >= data_codewords(ver) {
            break;
        }
        codewords.push(*pad);
    }
    let all = add_ecc_and_interleave(&codewords, ver);
    let size = ver * 4 + 17;
    let mut best: Option<(u32, Code)> = None;
    for mask in 0..8 {
        let mut q = Builder::new(ver);
        q.function_patterns();
        q.draw_codewords(&all);
        q.apply_mask(mask);
        q.format_bits(mask);
        let penalty = q.penalty();
        if best.as_ref().is_none_or(|(p, _)| penalty < *p) {
            best = Some((
                penalty,
                Code {
                    size,
                    modules: q.modules,
                },
            ));
        }
    }
    best.map(|(_, c)| c)
}

fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z: u16 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= ((y as u16 >> i) & 1) * x as u16;
    }
    z as u8
}
fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 0x02);
    }
    result
}
fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result[0];
        result.remove(0);
        result.push(0);
        for (r, &d) in result.iter_mut().zip(divisor) {
            *r ^= gf_mul(d, factor);
        }
    }
    result
}
fn add_ecc_and_interleave(data: &[u8], ver: usize) -> Vec<u8> {
    let blocks = BLOCKS[ver];
    let ecc_len = ECC_PER_BLOCK[ver];
    let raw = raw_modules(ver) / 8;
    let short_blocks = blocks - raw % blocks;
    let short_len = raw / blocks;
    let divisor = rs_divisor(ecc_len);
    let mut out_blocks: Vec<Vec<u8>> = vec![];
    let mut k = 0;
    for i in 0..blocks {
        let len = short_len - ecc_len + usize::from(i >= short_blocks);
        let mut block = data[k..k + len].to_vec();
        k += len;
        let ecc = rs_remainder(&block, &divisor);
        if i < short_blocks {
            block.push(0); // placeholder, skipped when interleaving
        }
        block.extend(ecc);
        out_blocks.push(block);
    }
    let mut result = vec![];
    for i in 0..out_blocks[0].len() {
        for (j, block) in out_blocks.iter().enumerate() {
            if i != short_len - ecc_len || j >= short_blocks {
                result.push(block[i]);
            }
        }
    }
    result
}

struct Builder {
    ver: usize,
    size: usize,
    modules: Vec<bool>,
    function: Vec<bool>,
}
impl Builder {
    fn new(ver: usize) -> Self {
        let size = ver * 4 + 17;
        Self {
            ver,
            size,
            modules: vec![false; size * size],
            function: vec![false; size * size],
        }
    }
    fn set(&mut self, x: usize, y: usize, dark: bool) {
        self.modules[y * self.size + x] = dark;
        self.function[y * self.size + x] = true;
    }
    fn get(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
    fn function_patterns(&mut self) {
        let size = self.size;
        for i in 0..size {
            self.set(6, i, i % 2 == 0);
            self.set(i, 6, i % 2 == 0);
        }
        for (x, y) in [(3, 3), (size - 4, 3), (3, size - 4)] {
            self.finder(x as i32, y as i32);
        }
        let positions = self.alignment_positions();
        let n = positions.len();
        for i in 0..n {
            for j in 0..n {
                if (i == 0 && j == 0) || (i == 0 && j == n - 1) || (i == n - 1 && j == 0) {
                    continue;
                }
                self.alignment(positions[i], positions[j]);
            }
        }
        self.format_bits(0); // reserve
        self.version_bits();
    }
    fn finder(&mut self, cx: i32, cy: i32) {
        for dy in -4..=4i32 {
            for dx in -4..=4i32 {
                let (x, y) = (cx + dx, cy + dy);
                if x < 0 || y < 0 || x >= self.size as i32 || y >= self.size as i32 {
                    continue;
                }
                let dist = dx.abs().max(dy.abs());
                self.set(x as usize, y as usize, dist != 2 && dist != 4);
            }
        }
    }
    fn alignment(&mut self, cx: usize, cy: usize) {
        for dy in -2..=2i32 {
            for dx in -2..=2i32 {
                let dist = dx.abs().max(dy.abs());
                self.set(
                    (cx as i32 + dx) as usize,
                    (cy as i32 + dy) as usize,
                    dist != 1,
                );
            }
        }
    }
    fn alignment_positions(&self) -> Vec<usize> {
        if self.ver == 1 {
            return vec![];
        }
        let n = self.ver / 7 + 2;
        let step = (self.ver * 4 + n * 2 + 1) / (n * 2 - 2) * 2;
        let mut result = vec![6];
        let mut pos = self.size - 7;
        let mut rest = vec![];
        for _ in 0..n - 1 {
            rest.push(pos);
            pos -= step;
        }
        rest.reverse();
        result.extend(rest);
        result
    }
    fn format_bits(&mut self, mask: usize) {
        // Level M has format bits 00.
        let data = mask;
        let mut rem = data;
        for _ in 0..10 {
            rem = (rem << 1) ^ ((rem >> 9) * 0x537);
        }
        let bits = (data << 10 | rem) ^ 0x5412;
        let bit = |i: usize| (bits >> i) & 1 == 1;
        let size = self.size;
        for i in 0..=5 {
            self.set(8, i, bit(i));
        }
        self.set(8, 7, bit(6));
        self.set(8, 8, bit(7));
        self.set(7, 8, bit(8));
        for i in 9..15 {
            self.set(14 - i, 8, bit(i));
        }
        for i in 0..8 {
            self.set(size - 1 - i, 8, bit(i));
        }
        for i in 8..15 {
            self.set(8, size - 15 + i, bit(i));
        }
        self.set(8, size - 8, true);
    }
    fn version_bits(&mut self) {
        if self.ver < 7 {
            return;
        }
        let mut rem = self.ver;
        for _ in 0..12 {
            rem = (rem << 1) ^ ((rem >> 11) * 0x1F25);
        }
        let bits = self.ver << 12 | rem;
        for i in 0..18 {
            let dark = (bits >> i) & 1 == 1;
            let (a, b) = (self.size - 11 + i % 3, i / 3);
            self.set(a, b, dark);
            self.set(b, a, dark);
        }
    }
    fn draw_codewords(&mut self, data: &[u8]) {
        let size = self.size as i32;
        let mut i = 0usize;
        let mut right = size - 1;
        while right >= 1 {
            if right == 6 {
                right = 5;
            }
            for vert in 0..size {
                for j in 0..2 {
                    let x = (right - j) as usize;
                    let upward = (right + 1) & 2 == 0;
                    let y = if upward { size - 1 - vert } else { vert } as usize;
                    if !self.function[y * self.size + x] && i < data.len() * 8 {
                        self.modules[y * self.size + x] = (data[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                        i += 1;
                    }
                }
            }
            right -= 2;
        }
    }
    fn apply_mask(&mut self, mask: usize) {
        for y in 0..self.size {
            for x in 0..self.size {
                let invert = match mask {
                    0 => (x + y) % 2 == 0,
                    1 => y % 2 == 0,
                    2 => x % 3 == 0,
                    3 => (x + y) % 3 == 0,
                    4 => (x / 3 + y / 2) % 2 == 0,
                    5 => x * y % 2 + x * y % 3 == 0,
                    6 => (x * y % 2 + x * y % 3) % 2 == 0,
                    _ => ((x + y) % 2 + x * y % 3) % 2 == 0,
                };
                if invert && !self.function[y * self.size + x] {
                    self.modules[y * self.size + x] ^= true;
                }
            }
        }
    }
    /// Penalty rules 1 (runs), 2 (2×2 blocks), 3 (finder-like patterns) and
    /// 4 (dark balance) of the standard.
    fn penalty(&self) -> u32 {
        let size = self.size;
        let mut result = 0u32;
        let line = |get: &dyn Fn(usize) -> bool| -> u32 {
            let mut p = 0;
            let mut run = 1;
            for i in 1..size {
                if get(i) == get(i - 1) {
                    run += 1;
                } else {
                    if run >= 5 {
                        p += run - 2;
                    }
                    run = 1;
                }
            }
            if run >= 5 {
                p += run - 2;
            }
            let pattern = [true, false, true, true, true, false, true];
            for i in 0..size.saturating_sub(6) {
                if (0..7).all(|k| get(i + k) == pattern[k]) {
                    let before = (i >= 4) && (i - 4..i).all(|k| !get(k));
                    let after = (i + 11 <= size) && (i + 7..i + 11).all(|k| !get(k));
                    if before || after {
                        p += 40;
                    }
                }
            }
            p as u32
        };
        for y in 0..size {
            result += line(&|x| self.get(x, y));
        }
        for x in 0..size {
            result += line(&|y| self.get(x, y));
        }
        for y in 0..size - 1 {
            for x in 0..size - 1 {
                let c = self.get(x, y);
                if c == self.get(x + 1, y) && c == self.get(x, y + 1) && c == self.get(x + 1, y + 1)
                {
                    result += 3;
                }
            }
        }
        let dark = self.modules.iter().filter(|&&m| m).count();
        let total = size * size;
        let k = ((dark * 20).abs_diff(total * 10))
            .div_ceil(total)
            .saturating_sub(1);
        result + k as u32 * 10
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_and_capacity() {
        assert_eq!(data_codewords(1), 16);
        assert_eq!(data_codewords(7), 124);
        assert_eq!(data_codewords(10), 216);
        assert_eq!(encode(b"hello").unwrap().size, 21);
        let url = format!(
            "https://www.last.fm/api/auth/?api_key={}&token={}",
            "a".repeat(32),
            "b".repeat(32)
        );
        assert_eq!(encode(url.as_bytes()).unwrap().size, 7 * 4 + 17);
        assert!(encode(&[b'x'; 214]).is_none());
        assert!(encode(&[b'x'; 213]).is_some());
    }

    #[test]
    fn reed_solomon_known_vector() {
        // ISO/IEC 18004 Annex I: "01234567" 1-M data codewords and their ECC.
        let data = [
            16, 32, 12, 86, 97, 128, 236, 17, 236, 17, 236, 17, 236, 17, 236, 17,
        ];
        let ecc = rs_remainder(&data, &rs_divisor(10));
        assert_eq!(ecc, [165, 36, 212, 193, 237, 54, 199, 135, 44, 85]);
    }
}
