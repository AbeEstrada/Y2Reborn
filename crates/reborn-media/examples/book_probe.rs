//! Audiobook risk probe (Phase 0): runs files through Reborn's media layer as the
//! scanner (inspect) and the player (open, seek near the end, read) do.
//! `cargo run -p reborn-media --example book_probe -- FILE...`
use reborn_media::{Cancel, Decoder, MediaFile};
use std::{path::Path, time::Instant};

fn main() {
    for arg in std::env::args().skip(1) {
        let path = Path::new(&arg);
        println!("== {arg}");
        let t = Instant::now();
        let duration = match Cancel::new().and_then(|c| MediaFile::open(path, c)) {
            Ok(file) => {
                let m = &file.metadata;
                println!(
                    "inspect ok {:.0} ms: title={:?} artist={:?} genre={:?} duration={} ms",
                    t.elapsed().as_secs_f64() * 1000.,
                    m.title,
                    m.artist,
                    m.genre,
                    m.duration_ms
                );
                m.duration_ms
            }
            Err(e) => {
                println!(
                    "inspect FAILED {:.0} ms: {e}",
                    t.elapsed().as_secs_f64() * 1000.
                );
                0
            }
        };
        let t = Instant::now();
        match Cancel::new().and_then(|c| Decoder::open(path, 44_100, c)) {
            Ok(mut d) => {
                println!("open ok {:.0} ms", t.elapsed().as_secs_f64() * 1000.);
                println!(
                    "first read from the start -> {:?}",
                    d.read().map(|b| b.map(|p| p.position_ms))
                );
                let target = duration * 9 / 10;
                let t = Instant::now();
                let seek = d.seek(target);
                let block = d.read();
                println!(
                    "seek {target} ms {:?} + read {:.0} ms -> position {:?}",
                    seek.err(),
                    t.elapsed().as_secs_f64() * 1000.,
                    block.map(|b| b.map(|p| p.position_ms))
                );
            }
            Err(e) => println!(
                "open FAILED {:.0} ms: {e}",
                t.elapsed().as_secs_f64() * 1000.
            ),
        }
    }
}
