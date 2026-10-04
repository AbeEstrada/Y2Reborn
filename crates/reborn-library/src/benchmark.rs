//! Disposable measurements through the same schema, SQL and scanner as Reborn.
//! Caller must supply a newly created, private scratch directory.
use super::*;
use serde_json::Value;
use std::io::Write;

pub fn summary(name: &str, times: &[u64]) -> Value {
    let mut sorted = times.to_vec();
    sorted.sort_unstable();
    let percentile = |p: usize| {
        sorted
            .get((sorted.len() * p).div_ceil(100).saturating_sub(1))
            .map(|v| *v as f64 / 1_000_000.)
    };
    json!({"operation":name,"samples":times.len(),"p50_ms":percentile(50),
        "p95_ms":percentile(95),"p99_ms":percentile(99),"max_ms":percentile(100),
        "elapsed_ms":times.iter().sum::<u64>() as f64/1_000_000.,
        "samples_ms":times.iter().map(|v|*v as f64/1_000_000.).collect::<Vec<_>>(),
        "operations_per_second":if times.iter().sum::<u64>()>0 {Some(times.len() as f64*1e9/times.iter().sum::<u64>() as f64)} else {None}})
}

pub fn timed<T>(
    samples: &mut Vec<u64>,
    f: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let start = Instant::now();
    let value = f()?;
    samples.push(start.elapsed().as_nanos().min(u64::MAX as u128) as u64);
    Ok(value)
}

pub fn database(root: &Path, count: usize) -> Result<(Value, Vec<Track>), String> {
    if !(1..=20_000).contains(&count) {
        return Err("dataset must be 1..20000 tracks".into());
    }
    let path = root.join("library.sqlite");
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    let mut measurements = vec![];
    let mut times = vec![];
    let mut c = timed(&mut times, || open(&path).map_err(open_message))?;
    measurements.push(summary("database_create_open", &times));
    c.execute(
        "INSERT INTO sources(id,root,online) VALUES('benchmark',?1,1)",
        [root.to_string_lossy()],
    )
    .map_err(err)?;
    let make_track = |i: usize| Track {
        source_id: "benchmark".into(),
        path: root.join(format!(
            "media/artist{:04}/album{:04}/{i:05}.wav",
            i / 200,
            i / 10
        )),
        filename: format!("{i:05}.wav"),
        size: 4096,
        mtime: 1,
        title: format!("Track {i:05}"),
        artist: format!("Artist {:04}", i / 200),
        album: format!("Album {:04}", i / 10),
        album_artist: format!("Artist {:04}", i / 200),
        track: (i % 10 + 1) as u32,
        disc: 1,
        duration_ms: 180_000,
        codec: "pcm_s16le".into(),
        sample_rate: 44100,
        channels: 2,
        bitrate: 1411200,
        online: true,
        ..Track::default()
    };
    times.clear();
    let population = Instant::now();
    for offset in (0..count).step_by(64) {
        let tracks = (offset..(offset + 64).min(count)).map(make_track).collect();
        timed(&mut times, || batch(&mut c, tracks, 1))?;
    }
    measurements.push(summary("batch_commit_64", &times));
    measurements.push(summary(
        "initial_population",
        &[population.elapsed().as_nanos() as u64],
    ));
    times.clear();
    for offset in (0..count).step_by(64) {
        let tracks = (offset..(offset + 64).min(count))
            .map(|i| {
                let mut t = make_track(i);
                t.mtime = 2;
                t
            })
            .collect();
        timed(&mut times, || batch(&mut c, tracks, 2))?;
    }
    measurements.push(summary("incremental_upsert_batch_64", &times));
    for (name, filter) in [
        (
            "list_page",
            Filter {
                limit: 64,
                ..Filter::default()
            },
        ),
        (
            "artist_lookup",
            Filter {
                artist: Some("Artist 0000".into()),
                limit: 64,
                ..Filter::default()
            },
        ),
        (
            "album_lookup",
            Filter {
                album: Some("Album 0000".into()),
                limit: 64,
                ..Filter::default()
            },
        ),
        (
            "folder_lookup",
            Filter {
                folder: Some(root.join("media/artist0000").to_string_lossy().into()),
                limit: 64,
                ..Filter::default()
            },
        ),
    ] {
        times.clear();
        for _ in 0..32 {
            timed(&mut times, || list(&c, &filter))?;
        }
        measurements.push(summary(name, &times));
    }
    times.clear();
    for n in 0..32 {
        timed(&mut times, || {
            c.query_row(
                "SELECT title FROM tracks WHERE source_id=?1 AND path=?2",
                params!["benchmark", make_track(n % count).path.to_string_lossy()],
                |r| r.get::<_, String>(0),
            )
            .map_err(err)
        })?;
    }
    measurements.push(summary("indexed_identity_lookup", &times));
    times.clear();
    for _ in 0..32 {
        timed(&mut times, || {
            let mut q = c
                .prepare("SELECT id FROM tracks WHERE deleted=0 AND title LIKE ?1 LIMIT 64")
                .map_err(err)?;
            let rows = q
                .query_map(["%001%"], |r| r.get::<_, i64>(0))
                .map_err(err)?;
            rows.collect::<rusqlite::Result<Vec<_>>>().map_err(err)
        })?;
    }
    measurements.push(summary("search_like_query", &times));
    let wal_size = fs::metadata(sidecar(&path, "-wal"))
        .map(|m| m.len())
        .unwrap_or(0);
    times.clear();
    timed(&mut times, || {
        c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
            r.get::<_, i64>(0)
        })
        .map_err(err)
        .and_then(|busy| {
            if busy == 0 {
                Ok(())
            } else {
                Err("checkpoint busy".into())
            }
        })
    })?;
    measurements.push(summary("wal_checkpoint_truncate", &times));
    drop(c);
    // No global drop_caches or claim of electrically cold storage.
    for name in ["reopen_first_connection", "reopen_warm_connection"] {
        times.clear();
        c = timed(&mut times, || open(&path).map_err(open_message))?;
        measurements.push(summary(name, &times));
        drop(c);
    }
    c = open(&path).map_err(open_message)?;
    times.clear();
    let tracks = timed(&mut times, || catalog(&mut c))?;
    measurements.push(summary("startup_full_library_load", &times));
    let check = validate(&c)?;
    if check["tracks"].as_u64() != Some(count as u64) {
        return Err("population_count_mismatch".into());
    }
    let report = json!({"schema_version":SCHEMA_VERSION,"sqlite_version":rusqlite::version(),
        "tracks":count,"measurements":measurements,"db_bytes":fs::metadata(&path).map_err(|e|e.to_string())?.len(),
        "wal_bytes_before_checkpoint":wal_size,"cache_state":"OS page cache uncontrolled; reopen is not cold-device evidence",
        "journal_mode":"WAL","synchronous":"NORMAL","batch_tracks":64,"quick_check":check,
        "memory":fs::read_to_string("/proc/self/status").ok()});
    Ok((report, tracks))
}

fn wave() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&100u32.to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&44100u32.to_le_bytes());
    bytes.extend_from_slice(&176400u32.to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&64u32.to_le_bytes());
    bytes.extend_from_slice(&[0; 64]);
    bytes
}

pub fn scanner(root: &Path, count: usize) -> Result<Value, String> {
    if !(1..=20_000).contains(&count) {
        return Err("dataset out of range".into());
    }
    let media = root.join("scan-media");
    fs::create_dir(&media).map_err(|e| e.to_string())?;
    let bytes = wave();
    for i in 0..count {
        let path = media.join(format!("{i:05}.wav"));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    let log = Observer::new(&root.join("logs")).map_err(|e| e.to_string())?;
    let db = Database::spawn(root.join("scan.sqlite"), log.clone())?;
    let source = Source {
        id: "benchmark-scan".into(),
        kind: MediaSource::Internal,
        root: media.clone(),
        online: true,
        mount: "scratch fixture".into(),
        mount_id: None,
    };
    db.sources(vec![source.clone()])?;
    let scanner = Scanner::spawn(db.clone(), log)?;
    let mut results = vec![];
    for name in ["initial_scan", "incremental_scan"] {
        scanner.scan(vec![source.clone()])?;
        let result = scanner
            .results
            .recv_timeout(Duration::from_secs(900))
            .map_err(|e| e.to_string())??;
        if !result.complete || result.discovered != count as u64 {
            return Err("scan_incomplete".into());
        }
        results.push(json!({"operation":name,"result":result}));
    }
    scanner.stop();
    db.stop();
    Ok(
        json!({"workload":"synthetic short WAV metadata, not audio decode throughput","results":results}),
    )
}

/// Platform pass: deterministic tags and actual scanner reuse, without timing
/// FFmpeg initial parsing (owned by the metadata pass). Each process is isolated.
pub fn platform_db(root: &Path, count: usize) -> Result<(Value, Vec<Track>), String> {
    if ![100, 1_000, 10_000, 25_000, 50_000].contains(&count) {
        return Err("platform dataset must be 100, 1000, 10000, 25000 or 50000".into());
    }
    let media = root.join("media");
    fs::create_dir(&media).map_err(|e| e.to_string())?;
    let mut fixtures = Vec::with_capacity(count);
    let bytes = wave();
    for i in 0..count {
        let album = i / 24;
        let directory = media.join(format!("release{album:05}"));
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        let path = directory.join(format!("{i:05}.wav"));
        fs::write(&path, &bytes).map_err(|e| e.to_string())?;
        let meta = fs::metadata(&path).map_err(|e| e.to_string())?;
        let missing = album % 29 == 0;
        let compilation = album % 7 == 0;
        fixtures.push(Track {
            source_id: "fixture".into(),
            path,
            filename: format!("{i:05}.wav"),
            size: meta.len(),
            mtime: meta
                .modified()
                .map_err(|e| e.to_string())?
                .duration_since(UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos() as i64,
            title: format!("Song {:03}", i % 89),
            artist: if missing {
                String::new()
            } else {
                format!(
                    "Artist {:03}",
                    if compilation { i % 127 } else { album % 127 }
                )
            },
            album: if missing {
                String::new()
            } else {
                format!("Release {:04}", album % 509)
            },
            album_artist: if missing {
                String::new()
            } else if compilation {
                "Various Artists".into()
            } else {
                format!("Artist {:03}", album % 127)
            },
            track: (i % 12 + 1) as u32,
            disc: (i % 24 / 12 + 1) as u32,
            duration_ms: 180_000 + (i % 180) as u64 * 1000,
            codec: "pcm_s16le".into(),
            sample_rate: 44_100,
            channels: 2,
            bitrate: 1_411_200,
            online: true,
            ..Track::default()
        });
    }
    let mut measurements = vec![];
    let mut times = vec![];
    let path = root.join("library.sqlite");
    let mut c = open(&path).map_err(open_message)?;
    for repetition in 0..5 {
        let trial = root.join(format!("population{repetition}.sqlite"));
        let mut populate = open(&trial).map_err(open_message)?;
        populate
            .execute(
                "INSERT INTO sources VALUES('fixture',?1,1)",
                [media.to_string_lossy()],
            )
            .map_err(err)?;
        timed(&mut times, || {
            for chunk in fixtures.chunks(64) {
                batch(&mut populate, chunk.to_vec(), 1)?;
            }
            Ok(())
        })?;
        drop(populate);
        fs::remove_file(trial).map_err(|e| e.to_string())?;
    }
    measurements.push(summary("initial_population", &times));
    c.execute(
        "INSERT INTO sources VALUES('fixture',?1,1)",
        [media.to_string_lossy()],
    )
    .map_err(err)?;
    for chunk in fixtures.chunks(64) {
        batch(&mut c, chunk.to_vec(), 1)?;
    }
    let album = fixtures
        .iter()
        .find(|t| !t.album.is_empty())
        .unwrap()
        .album
        .clone();
    let artist = fixtures
        .iter()
        .find(|t| !t.artist.is_empty())
        .unwrap()
        .artist
        .clone();
    for (name, f) in [
        (
            "songs_page",
            Filter {
                limit: 64,
                ..Default::default()
            },
        ),
        (
            "deep_page",
            Filter {
                limit: 64,
                offset: count.saturating_sub(64),
                ..Default::default()
            },
        ),
        (
            "album_contents",
            Filter {
                album: Some(album.clone()),
                limit: 512,
                ..Default::default()
            },
        ),
        (
            "artist_contents",
            Filter {
                artist: Some(artist.clone()),
                limit: 512,
                ..Default::default()
            },
        ),
    ] {
        times.clear();
        for _ in 0..41 {
            timed(&mut times, || list(&c, &f))?;
        }
        measurements.push(summary(name, &times));
    }
    let queries = [
        ("songs_page",format!("{SELECT} WHERE t.deleted=0 ORDER BY t.artist,t.album,t.disc,t.track,t.title,t.path LIMIT 64")),
        ("artists_sql", "SELECT DISTINCT artist FROM tracks WHERE deleted=0 ORDER BY artist".into()),
        ("albums_sql", "SELECT DISTINCT album_artist,album FROM tracks WHERE deleted=0 ORDER BY album_artist,album".into()),
        ("album_contents",format!("{SELECT} WHERE t.deleted=0 AND t.album='Release 0001' ORDER BY t.artist,t.album,t.disc,t.track,t.title,t.path LIMIT 512")),
        ("artist_contents",format!("{SELECT} WHERE t.deleted=0 AND t.artist='Artist 001' ORDER BY t.artist,t.album,t.disc,t.track,t.title,t.path LIMIT 512")),
        ("search_diagnostic", "SELECT id FROM tracks WHERE deleted=0 AND (title LIKE '%Song 088%' OR artist LIKE '%088%' OR album LIKE '%088%') LIMIT 64".into()),
        ("now_playing_id", "SELECT title FROM tracks WHERE id=7".into()),
        ("prune_legacy", "UPDATE tracks SET deleted=1 WHERE source_id='fixture' AND seen<>2".into()),
    ];
    let mut plans = serde_json::Map::new();
    for (name, sql) in queries {
        let mut q = c
            .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
            .map_err(err)?;
        let plan = q
            .query_map([], |r| r.get::<_, String>(3))
            .map_err(err)?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(err)?;
        plans.insert(name.into(), json!(plan));
        if name.ends_with("sql") || name == "search_diagnostic" || name == "now_playing_id" {
            times.clear();
            for _ in 0..41 {
                timed(&mut times, || {
                    let mut q = c.prepare_cached(&sql).map_err(err)?;
                    let mut rows = q.query([]).map_err(err)?;
                    while rows.next().map_err(err)?.is_some() {}
                    Ok(())
                })?;
            }
            measurements.push(summary(name, &times));
        }
    }
    let mutation_paths: Vec<_> = fixtures.iter().take(21).map(|t| t.path.clone()).collect();
    drop(fixtures);
    times.clear();
    let mut library = vec![];
    for _ in 0..7 {
        library = timed(&mut times, || catalog(&mut c))?;
    }
    measurements.push(summary("startup_library_pages", &times));
    let settings = json!({"journal_mode":c.pragma_query_value(None,"journal_mode",|r|r.get::<_,String>(0)).map_err(err)?,
        "synchronous":c.pragma_query_value(None,"synchronous",|r|r.get::<_,i64>(0)).map_err(err)?,
        "cache_size":c.pragma_query_value(None,"cache_size",|r|r.get::<_,i64>(0)).map_err(err)?,
        "page_size":c.pragma_query_value(None,"page_size",|r|r.get::<_,i64>(0)).map_err(err)?,
        "wal_autocheckpoint":c.pragma_query_value(None,"wal_autocheckpoint",|r|r.get::<_,i64>(0)).map_err(err)?});
    drop(c);
    times.clear();
    for _ in 0..21 {
        timed(&mut times, || open(&path).map_err(open_message))?;
    }
    measurements.push(summary("database_open", &times));
    let log = Observer::new(&root.join("logs")).map_err(|e| e.to_string())?;
    let db = Database::spawn(path.clone(), log.clone())?;
    let source = Source {
        id: "fixture".into(),
        kind: MediaSource::Internal,
        root: media.clone(),
        online: true,
        mount: "scratch".into(),
        mount_id: None,
    };
    db.sources(vec![source.clone()])?;
    let scanner = Scanner::spawn(db.clone(), log)?;
    let mut scans = vec![];
    let before_writes = proc_write_bytes();
    times.clear();
    for _ in 0..7 {
        let result = timed(&mut times, || {
            scanner.scan(vec![source.clone()])?;
            scanner
                .results
                .recv_timeout(Duration::from_secs(180))
                .map_err(|e| e.to_string())?
        })?;
        if !result.complete || result.reused != count as u64 {
            return Err("unchanged scan mismatch".into());
        }
        scans.push(json!(result));
    }
    measurements.push(summary("unchanged_scan", &times));
    let unchanged_write_bytes = proc_write_bytes().saturating_sub(before_writes);
    // Real mutation: new, changed, removed and rename. Scan has no destructive
    // replacement, and unrelated rows retain their stable IDs.
    times.clear();
    let mut mutations = vec![];
    for sample in 0..7 {
        fs::write(media.join(format!("new{sample}.wav")), &bytes).map_err(|e| e.to_string())?;
        fs::remove_file(&mutation_paths[sample * 3]).map_err(|e| e.to_string())?;
        fs::rename(
            &mutation_paths[sample * 3 + 1],
            media.join(format!("moved{sample}.wav")),
        )
        .map_err(|e| e.to_string())?;
        fs::OpenOptions::new()
            .append(true)
            .open(&mutation_paths[sample * 3 + 2])
            .map_err(|e| e.to_string())?
            .write_all(&[0; 8])
            .map_err(|e| e.to_string())?;
        let mutation = timed(&mut times, || {
            scanner.scan(vec![source.clone()])?;
            scanner
                .results
                .recv_timeout(Duration::from_secs(180))
                .map_err(|e| e.to_string())?
        })?;
        if !mutation.complete || mutation.rescanned != 3 || mutation.reused != (count - 3) as u64 {
            return Err("mutation count mismatch".into());
        }
        mutations.push(mutation);
    }
    measurements.push(summary("incremental_mutation_scan", &times));
    let check = db
        .test()?
        .recv_timeout(Duration::from_secs(15))
        .map_err(|e| e.to_string())??;
    if check["tracks"] != count {
        return Err("mutation row count mismatch".into());
    }
    let replacement = Source {
        id: "replacement".into(),
        ..source.clone()
    };
    times.clear();
    for _ in 0..21 {
        timed(&mut times, || {
            db.sources(vec![replacement.clone()])?;
            let rows = db
                .list(Filter {
                    limit: 1,
                    ..Default::default()
                })?
                .recv_timeout(Duration::from_secs(15))
                .map_err(|e| e.to_string())??;
            if rows.first().is_none_or(|t| t.online) {
                return Err("old media still online".into());
            }
            Ok(())
        })?;
        db.sources(vec![source.clone()])?;
        db.list(Filter {
            limit: 1,
            ..Default::default()
        })?
        .recv_timeout(Duration::from_secs(15))
        .map_err(|e| e.to_string())??;
    }
    measurements.push(summary("replace_media_identity", &times));
    scanner.stop();
    db.shutdown(Duration::from_secs(15))?;
    c = open(&path).map_err(open_message)?;
    c.execute_batch("CREATE TEMP TABLE IF NOT EXISTS scan_seen(id INTEGER PRIMARY KEY);")
        .map_err(err)?;
    times.clear();
    for _ in 0..7 {
        c.execute_batch("UPDATE tracks SET deleted=0,seen=CASE WHEN id%100=0 THEN 1 ELSE 2 END; DELETE FROM scan_seen; INSERT INTO scan_seen SELECT id FROM tracks WHERE id%100<>0;").map_err(err)?;
        timed(&mut times, || benchmark_prune(&mut c, &source))?;
    }
    measurements.push(summary("delete_missing_rows", &times));
    c.execute("UPDATE tracks SET deleted=0 WHERE deleted<>0", [])
        .map_err(err)?;
    // Restore tombstones from the seven controlled remove/move iterations.
    for file in mutation_paths
        .iter()
        .take(21)
        .enumerate()
        .filter(|(i, _)| i % 3 != 2)
        .map(|(_, p)| p)
    {
        c.execute(
            "UPDATE tracks SET deleted=1 WHERE source_id='fixture' AND path=?1",
            [file.to_string_lossy()],
        )
        .map_err(err)?;
    }
    let count_rows: i64 = c
        .query_row("SELECT count(*) FROM tracks WHERE deleted=0", [], |r| {
            r.get(0)
        })
        .map_err(err)?;
    let retained_rows: i64 = c
        .query_row("SELECT count(*) FROM tracks", [], |r| r.get(0))
        .map_err(err)?;
    Ok((
        json!({"tracks":count,"fixture":"deterministic v1; 24-track multi-disc, 127 artists, compilations, missing tags, duplicate titles/releases",
        "sqlite_version":rusqlite::version(),"measurements":measurements,"query_plans":plans,"settings":settings,
        "db_bytes":fs::metadata(&path).map_err(|e|e.to_string())?.len(),"unchanged_scan_write_bytes":unchanged_write_bytes,
        "write_metric":"/proc/self/io write_bytes; includes log/temp I/O, excludes already dirty pages and physical flash amplification",
        "unchanged_scans":scans,"mutations":mutations,"live_rows":count_rows,"retained_rows":retained_rows,
        "memory":fs::read_to_string("/proc/self/status").ok()}),
        library,
    ))
}

fn proc_write_bytes() -> u64 {
    fs::read_to_string("/proc/self/io")
        .ok()
        .and_then(|s| {
            s.lines().find_map(|line| {
                line.strip_prefix("write_bytes: ")
                    .and_then(|v| v.trim().parse().ok())
            })
        })
        .unwrap_or(0)
}

fn benchmark_prune(c: &mut Connection, source: &Source) -> Result<(), String> {
    finish(c, source, true).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disposable_benchmark_exercises_current_schema_and_refuses_existing_database() {
        let root =
            std::env::temp_dir().join(format!("reborn-platform-bench-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        let (report, tracks) = database(&root, 128).unwrap();
        assert_eq!(tracks.len(), 128);
        assert_eq!(report["quick_check"]["tracks"], 128);
        assert_eq!(report["batch_tracks"], 64);
        assert!(database(&root, 128).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
