//! Scrobbles on disk: the durable queue Last.fm is fed from, the optional
//! Rockbox `.scrobbler.log`, and whether the wall clock can be trusted.
//!
//! The queue (`/data/reborn/state/scrobbles.jsonl`) gets one JSON line per
//! counted play, written and fsynced the moment it counts, so sleep, a flat
//! battery or an update lose nothing. A torn last line (power loss mid-write)
//! is ignored and rewritten away. Sent entries leave by an atomic rewrite.
use reborn_core::scrobble::{log_header, log_line, Scrobble};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// At most this many plays wait; the oldest go first beyond it.
pub const QUEUE_MAX: usize = 10_000;
/// Last.fm ignores plays older than about two weeks.
pub const MAX_AGE_S: i64 = 14 * 24 * 3600;
pub const LOG_NAME: &str = ".scrobbler.log";

#[derive(Debug, Clone)]
pub struct Queue {
    path: PathBuf,
}
impl Queue {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn default_path() -> Self {
        Self::new("/data/reborn/state/scrobbles.jsonl")
    }
    /// Every complete entry, oldest first.
    pub fn load(&self) -> Vec<Scrobble> {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return vec![];
        };
        text.lines()
            .filter_map(|line| serde_json::from_str::<Scrobble>(line).ok())
            .collect()
    }
    pub fn len(&self) -> usize {
        self.load().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Add one play durably.
    pub fn push(&self, s: &Scrobble) -> io::Result<()> {
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        // A torn previous line would glue onto this one: start on a fresh line.
        let torn = fs::read(&self.path).is_ok_and(|b| !b.is_empty() && b.last() != Some(&b'\n'));
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        let mut line = serde_json::to_string(s).map_err(io::Error::other)?;
        line.push('\n');
        if torn {
            line.insert(0, '\n');
        }
        file.write_all(line.as_bytes())?;
        file.sync_data()?;
        drop(file);
        if self.load().len() > QUEUE_MAX {
            let keep = self.load();
            self.rewrite(&keep[keep.len() - QUEUE_MAX..])?;
        }
        Ok(())
    }
    /// Remove the first `n` entries (they were sent or refused for good).
    pub fn drop_first(&self, n: usize) -> io::Result<()> {
        let all = self.load();
        self.rewrite(&all[n.min(all.len())..])
    }
    fn rewrite(&self, entries: &[Scrobble]) -> io::Result<()> {
        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            for s in entries {
                let mut line = serde_json::to_string(s).map_err(io::Error::other)?;
                line.push('\n');
                file.write_all(line.as_bytes())?;
            }
            file.sync_all()?;
        }
        fs::rename(&tmp, &self.path)?;
        if let Some(dir) = self.path.parent() {
            File::open(dir)?.sync_all()?;
        }
        Ok(())
    }
}

/// Append one play to `<root>/.scrobbler.log`, with the header on a new file.
pub fn append_log(root: &Path, client: &str, s: &Scrobble) -> io::Result<()> {
    let path = root.join(LOG_NAME);
    let fresh = fs::metadata(&path).map(|m| m.len() == 0).unwrap_or(true);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut text = String::new();
    if fresh {
        text.push_str(&log_header(client));
    }
    text.push_str(&log_line(s));
    file.write_all(text.as_bytes())?;
    file.sync_data()
}

/// Milliseconds since boot, including suspend (CLOCK_BOOTTIME): a play counted
/// before the clock was known is stamped correctly after a sleep.
pub fn boottime_ms() -> u64 {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: clock_gettime writes a valid timespec on success; only then read.
    unsafe {
        if libc::clock_gettime(libc::CLOCK_BOOTTIME, value.as_mut_ptr()) != 0 {
            return 0;
        }
        let value = value.assume_init();
        value.tv_sec as u64 * 1000 + value.tv_nsec as u64 / 1_000_000
    }
}

/// Y2Linux's clock status file.
pub const TIME_STATUS: &str = "/run/y2/time-status.json";

/// The wall clock is trustworthy: Y2Linux set it from NTP or a retained RTC
/// (`/run/y2/time-status.json`, `tls_ready`).
pub fn clock_trusted(status: &Path) -> bool {
    fs::read(status)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("tls_ready").and_then(|t| t.as_bool()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reborn_core::scrobble::Play;

    fn dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("reborn-scrobble-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }
    fn s(n: i64) -> Scrobble {
        Scrobble {
            play: Play {
                artist: "A".into(),
                title: format!("T{n}"),
                duration_s: 200,
                ..Default::default()
            },
            timestamp: 1_700_000_000 + n,
        }
    }

    #[test]
    fn queue_survives_a_torn_line_and_drops_what_was_sent() {
        let root = dir("queue");
        let q = Queue::new(root.join("state/scrobbles.jsonl"));
        assert!(q.is_empty());
        q.push(&s(1)).unwrap();
        q.push(&s(2)).unwrap();
        // Power lost mid-write: a partial third line.
        let mut f = OpenOptions::new()
            .append(true)
            .open(root.join("state/scrobbles.jsonl"))
            .unwrap();
        f.write_all(b"{\"artist\":\"A\",\"ti").unwrap();
        drop(f);
        assert_eq!(q.load(), vec![s(1), s(2)]);
        q.push(&s(3)).unwrap();
        assert_eq!(q.load(), vec![s(1), s(2), s(3)]);
        q.drop_first(2).unwrap();
        assert_eq!(q.load(), vec![s(3)]);
        q.drop_first(9).unwrap();
        assert!(q.is_empty());
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(root.join("state/scrobbles.jsonl"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn log_starts_with_the_header_once() {
        let root = dir("log");
        append_log(&root, "Y2Reborn 1.0", &s(1)).unwrap();
        append_log(&root, "Y2Reborn 1.0", &s(2)).unwrap();
        let text = fs::read_to_string(root.join(LOG_NAME)).unwrap();
        assert_eq!(text.matches("#AUDIOSCROBBLER/1.1").count(), 1);
        assert!(text.ends_with("A\t\tT2\t\t200\tL\t1700000002\t\n"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn clock_trust_follows_the_platform() {
        let root = dir("clock");
        let p = root.join("time-status.json");
        assert!(!clock_trusted(&p));
        fs::write(&p, r#"{"clock_sane":true,"tls_ready":false}"#).unwrap();
        assert!(!clock_trusted(&p));
        fs::write(&p, r#"{"clock_sane":true,"tls_ready":true}"#).unwrap();
        assert!(clock_trusted(&p));
        fs::remove_dir_all(root).unwrap();
    }
}
