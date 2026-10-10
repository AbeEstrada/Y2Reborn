//! Library thumbnails: bounded background decoding with an on-disk cache.
//! Native decoding never runs on the UI thread.
use reborn_core::Track;
use reborn_graphics::THUMB_SIZE;
use reborn_media::{external_artwork, folder_artwork_candidates, Cancel, MediaFile};
use reborn_platform::storage::{pin_track, PinnedTrack};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        mpsc::{channel, Receiver, Sender},
        Arc, Condvar, Mutex,
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Key {
    pub source: String,
    pub id: i64,
    path: PathBuf,
    size: u64,
    mtime: i64,
    mount: Option<u64>,
    embedded: bool,
    music: PathBuf,
}
impl Key {
    pub fn new(track: &Track, mount: Option<u64>, music: &Path) -> Self {
        Self {
            source: track.source_id.clone(),
            id: track.id,
            path: track.path.clone(),
            size: track.size,
            mtime: track.mtime,
            mount,
            embedded: track.artwork,
            music: music.into(),
        }
    }
    /// Size and modification time of the file, part of a cache file's name.
    fn revision(&self) -> (u64, i64) {
        (self.size, self.mtime)
    }
    /// The cover as 160x160 RGBA, if the file is still the one this key names.
    pub fn load_cover(&self, cancel: &Cancel) -> Option<Vec<u8>> {
        let source = self.pin()?;
        let (pixels, _) = load(self, &source, cancel);
        let pixels = pixels.filter(|v| v.len() == 160 * 160 * 4)?;
        source.is_current().then_some(pixels)
    }
    fn pin(&self) -> Option<Arc<PinnedTrack>> {
        let track = Track {
            source_id: self.source.clone(),
            path: self.path.clone(),
            ..Default::default()
        };
        let pin = pin_track(&track, &self.music).ok()?;
        (pin.mount_id() == self.mount
            && pin.matches_revision(self.size, self.mtime)
            && pin.is_current())
        .then(|| Arc::new(pin))
    }
}
/// A thumbnail wanted by the UI: the opaque id the renderer uses, the track
/// whose cover supplies it and the directory of the on-disk cache.
pub struct Job {
    pub thumb: u64,
    pub art: Key,
    pub cache: PathBuf,
}
pub struct Reply {
    pub thumb: u64,
    /// 96x96 RGBA with rounded corners, or `None` when the track has no cover.
    pub rgba: Option<Vec<u8>>,
    pub decode_ms: f64,
    pub cache_hit: bool,
}
#[derive(Default)]
struct Shared {
    queue: VecDeque<Job>,
    current: Option<u64>,
    stop: bool,
}
/// Cached thumbnails are at most this many files (about 55 MB).
const CACHE_LIMIT: usize = 1500;

pub struct Worker {
    shared: Arc<(Mutex<Shared>, Condvar)>,
    pub replies: Receiver<Reply>,
    failed: HashSet<u64>,
}
impl Worker {
    pub fn spawn() -> Result<Self, std::io::Error> {
        let shared = Arc::new((Mutex::new(Shared::default()), Condvar::new()));
        let (tx, replies) = channel();
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || run(&worker, &tx))?;
        Ok(Self {
            shared,
            replies,
            failed: HashSet::new(),
        })
    }
    /// Replace the queue with the thumbnails the screen shows now, most
    /// important first. Work that already failed is not retried until
    /// `forget_failures`; the job being decoded finishes regardless.
    pub fn want(&mut self, jobs: Vec<Job>) {
        let (lock, wake) = &*self.shared;
        let Ok(mut shared) = lock.lock() else {
            return;
        };
        let current = shared.current;
        shared.queue = jobs
            .into_iter()
            .filter(|job| !self.failed.contains(&job.thumb) && Some(job.thumb) != current)
            .collect();
        if !shared.queue.is_empty() {
            wake.notify_one();
        }
    }
    // Used by the player; the benchmark build shares this file without them.
    #[allow(dead_code)]
    pub fn record_failure(&mut self, thumb: u64) {
        if self.failed.len() < 4096 {
            self.failed.insert(thumb);
        }
    }
    #[allow(dead_code)]
    pub fn forget_failures(&mut self) {
        self.failed.clear();
    }
    /// Drop queued work; true once nothing is being decoded.
    pub fn quiesce(&mut self) -> bool {
        let (lock, _) = &*self.shared;
        lock.lock().is_ok_and(|mut shared| {
            shared.queue.clear();
            shared.current.is_none()
        })
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        let (lock, wake) = &*self.shared;
        if let Ok(mut shared) = lock.lock() {
            shared.stop = true;
            shared.queue.clear();
        }
        wake.notify_all();
    }
}

fn cache_path(job: &Job) -> PathBuf {
    let (size, mtime) = job.art.revision();
    job.cache
        .join(format!("thumb-{:016x}-{mtime:x}-{size:x}.t96", job.thumb))
}

fn run(shared: &(Mutex<Shared>, Condvar), tx: &Sender<Reply>) {
    let (lock, wake) = shared;
    // Counted on the first job; every job names the same cache directory.
    let mut cached = 0usize;
    let mut counted = false;
    loop {
        let job = {
            let Ok(mut guard) = lock.lock() else {
                return;
            };
            loop {
                if guard.stop {
                    return;
                }
                if let Some(job) = guard.queue.pop_front() {
                    guard.current = Some(job.thumb);
                    break job;
                }
                guard = match wake.wait(guard) {
                    Ok(g) => g,
                    Err(_) => return,
                };
            }
        };
        if !counted {
            counted = true;
            cached = std::fs::read_dir(&job.cache)
                .map(|d| {
                    d.flatten()
                        .filter(|e| e.file_name().to_string_lossy().starts_with("thumb-"))
                        .count()
                })
                .unwrap_or(0);
        }
        let started = std::time::Instant::now();
        let path = cache_path(&job);
        let hit = std::fs::read(&path)
            .ok()
            .filter(|bytes| bytes.len() == THUMB_SIZE * THUMB_SIZE * 4);
        let (rgba, cache_hit) = match hit {
            Some(bytes) => (Some(bytes), true),
            None => {
                let cancel = Cancel::new().ok();
                let rgba = cancel
                    .and_then(|cancel| job.art.load_cover(&cancel))
                    .and_then(|cover| reborn_ui::art::thumbnail(&cover));
                if let Some(bytes) = &rgba {
                    if cached < CACHE_LIMIT {
                        let temp = path.with_extension("tmp");
                        if std::fs::write(&temp, bytes).is_ok()
                            && std::fs::rename(&temp, &path).is_ok()
                        {
                            cached += 1;
                        } else {
                            let _ = std::fs::remove_file(&temp);
                        }
                    }
                }
                (rgba, false)
            }
        };
        if let Ok(mut guard) = lock.lock() {
            guard.current = None;
        }
        let reply = Reply {
            thumb: job.thumb,
            rgba,
            decode_ms: started.elapsed().as_secs_f64() * 1000.,
            cache_hit,
        };
        if tx.send(reply).is_err() {
            return;
        }
    }
}

fn load(key: &Key, source: &PinnedTrack, cancel: &Cancel) -> (Option<Vec<u8>>, bool) {
    let _cpu_hint = reborn_platform::workload::CpuWorkloadLease::acquire_workload_hint(
        reborn_platform::workload::Class::ArtworkDecode,
    );
    if key.embedded && !cancel.is_cancelled() {
        if let Ok(mut media) = MediaFile::open(source.path(), cancel.clone()) {
            if let Ok(pixels) = media.artwork() {
                return (Some(pixels), true);
            }
        }
    }
    for name in folder_artwork_candidates(&source.sidecar_directory()).unwrap_or_default() {
        if cancel.is_cancelled() {
            break;
        }
        if let Ok(file) = source.open_sidecar(&name) {
            if let Ok(pixels) = external_artwork(file.path()) {
                return (Some(pixels), false);
            }
        }
    }
    (None, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_track(root: &Path, name: &str, id: i64, embedded: bool) -> Track {
        let path = root.join(name);
        let meta = std::fs::metadata(&path).unwrap();
        Track {
            id,
            source_id: "internal".into(),
            path,
            artwork: embedded,
            size: meta.len(),
            mtime: meta
                .modified()
                .unwrap()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
                .min(i64::MAX as u128) as i64,
            ..Default::default()
        }
    }
    fn job(root: &Path, track: &Track, thumb: u64) -> Job {
        Job {
            thumb,
            art: Key::new(track, None, root),
            cache: root.join("cache"),
        }
    }
    fn reply(worker: &mut Worker, job: Job) -> Reply {
        worker.want(vec![job]);
        worker
            .replies
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
    }
    #[test]
    fn removable_mount_and_file_revision_are_part_of_art_identity() {
        let mut t = Track {
            id: 1,
            source_id: "sd".into(),
            path: "/media/sd/a.flac".into(),
            ..Default::default()
        };
        let key = Key::new(&t, Some(10), Path::new("/data/music"));
        assert_ne!(key, Key::new(&t, Some(11), Path::new("/data/music")));
        t.mtime = 4;
        assert_ne!(key, Key::new(&t, Some(10), Path::new("/data/music")));
    }
    #[test]
    fn thumbnails_are_cached_on_disk_and_never_pin_removable_media() {
        let root = std::env::temp_dir().join(format!("reborn-thumbs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("cache")).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fixtures");
        std::fs::copy(fixtures.join("artwork.flac"), root.join("song.flac")).unwrap();
        let track = fixture_track(&root, "song.flac", 1, true);
        let mut worker = Worker::spawn().unwrap();
        let first = reply(&mut worker, job(&root, &track, 7));
        let rgba = first.rgba.expect("embedded cover decodes");
        assert_eq!(rgba.len(), THUMB_SIZE * THUMB_SIZE * 4);
        assert_eq!(rgba[3], 0, "rounded corner is transparent");
        assert!(!first.cache_hit);
        let again = reply(&mut worker, job(&root, &track, 7));
        assert!(again.cache_hit);
        assert_eq!(again.rgba.unwrap(), rgba);
        let open_pins = std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .filter(|path| path.starts_with(&root))
            .count();
        assert_eq!(open_pins, 0, "thumbnail work must not keep SD eject pins");
        // A replaced file has a new revision and therefore a new cache entry.
        let mut changed = track.clone();
        changed.mtime += 1;
        assert!(!reply(&mut worker, job(&root, &changed, 7)).cache_hit);
        drop(worker);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn missing_or_changed_covers_report_failure_and_are_not_retried() {
        let root = std::env::temp_dir().join(format!("reborn-thumbs-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("cache")).unwrap();
        std::fs::write(root.join("bad.flac"), b"bad media").unwrap();
        let bad = fixture_track(&root, "bad.flac", 5, true);
        let mut worker = Worker::spawn().unwrap();
        let r = reply(&mut worker, job(&root, &bad, 9));
        assert!(r.rgba.is_none());
        worker.record_failure(9);
        worker.want(vec![job(&root, &bad, 9)]);
        assert!(worker
            .replies
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err());
        worker.forget_failures();
        assert!(reply(&mut worker, job(&root, &bad, 9)).rgba.is_none());
        // A source replaced in place (same size and mtime) fails the freshness check.
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fixtures");
        std::fs::copy(fixtures.join("artwork.flac"), root.join("song.flac")).unwrap();
        let track = fixture_track(&root, "song.flac", 1, true);
        let key = Key::new(&track, None, &root);
        let source = key.pin().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let modified = std::fs::metadata(root.join("song.flac"))
            .unwrap()
            .modified()
            .unwrap();
        let mut bytes = std::fs::read(root.join("song.flac")).unwrap();
        bytes[0] ^= 1;
        std::fs::write(root.join("song.flac"), bytes).unwrap();
        std::fs::File::open(root.join("song.flac"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        assert!(!source.is_current());
        assert!(key.load_cover(&Cancel::new().unwrap()).is_none());
        drop(worker);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn pinned_sidecar_art_is_fresh_and_cancelled_requests_do_not_fall_through() {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!("reborn-art-sidecar-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fixtures");
        std::fs::copy(fixtures.join("tone.flac"), root.join("song.flac")).unwrap();
        std::fs::write(root.join("CoVeR.jpg"), b"bad cover").unwrap();
        std::fs::copy(
            fixtures.join("artwork-external.png"),
            root.join("FoLdEr.PNG"),
        )
        .unwrap();
        symlink(
            fixtures.join("artwork-external.jpg"),
            root.join("album.jpg"),
        )
        .unwrap();
        let key = Key::new(&fixture_track(&root, "song.flac", 1, false), None, &root);
        let old = key.load_cover(&Cancel::new().unwrap()).unwrap();
        assert_eq!(old, external_artwork(&root.join("FoLdEr.PNG")).unwrap());
        let cancel = Cancel::new().unwrap();
        cancel.cancel();
        assert!(cancel.is_cancelled());
        assert_eq!(load(&key, &key.pin().unwrap(), &cancel), (None, false));
        std::fs::remove_file(root.join("FoLdEr.PNG")).unwrap();
        assert!(key.load_cover(&Cancel::new().unwrap()).is_none());
        std::fs::remove_dir_all(root).unwrap();
    }
}
