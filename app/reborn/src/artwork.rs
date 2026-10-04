//! Bounded collection art loader. Native decoding never runs on the UI thread.
use reborn_core::Track;
use reborn_media::{external_artwork, folder_artwork_candidates, Cancel, MediaFile};
use reborn_platform::storage::{pin_track, PinnedTrack, TrackClaim};
use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc,
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
pub struct Reply {
    pub key: Key,
    pub pixels: Option<Vec<u8>>,
    pub decode_ms: f64,
    pub cache_hit: bool,
    pub source: Option<TrackClaim>,
}
pub struct Worker {
    requests: SyncSender<(Key, Cancel)>,
    pub replies: Receiver<Reply>,
    pub selected: Option<Key>,
    cancel: Option<Cancel>,
    active: Arc<AtomicBool>,
}
struct Cached {
    key: Key,
    pixels: Vec<u8>,
    source: TrackClaim,
}
impl Worker {
    pub fn spawn() -> Result<Self, std::io::Error> {
        let (requests, rx) = sync_channel::<(Key, Cancel)>(1);
        let (tx, replies) = sync_channel(1);
        let active = Arc::new(AtomicBool::new(false));
        let activity = active.clone();
        std::thread::Builder::new()
            .name("collection-art".into())
            .spawn(move || {
                let mut cache: VecDeque<Cached> = VecDeque::new();
                while let Ok((key, cancel)) = rx.recv() {
                    struct Activity<'a>(&'a AtomicBool);
                    impl Drop for Activity<'_> {
                        fn drop(&mut self) {
                            self.0.store(false, Ordering::Release);
                        }
                    }
                    activity.store(true, Ordering::Release);
                    let _activity = Activity(&activity);
                    let started = std::time::Instant::now();
                    if cancel.is_cancelled() {
                        continue;
                    }
                    cache.retain(|entry| {
                        entry
                            .source
                            .matches_revision(entry.key.size, entry.key.mtime)
                            && entry.source.is_current()
                    });
                    let hit = cache.iter().position(|entry| entry.key == key);
                    let (mut pixels, source, cache_hit) = if let Some(i) = hit {
                        let entry = cache.remove(i).expect("known cache position");
                        let result = (Some(entry.pixels.clone()), Some(entry.source.clone()), true);
                        cache.push_back(entry);
                        result
                    } else {
                        let source = key.pin();
                        let (pixels, cacheable) = source
                            .as_ref()
                            .map(|source| load(&key, source, &cancel))
                            .unwrap_or((None, false));
                        let pixels = pixels.filter(|v| v.len() == 160 * 160 * 4);
                        let source = source.map(|pin| pin.claim());
                        if !cancel.is_cancelled()
                            && cacheable
                            && source.as_ref().is_some_and(|s| s.is_current())
                        {
                            if let (Some(pixels), Some(source)) = (&pixels, &source) {
                                if cache.len() == 4 {
                                    cache.pop_front();
                                }
                                cache.push_back(Cached {
                                    key: key.clone(),
                                    pixels: pixels.clone(),
                                    source: source.clone(),
                                });
                            }
                        }
                        (pixels, source, false)
                    };
                    if cancel.is_cancelled() {
                        continue;
                    }
                    if !source.as_ref().is_some_and(|s| s.is_current()) {
                        pixels = None;
                    }
                    // One bounded route result; stale/cancelled work cannot publish artwork.
                    drop(_activity);
                    if tx
                        .send(Reply {
                            key,
                            pixels,
                            decode_ms: started.elapsed().as_secs_f64() * 1000.,
                            cache_hit,
                            source,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            })?;
        Ok(Self {
            requests,
            replies,
            selected: None,
            cancel: None,
            active,
        })
    }
    pub fn quiesce(&mut self) -> bool {
        self.select(None);
        !self.active.load(Ordering::Acquire)
    }
    pub fn select(&mut self, key: Option<Key>) -> bool {
        if key == self.selected {
            return false;
        }
        if let Some(cancel) = self.cancel.take() {
            cancel.cancel();
        }
        if let Some(key) = key {
            let Ok(cancel) = Cancel::new() else {
                return false;
            };
            if self
                .requests
                .try_send((key.clone(), cancel.clone()))
                .is_err()
            {
                return false;
            }
            self.cancel = Some(cancel);
            self.selected = Some(key);
        } else {
            self.selected = None;
        }
        true
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
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
    fn reply(worker: &mut Worker, key: Key) -> Reply {
        worker.select(None);
        assert!(worker.select(Some(key)));
        worker
            .replies
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap()
    }
    #[test]
    fn embedded_cache_survives_failed_requests_and_rejects_changed_sources() {
        let root = std::env::temp_dir().join(format!("reborn-art-cache-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir(&root).unwrap();
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/fixtures");
        std::fs::copy(fixtures.join("artwork.flac"), root.join("song.flac")).unwrap();
        let mut worker = Worker::spawn().unwrap();
        let keys = (0..4)
            .map(|id| Key::new(&fixture_track(&root, "song.flac", id, true), None, &root))
            .collect::<Vec<_>>();
        for key in &keys {
            let r = reply(&mut worker, key.clone());
            assert!(r.pixels.is_some());
            assert!(!r.cache_hit);
        }
        let open_pins = std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|e| std::fs::read_link(e.path()).ok())
            .filter(|path| path.starts_with(&root))
            .count();
        assert_eq!(open_pins, 0, "art cache must not keep SD eject pins");
        std::fs::write(root.join("bad.flac"), b"bad media").unwrap();
        let bad = Key::new(&fixture_track(&root, "bad.flac", 5, true), None, &root);
        assert!(reply(&mut worker, bad).pixels.is_none());
        for key in &keys {
            assert!(reply(&mut worker, key.clone()).cache_hit);
        }
        let source = keys[0].pin().unwrap();
        // In-place replacement with preserved size/mtime still changes ctime.
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
        let r = reply(&mut worker, keys[0].clone());
        assert!(!r.cache_hit);
        assert!(r.pixels.is_none());
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
        let mut worker = Worker::spawn().unwrap();
        let old = reply(&mut worker, key.clone());
        assert!(!old.cache_hit);
        assert_eq!(
            old.pixels.unwrap(),
            external_artwork(&root.join("FoLdEr.PNG")).unwrap()
        );
        std::fs::copy(
            fixtures.join("artwork-external.jpg"),
            root.join("FoLdEr.PNG"),
        )
        .unwrap();
        // The named extension is misleading, but pinned file header detection remains authoritative.
        let current = reply(&mut worker, key.clone());
        assert!(!current.cache_hit);
        assert!(current.pixels.is_some());
        let cancel = Cancel::new().unwrap();
        cancel.cancel();
        assert!(cancel.is_cancelled());
        assert_eq!(load(&key, &key.pin().unwrap(), &cancel), (None, false));
        std::fs::remove_file(root.join("FoLdEr.PNG")).unwrap();
        assert!(reply(&mut worker, key).pixels.is_none());
        drop(worker);
        std::fs::remove_dir_all(root).unwrap();
    }
}
