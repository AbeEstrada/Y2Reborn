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
use std::sync::Mutex;

/// At most this many plays wait; the oldest go first beyond it.
pub const QUEUE_MAX: usize = 10_000;
/// Last.fm ignores plays older than about two weeks.
pub const MAX_AGE_S: i64 = 14 * 24 * 3600;
pub const LOG_NAME: &str = ".scrobbler.log";

/// One lock for every queue operation in the process: the sender thread
/// rewrites the file while the player may append a play that just counted.
static LOCK: Mutex<()> = Mutex::new(());

fn parse(bytes: &[u8]) -> Vec<Scrobble> {
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|line| serde_json::from_slice::<Scrobble>(line).ok())
        .collect()
}

#[derive(Debug, Clone)]
pub struct Queue {
    path: PathBuf,
    max: usize,
}
impl Queue {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            max: QUEUE_MAX,
        }
    }
    /// A queue with another cap (tests).
    pub fn with_max(path: impl Into<PathBuf>, max: usize) -> Self {
        Self {
            path: path.into(),
            max: max.max(1),
        }
    }
    fn lock() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Every complete entry, oldest first.
    pub fn load(&self) -> Vec<Scrobble> {
        let _guard = Self::lock();
        parse(&fs::read(&self.path).unwrap_or_default())
    }
    pub fn len(&self) -> usize {
        self.load().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Add one play durably (one read, one append; a rewrite only at the cap).
    pub fn push(&self, s: &Scrobble) -> io::Result<()> {
        let _guard = Self::lock();
        if let Some(dir) = self.path.parent() {
            fs::create_dir_all(dir)?;
        }
        let existing = fs::read(&self.path).unwrap_or_default();
        if existing.iter().filter(|&&b| b == b'\n').count() >= self.max {
            let mut all = parse(&existing);
            all.push(s.clone());
            let keep = all.len().saturating_sub(self.max);
            return self.rewrite(&all[keep..]);
        }
        // A torn previous line would glue onto this one: start on a fresh line.
        let torn = existing.last().is_some_and(|&b| b != b'\n');
        let mut line = serde_json::to_string(s).map_err(io::Error::other)?;
        line.push('\n');
        if torn {
            line.insert(0, '\n');
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        file.sync_data()
    }
    /// Remove the first `n` entries (they were sent or refused for good).
    pub fn drop_first(&self, n: usize) -> io::Result<()> {
        let _guard = Self::lock();
        let all = parse(&fs::read(&self.path).unwrap_or_default());
        self.rewrite(&all[n.min(all.len())..])
    }
    /// Remove `sent` from the front, but only while the front still is
    /// exactly those plays: a send that finishes after a Disconnect (and a
    /// new account's first plays) must not remove anything it did not send.
    /// Returns whether they were removed.
    pub fn drop_sent(&self, sent: &[Scrobble]) -> io::Result<bool> {
        let _guard = Self::lock();
        let all = parse(&fs::read(&self.path).unwrap_or_default());
        if !all.starts_with(sent) {
            return Ok(false);
        }
        self.rewrite(&all[sent.len()..])?;
        Ok(true)
    }
    /// Called with the lock held.
    fn rewrite(&self, entries: &[Scrobble]) -> io::Result<()> {
        let tmp = self.path.with_extension("jsonl.tmp");
        {
            let mut file = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            let mut text = String::new();
            for s in entries {
                text.push_str(&serde_json::to_string(s).map_err(io::Error::other)?);
                text.push('\n');
            }
            file.write_all(text.as_bytes())?;
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
    fn concurrent_appends_survive_a_rewrite_and_the_cap_holds() {
        let root = dir("race");
        let q = Queue::new(root.join("q.jsonl"));
        for i in 0..200 {
            q.push(&s(i)).unwrap();
        }
        let writer = {
            let q = q.clone();
            std::thread::spawn(move || {
                for i in 200..400 {
                    q.push(&s(i)).unwrap();
                }
            })
        };
        for _ in 0..20 {
            q.drop_first(5).unwrap();
        }
        writer.join().unwrap();
        // 400 written, 100 dropped from the front: nothing appended was lost.
        let left = q.load();
        assert_eq!(left.len(), 300);
        assert_eq!(left.last().unwrap(), &s(399));
        // A late send removes only what it sent.
        let sent = q.load()[..2].to_vec();
        q.drop_first(usize::MAX).unwrap();
        q.push(&s(900)).unwrap();
        assert!(!q.drop_sent(&sent).unwrap());
        assert_eq!(q.load(), vec![s(900)]);
        assert!(q.drop_sent(&[s(900)]).unwrap());
        assert!(q.is_empty());
        // At the cap the oldest go first.
        let small = Queue::with_max(root.join("small.jsonl"), 3);
        for i in 0..5 {
            small.push(&s(i)).unwrap();
        }
        assert_eq!(small.load(), vec![s(2), s(3), s(4)]);
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
