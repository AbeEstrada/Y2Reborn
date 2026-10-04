//! Product-path benchmark. Existing media is read only; DB/logs use new scratch.
use reborn_core::{MediaSource, Source};
use reborn_library::{Database, Scanner};
use reborn_media::{Cancel, Decoder, MediaFile};
use reborn_observability::Observer;
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args_os().collect();
    let mode = args
        .get(1)
        .ok_or("mode: parse, playback, artwork, parallel or scan")?
        .to_string_lossy();
    let media = PathBuf::from(args.get(2).ok_or("media directory")?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    if matches!(mode.as_ref(), "parse" | "playback") {
        let mut rows = vec![];
        let mut paths = fs::read_dir(&media)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| reborn_library::supported(p))
            .collect::<Vec<_>>();
        paths.sort();
        if paths.is_empty() {
            return Err("no media fixtures".into());
        }
        for path in paths {
            let mut samples = vec![];
            let mut metadata = None;
            for _ in 0..20 {
                let start = Instant::now();
                let (elapsed, m) = if mode == "playback" {
                    let decoder = Decoder::open(&path, 48000, Cancel::new()?)?;
                    (start.elapsed().as_nanos() as u64, decoder.metadata.clone())
                } else {
                    let media = MediaFile::open(&path, Cancel::new()?)?;
                    (start.elapsed().as_nanos() as u64, media.metadata.clone())
                };
                samples.push(elapsed);
                metadata = Some(m);
            }
            rows.push(json!({"file":path.file_name().unwrap_or_default().to_string_lossy(),"timing":reborn_library::benchmark::summary("parse",&samples),"metadata":metadata}));
        }
        println!(
            "{}",
            json!({"rows":rows,"memory":reborn_library::benchmark::process_memory()})
        );
    } else if mode == "parallel" {
        let paths = fs::read_dir(&media)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| reborn_library::supported(p))
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Err("no media fixtures".into());
        }
        let mut rows = vec![];
        for workers in [1, 2, 4] {
            let start = Instant::now();
            let result = std::thread::scope(|scope| {
                let paths = &paths;
                let handles = (0..workers)
                    .map(|worker| {
                        scope.spawn(move || -> Result<u64, String> {
                            let mut count = 0;
                            for i in (worker..paths.len() * 100).step_by(workers) {
                                let _ = MediaFile::open(&paths[i % paths.len()], Cancel::new()?)?;
                                count += 1;
                            }
                            Ok(count)
                        })
                    })
                    .collect::<Vec<_>>();
                handles
                    .into_iter()
                    .map(|h| h.join().map_err(|_| "worker panic".to_string())?)
                    .sum::<Result<u64, String>>()
            })?;
            rows.push(json!({"workers":workers,"files":result,"elapsed_ms":start.elapsed().as_secs_f64()*1000.,"memory":reborn_library::benchmark::process_memory()}));
        }
        println!(
            "{}",
            json!({"rows":rows,"scope":"metadata-only concurrency experiment; no database or physical SD contention"})
        );
    } else if mode == "artwork" {
        let mut paths = fs::read_dir(&media)
            .map_err(|e| e.to_string())?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect::<Vec<_>>();
        paths.sort();
        let mut rows = vec![];
        for path in paths {
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            if !["jpg", "png", "webp", "flac"].contains(&ext) {
                continue;
            }
            let mut samples = vec![];
            let mut successes = 0;
            for _ in 0..10 {
                let start = Instant::now();
                let pixels = if ext == "flac" {
                    MediaFile::open(&path, Cancel::new()?).and_then(|mut m| m.artwork())
                } else {
                    reborn_media::external_artwork(&path)
                };
                if pixels.is_ok() {
                    successes += 1;
                }
                samples.push(start.elapsed().as_nanos() as u64);
            }
            rows.push(json!({"file":path.file_name().unwrap_or_default().to_string_lossy(),"successes":successes,"timing":reborn_library::benchmark::summary("artwork",&samples)}));
        }
        println!(
            "{}",
            json!({"rows":rows,"memory":reborn_library::benchmark::process_memory()})
        );
    } else if mode == "scan" {
        let root = PathBuf::from(args.get(3).ok_or("new scratch directory")?);
        fs::create_dir(&root).map_err(|e| e.to_string())?;
        let log = Observer::new(&root.join("logs")).map_err(|e| e.to_string())?;
        let db = Database::spawn(root.join("library.sqlite"), log.clone())?;
        let source = Source {
            id: "bench".into(),
            kind: MediaSource::Internal,
            root: media,
            online: true,
            mount: "host fixture".into(),
            mount_id: None,
        };
        db.sources(vec![source.clone()])?;
        let scanner = Scanner::spawn(db.clone(), log)?;
        let mut rows = vec![];
        for operation in ["initial", "incremental"] {
            scanner.scan(vec![source.clone()])?;
            let metrics = scanner
                .results
                .recv_timeout(Duration::from_secs(900))
                .map_err(|e| e.to_string())??;
            rows.push(json!({"operation":operation,"metrics":metrics,"memory":reborn_library::benchmark::process_memory()}));
        }
        scanner.stop();
        db.shutdown(Duration::from_secs(15))?;
        println!("{}", json!({"rows":rows}));
    } else {
        return Err("mode: parse, playback, artwork, parallel or scan".into());
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(1)
    }
}
