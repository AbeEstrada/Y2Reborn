#![forbid(unsafe_code)]
//! Invoked only with an inherited, empty scratch directory descriptor.
use reborn_core::{Action, AppModel, Screen};
use reborn_library::benchmark::{database, scanner, summary, timed};
use reborn_platform::allocation_counts as allocations;
use reborn_ui::Ui;
use serde_json::json;
use std::{fs, path::PathBuf, time::Instant};
#[global_allocator]
static ALLOCATOR: allocations::Counting = allocations::Counting;
#[path = "../artwork.rs"]
mod artwork;

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 || args[1] != "--scratch-fd" || args[3] != "--tracks" {
        return Err("use y2-platform bench-library (inherited empty scratch FD required)".into());
    }
    let fd = args[2].parse::<u32>().map_err(|e| e.to_string())?;
    if fd < 3 {
        return Err("scratch directory fd must be >=3".into());
    }
    let root = PathBuf::from(format!("/proc/self/fd/{fd}"));
    let entries = fs::read_dir(&root).map_err(|e| e.to_string())?;
    if entries.count() != 0 {
        return Err("scratch directory must be empty".into());
    }
    let tracks = args[4].parse::<usize>().map_err(|e| e.to_string())?;
    if args.iter().any(|a| a == "--platform-db") {
        let media_root = args
            .windows(2)
            .find(|pair| pair[0] == "--media-fd")
            .map(|pair| {
                let fd = pair[1].parse::<u32>().map_err(|e| e.to_string())?;
                if fd < 3 {
                    return Err("media directory fd must be >=3".to_owned());
                }
                let path = PathBuf::from(format!("/proc/self/fd/{fd}"));
                if fs::read_dir(&path).map_err(|e| e.to_string())?.count() != 0 {
                    return Err("media scratch directory must be empty".to_owned());
                }
                Ok(path)
            })
            .transpose()?;
        let (report, library) =
            reborn_library::benchmark::platform_db_on_media(&root, media_root.as_deref(), tracks)?;
        let mut model = AppModel::default();
        let mut ui = Ui::default();
        let mut measurements = vec![];
        for (name, screen) in [
            ("songs_catalog", Screen::Tracks),
            ("albums_catalog", Screen::Albums),
            ("artists_catalog", Screen::Artists),
        ] {
            model.screen = screen;
            let mut samples = vec![];
            for _ in 0..21 {
                ui.invalidate_catalog();
                timed(&mut samples, || {
                    Ok(ui.action(&mut model, &library, Action::WheelClockwise(1)))
                })?;
            }
            measurements.push(summary(name, &samples));
        }
        model.screen = Screen::Tracks;
        let mut samples = vec![];
        for _ in 0..128 {
            timed(&mut samples, || {
                Ok(ui.action(&mut model, &library, Action::WheelClockwise(1)))
            })?;
        }
        measurements.push(summary("cached_wheel_navigation", &samples));
        samples.clear();
        for _ in 0..7 {
            timed(&mut samples, || {
                model.replace_queue(
                    library
                        .iter()
                        .take(reborn_core::MAX_QUEUE)
                        .cloned()
                        .collect(),
                    0,
                )
            })?;
        }
        measurements.push(summary("queue_population", &samples));
        println!(
            "{}",
            json!({"database":report,"ui":measurements,
            "memory":fs::read_to_string("/proc/self/status").ok(),
            "evidence":"scratch-only userspace; page cache uncontrolled"})
        );
        return Ok(());
    }
    if ![1000, 10000, 20000].contains(&tracks) {
        return Err("tracks must be 1000, 10000 or 20000".into());
    }
    let (db, library) = database(&root, tracks)?;
    let mut ui = Ui::default();
    let mut model = AppModel::default();
    let mut measurements = vec![];
    for (name, screen) in [
        ("album_listing", Screen::Albums),
        ("artist_listing", Screen::Artists),
        ("folder_listing", Screen::Folders),
        ("track_listing", Screen::Tracks),
    ] {
        model.screen = screen;
        let mut samples = vec![];
        for _ in 0..16 {
            timed(&mut samples, || Ok(ui.rows(&model, &library)))?;
        }
        measurements.push(summary(name, &samples));
    }
    model.screen = Screen::Tracks;
    let mut samples = vec![];
    for _ in 0..128 {
        timed(&mut samples, || {
            Ok(ui.action(&mut model, &library, Action::WheelClockwise(1)))
        })?;
    }
    measurements.push(summary("wheel_navigation", &samples));
    samples.clear();
    timed(&mut samples, || {
        model.replace_queue(
            library
                .iter()
                .take(reborn_core::MAX_QUEUE)
                .cloned()
                .collect(),
            0,
        )
    })?;
    measurements.push(summary("queue_construction", &samples));
    // Complete Product v2 frames, at the actual 480x360 layout. Optional KMS
    // rendering requires exclusive display ownership; the fixture driver stops
    // and restores the application, never runs beside the production renderer.
    let mut renderer = if args.iter().any(|a| a == "--graphics") {
        Some(reborn_graphics::Renderer::open(
            &reborn_ui::atlases(),
            reborn_observability::Observer::new(&root.join("logs")).map_err(|e| e.to_string())?,
        )?)
    } else {
        None
    };
    let mut frames = vec![];
    let capture_dir = args
        .iter()
        .position(|a| a == "--capture-dir")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from);
    if let Some(dir) = &capture_dir {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    for screen in [
        Screen::Home,
        Screen::Music,
        Screen::Artists,
        Screen::Albums,
        Screen::Tracks,
        Screen::Artist,
        Screen::Album,
        Screen::NowPlaying,
        Screen::Queue,
        Screen::Settings,
    ] {
        model.screen = screen;
        model.navigation = Default::default();
        if screen == Screen::Artist {
            model.navigation.filter = format!("artist:{}", library[0].artist);
        }
        if screen == Screen::Album {
            model.navigation.filter = format!(
                "album:{}\u{1f}{}",
                library[0].album_artist, library[0].album
            );
        }
        ui.invalidate_catalog();
        let mut layout = vec![];
        let mut layout_allocations = Vec::with_capacity(32);
        let mut submit = vec![];
        let mut present = vec![];
        let mut cpu = vec![];
        let mut quads = 0;
        let mut draw_calls = None;
        let first = Instant::now();
        for _ in 0..32 {
            let draw = timed(&mut layout, || {
                let (draw, counts) = allocations::measure(|| {
                    ui.normalize(&mut model, &library);
                    ui.draw(&model, &library, false)
                });
                layout_allocations.push(counts);
                Ok(draw)
            })?;
            quads = draw.len();
            if let Some(g) = &mut renderer {
                g.render(&draw)?;
                draw_calls = Some(g.timings.draw_calls);
                submit.push((g.timings.submit_ms * 1e6) as u64);
                present.push((g.timings.present_ms * 1e6) as u64);
                if let Some(ms) = g.timings.cpu_ms {
                    cpu.push((ms * 1e6) as u64);
                }
            }
        }
        frames.push(json!({"screen":screen,"quads":quads,"draw_calls":draw_calls,"first_layout_ms":layout[0] as f64/1e6,
            "layout":summary("complete_frame_layout", &layout),"submit":summary("GL_submission",&submit),
            "present":summary("swap_and_KMS_completion",&present),"render_cpu":summary("render_thread_CPU",&cpu),
            "first_layout_allocations":layout_allocations[0],"warm_layout_allocations":allocations::summary(&layout_allocations[1..]),
            "sequence_ms":first.elapsed().as_secs_f64()*1000.}));
        if let (Some(dir), Some(g)) = (&capture_dir, &mut renderer) {
            let pixels = g.capture(&ui.draw(&model, &library, false))?;
            fs::write(dir.join(format!("{screen:?}.rgba")), pixels).map_err(|e| e.to_string())?;
        }
    }
    model.screen = Screen::Tracks;
    model.navigation = Default::default();
    let mut wheel = vec![];
    let mut wheel_frame = vec![];
    let mut wheel_allocations = Vec::with_capacity(1000);
    let mut scroll_allocations = Vec::with_capacity(1000);
    // Cold catalog construction is already retained in Tracks.first_layout.
    // Wheel counts describe an established list, not the first route entry.
    let _ = ui.draw(&model, &library, false);
    for _ in 0..1000 {
        timed(&mut wheel, || {
            let (effect, counts) =
                allocations::measure(|| ui.action(&mut model, &library, Action::WheelClockwise(1)));
            wheel_allocations.push(counts);
            Ok(effect)
        })?;
        timed(&mut wheel_frame, || {
            let (draw, counts) = allocations::measure(|| ui.draw(&model, &library, false));
            scroll_allocations.push(counts);
            Ok(draw)
        })?;
    }
    measurements.push(summary("1000_wheel_actions", &wheel));
    measurements.push(summary("1000_scrolled_frame_layouts", &wheel_frame));
    let mut navigation = vec![];
    for _ in 0..32 {
        model.screen = Screen::Artists;
        model.navigation = Default::default();
        ui.invalidate_catalog();
        let _ = ui.draw(&model, &library, false);
        timed(&mut navigation, || {
            let _ = ui.action(&mut model, &library, Action::Select);
            Ok(ui.draw(&model, &library, false))
        })?;
        let _ = ui.action(&mut model, &library, Action::Back);
    }
    measurements.push(summary("open_artist_to_complete_layout", &navigation));
    // Stress a single collection containing the entire fixture, rather than
    // mistaking its normal ten-song albums for a large-album qualification.
    let mut collection = library.clone();
    for track in &mut collection {
        track.artist = "Large collection".into();
        track.album_artist = "Large collection".into();
        track.album = "Large album".into();
    }
    let mut large_collections = vec![];
    for (parent, destination) in [
        (Screen::Artists, Screen::Artist),
        (Screen::Albums, Screen::Album),
    ] {
        let mut layouts = vec![];
        let mut counts = Vec::with_capacity(16);
        let mut render_cpu = vec![];
        for _ in 0..16 {
            model.screen = parent;
            model.navigation = Default::default();
            ui.invalidate_catalog();
            let _ = ui.draw(&model, &collection, false);
            let draw = timed(&mut layouts, || {
                let (draw, allocations) = allocations::measure(|| {
                    let _ = ui.action(&mut model, &collection, Action::Select);
                    ui.draw(&model, &collection, false)
                });
                counts.push(allocations);
                Ok(draw)
            })?;
            if model.screen != destination {
                return Err("large collection navigation failed".into());
            }
            if let Some(g) = &mut renderer {
                g.render(&draw)?;
                if let Some(ms) = g.timings.cpu_ms {
                    render_cpu.push((ms * 1e6) as u64);
                }
            }
            let _ = ui.action(&mut model, &collection, Action::Back);
            if model.screen != parent {
                return Err("large collection Back failed".into());
            }
        }
        large_collections.push(json!({"screen":destination,"tracks":collection.len(),
            "select_to_complete_layout":summary("large_collection_navigation",&layouts),
            "render_cpu":summary("large_collection_render_CPU",&render_cpu),"allocations":allocations::summary(&counts)}));
    }
    let mut artwork_samples = vec![];
    if let Some(dir) = args
        .iter()
        .position(|a| a == "--artwork-dir")
        .and_then(|i| args.get(i + 1))
    {
        let mut worker = artwork::Worker::spawn().map_err(|e| e.to_string())?;
        let fixtures = [
            "artwork-jpeg.flac",
            "artwork-png.flac",
            "artwork-webp.flac",
            "artwork.flac",
            "malformed.mp3",
            "missing.flac",
        ];
        for (step, index) in [0, 1, 2, 3, 0, 1, 2, 3, 4, 5, 0, 1, 2, 3]
            .into_iter()
            .enumerate()
        {
            let path = PathBuf::from(dir).join(fixtures[index]);
            let stat = fs::metadata(&path).ok();
            let track = reborn_core::Track {
                id: index as i64,
                source_id: "internal".into(),
                artwork: index < 4,
                mtime: stat
                    .as_ref()
                    .and_then(|s| s.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|t| t.as_nanos().min(i64::MAX as u128) as i64)
                    .unwrap_or(0),
                path,
                size: stat.as_ref().map(|s| s.len()).unwrap_or(0),
                ..Default::default()
            };
            let cache =
                std::env::temp_dir().join(format!("reborn-bench-thumbs-{}", std::process::id()));
            fs::create_dir_all(&cache).map_err(|e| e.to_string())?;
            worker.want(vec![artwork::Job {
                thumb: index as u64 + 1,
                art: artwork::Key::new(&track, None, std::path::Path::new(dir)),
                cache: cache.clone(),
            }]);
            let reply = worker
                .replies
                .recv_timeout(std::time::Duration::from_secs(10))
                .map_err(|e| e.to_string())?;
            if reply.thumb != index as u64 + 1 {
                return Err("stale artwork fixture reply".into());
            }
            let expected_hit = (4..8).contains(&step) || step >= 10;
            if reply.cache_hit != expected_hit && reply.rgba.is_some() {
                return Err("artwork fixture cache regression".into());
            }
            let mut upload_ms = None;
            if let (Some(pixels), Some(g)) = (&reply.rgba, &mut renderer) {
                let start = Instant::now();
                g.upload_thumb(index as u64 + 1, pixels)?;
                upload_ms = Some(start.elapsed().as_secs_f64() * 1000.);
            }
            artwork_samples.push(json!({"fixture":fixtures[index], "cache_hit":reply.cache_hit,
                "decode_ms":reply.decode_ms,"rgba_bytes":reply.rgba.as_ref().map(Vec::len),"upload_ms":upload_ms}));
        }
        if !worker.quiesce() {
            return Err("artwork benchmark did not quiesce".into());
        }
    }
    let scan = if args.iter().any(|a| a == "--scan") {
        Some(scanner(&root, tracks)?)
    } else {
        None
    };
    println!(
        "{}",
        json!({"database":db,"ui":measurements,"frames":frames,"artwork":artwork_samples,"scan":scan,
        "allocations":{"1000_wheel_actions":allocations::summary(&wheel_allocations),"1000_scrolled_frame_layouts":allocations::summary(&scroll_allocations)},
        "large_collections":large_collections,
        "memory":fs::read_to_string("/proc/self/status").ok(),
        "evidence":"userspace measurement only; no electrical durability or target qualification"})
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        println!("{}", json!({"result":"FAILED","failure":error}));
        std::process::exit(1);
    }
}
