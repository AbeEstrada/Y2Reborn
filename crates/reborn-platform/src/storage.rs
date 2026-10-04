#![forbid(unsafe_code)]
use reborn_core::{MediaSource, Source, Track};
use std::{
    fs,
    os::fd::AsRawFd,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub struct PinnedTrack {
    _file: fs::File,
    _parent: fs::File,
    path: PathBuf,
}
pub struct PinnedSidecar {
    _file: fs::File,
    path: PathBuf,
}
impl PinnedSidecar {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl PinnedTrack {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn open_sidecar(&self, name: &str) -> Result<PinnedSidecar, String> {
        let mut components = Path::new(name).components();
        let Some(std::path::Component::Normal(name)) = components.next() else {
            return Err("invalid sidecar name".into());
        };
        if components.next().is_some() {
            return Err("invalid sidecar name".into());
        }
        let file = source_file(&self._parent, name)?;
        Ok(PinnedSidecar {
            path: PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
            _file: file,
        })
    }
}
fn source_file(parent: &fs::File, name: &std::ffi::OsStr) -> Result<fs::File, String> {
    let path = PathBuf::from(format!("/proc/self/fd/{}", parent.as_raw_fd())).join(name);
    let file = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|e| e.to_string())?;
    let metadata = file.metadata().map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.dev() != parent.metadata().map_err(|e| e.to_string())?.dev()
        || descriptor_mount(&file)? != descriptor_mount(parent)?
    {
        return Err("not a regular source file".into());
    }
    Ok(file)
}
fn descriptor_mount(file: &fs::File) -> Result<u64, String> {
    fs::read_to_string(format!("/proc/self/fdinfo/{}", file.as_raw_fd()))
        .map_err(|e| e.to_string())?
        .lines()
        .find_map(|line| line.strip_prefix("mnt_id:")?.trim().parse().ok())
        .ok_or_else(|| "source descriptor mount unavailable".into())
}
fn pin_source_path(track: &Track, source: &Source) -> Result<PinnedTrack, String> {
    if !source.online || source.id != track.source_id {
        return Err("track source unavailable".into());
    }
    let relative = track
        .path
        .strip_prefix(&source.root)
        .map_err(|_| "track outside source")?;
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("track outside source".into());
    }
    let mut root = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&source.root)
        .map_err(|e| e.to_string())?;
    if !root.metadata().map_err(|e| e.to_string())?.is_dir() {
        return Err("track source unavailable".into());
    }
    let mount = descriptor_mount(&root)?;
    if source.mount_id.is_some_and(|expected| mount != expected) {
        return Err("track source mount changed".into());
    }
    let device = root.metadata().map_err(|e| e.to_string())?.dev();
    let components = relative.components().collect::<Vec<_>>();
    for component in &components[..components.len() - 1] {
        let path = PathBuf::from(format!("/proc/self/fd/{}", root.as_raw_fd()))
            .join(component.as_os_str());
        let next = fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|e| e.to_string())?;
        if next.metadata().map_err(|e| e.to_string())?.dev() != device
            || descriptor_mount(&next)? != mount
        {
            return Err("track crosses source filesystem".into());
        }
        root = next;
    }
    let file = source_file(&root, components.last().unwrap().as_os_str())?;
    Ok(PinnedTrack {
        path: PathBuf::from(format!("/proc/self/fd/{}", file.as_raw_fd())),
        _file: file,
        _parent: root,
    })
}
/// Validate the live card claim and pin a regular file before decoder admission.
/// Keep this value alive for the decoder's lifetime, including queued tracks.
pub fn pin_track(track: &Track, music: &Path) -> Result<PinnedTrack, String> {
    let current_sources = || {
        let mut current = sources(music);
        // Desktop/headless fixtures use an explicit directory without Y2DATA.
        // Managed Y2 media always requires its real platform mount/claim.
        if !platform_manages_media() && !current.iter().any(|s| s.id == "internal") {
            current.push(Source {
                id: "internal".into(),
                kind: MediaSource::Internal,
                root: music.into(),
                mount: "configured directory".into(),
                mount_id: None,
                online: music.is_dir(),
            });
        }
        current
    };
    let source = current_sources()
        .into_iter()
        .find(|s| s.online && s.id == track.source_id)
        .ok_or("track source unavailable")?;
    let pinned = pin_source_path(track, &source)?;
    if !current_sources().contains(&source) {
        return Err("track source changed during open".into());
    }
    Ok(pinned)
}
#[derive(Clone, Debug)]
pub struct Mount {
    pub device: String,
    pub path: PathBuf,
    pub filesystem: String,
    pub options: String,
}
fn unescape(s: &str) -> String {
    s.replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}
pub fn parse_mounts(s: &str) -> Vec<Mount> {
    s.lines()
        .filter_map(|l| {
            let v = l.split_whitespace().collect::<Vec<_>>();
            if v.len() < 4 {
                return None;
            }
            Some(Mount {
                device: unescape(v[0]),
                path: unescape(v[1]).into(),
                filesystem: v[2].into(),
                options: v[3].into(),
            })
        })
        .collect()
}
pub fn parse_mountinfo(s: &str) -> Vec<(u64, PathBuf)> {
    s.lines()
        .filter_map(|line| {
            let (mount, _) = line.split_once(" - ")?;
            let fields = mount.split_whitespace().collect::<Vec<_>>();
            Some((
                fields.first()?.parse().ok()?,
                unescape(fields.get(4)?).into(),
            ))
        })
        .collect()
}
pub fn mounts() -> Vec<Mount> {
    parse_mounts(&fs::read_to_string("/proc/mounts").unwrap_or_default())
}
pub fn sources(music: &Path) -> Vec<Source> {
    let m = mounts();
    let mountinfo =
        parse_mountinfo(&fs::read_to_string("/proc/self/mountinfo").unwrap_or_default());
    let mut result = vec![];
    for (mount_path, internal) in [("/data", true), ("/media/sd", false)] {
        if let Some(m) = m.iter().find(|m| m.path == Path::new(mount_path)) {
            if !internal && !Path::new(&m.device).exists() {
                continue;
            }
            let mut uuid =
                super::filesystem_uuid(&m.device).unwrap_or_else(|| format!("device:{}", m.device));
            let observed_mount = mountinfo
                .iter()
                .find_map(|(id, path)| (path == Path::new(mount_path)).then_some(*id));
            let mut sd_identity = None;
            if !internal && Path::new("/etc/y2linux/capabilities.json").exists() {
                let claim = fs::read_to_string("/run/y2/media-mount.json")
                    .ok()
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
                let boot =
                    fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
                if uuid.starts_with("device:") {
                    if let Some(identity) = claim.as_ref().and_then(media_claim_identity) {
                        uuid = identity;
                    }
                }
                let instance = claim.as_ref().and_then(|v| {
                    let device = v["device_id"].as_str()?;
                    if device.len() > 24 || !device.bytes().all(|b| b.is_ascii_digit() || b == b':')
                    {
                        return None;
                    }
                    block_instance(&PathBuf::from(format!("/sys/dev/block/{device}")))
                });
                if !sd_claim_matches(
                    claim.as_ref(),
                    &uuid,
                    observed_mount,
                    boot.trim(),
                    instance.as_deref(),
                ) {
                    continue;
                }
                sd_identity = claim.as_ref().and_then(media_claim_identity);
                if sd_identity.is_none() {
                    continue;
                }
            }
            result.push(Source {
                id: if internal {
                    "internal".into()
                } else {
                    sd_source_id(&uuid, sd_identity.as_deref())
                },
                kind: if internal {
                    MediaSource::Internal
                } else {
                    MediaSource::SdCard(uuid)
                },
                root: if internal {
                    music.into()
                } else {
                    mount_path.into()
                },
                online: true,
                mount: m.device.clone(),
                mount_id: observed_mount,
            });
        }
    }
    result
}
fn block_instance(entry: &Path) -> Option<String> {
    let entry = fs::canonicalize(entry).ok()?;
    let disk = if entry.join("partition").is_file() {
        entry.parent()?
    } else {
        &entry
    };
    let dev = fs::read_to_string(entry.join("dev")).ok()?;
    let dev = dev.trim();
    let (major, minor) = dev.split_once(':')?;
    if major.is_empty()
        || minor.is_empty()
        || !major
            .bytes()
            .chain(minor.bytes())
            .all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let seq = fs::read_to_string(disk.join("diskseq")).ok()?;
    let seq = seq.trim();
    if seq.parse::<u64>().ok()? == 0 || !seq.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(format!("block:{dev}:diskseq:{seq}"))
}
fn sd_claim_matches(
    claim: Option<&serde_json::Value>,
    uuid: &str,
    mount: Option<u64>,
    boot: &str,
    instance: Option<&str>,
) -> bool {
    claim.is_some_and(|c| {
        c["schema"].as_u64() == Some(1)
            && !boot.is_empty()
            && c["boot_id"].as_str() == Some(boot)
            && (c["uuid"].as_str() == Some(uuid)
                || (c["uuid"].is_null()
                    && c["identity"]["id"]
                        .as_str()
                        .is_some_and(|id| uuid == format!("cid:{id}"))))
            && mount.is_some()
            && c["mount_id"].as_u64() == mount
            && c["state"].as_str() == Some("Ready")
            && instance.is_some()
            && c["source_instance"].as_str() == instance
    })
}
fn media_claim_identity(claim: &serde_json::Value) -> Option<String> {
    let device = claim["device_id"].as_str()?;
    if !device.bytes().all(|b| b.is_ascii_digit() || b == b':') || device.len() > 24 {
        return None;
    }
    let entry = fs::canonicalize(format!("/sys/dev/block/{device}")).ok()?;
    if !entry.to_string_lossy().contains("/11240000.mmc/") {
        return None;
    }
    media_claim_identity_at(claim, &entry)
}
fn media_claim_identity_at(claim: &serde_json::Value, entry: &Path) -> Option<String> {
    for field in ["start", "size"] {
        let current = if field == "start" && !entry.join("partition").exists() {
            "0".into()
        } else {
            fs::read_to_string(entry.join(field)).ok()?
        };
        if claim["identity"]["geometry"][field].as_str() != Some(current.trim()) {
            return None;
        }
    }
    let disk = if entry.join("partition").exists() {
        entry.parent()?
    } else {
        entry
    };
    let cid = fs::read_to_string(disk.join("device/cid")).ok()?;
    if claim["identity"]["cid"].as_str() != Some(cid.trim()) {
        return None;
    }
    let id = claim["identity"]["id"].as_str()?;
    (id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit())).then(|| format!("cid:{id}"))
}
fn sd_source_id(uuid: &str, identity: Option<&str>) -> String {
    // Duplicate filesystem UUIDs on different cards must not alias a library.
    match identity {
        Some(id) if id != uuid => format!("uuid:{uuid}:{id}"),
        _ => format!("uuid:{uuid}"),
    }
}
pub fn platform_manages_media() -> bool {
    Path::new("/etc/y2linux/platform-contract").exists()
}
pub fn data_ready() -> bool {
    mounts()
        .iter()
        .any(|m| m.path == Path::new("/data") && m.filesystem == "ext4")
        && fs::read_to_string("/data/.y2data-schema").is_ok_and(|s| s.trim() == "1")
}
pub fn unmount_sd() -> Result<(), String> {
    helper("unmount")
}
pub fn mount_sd() -> Result<(), String> {
    if mounts().iter().any(|m| m.path == Path::new("/media/sd")) {
        return Ok(());
    }
    helper("mount")
}
fn helper(action: &str) -> Result<(), String> {
    // The platform owns filesystem selection/partition policy; no arbitrary shell or device argument.
    let mut child = Command::new("/usr/sbin/y2-media")
        .arg(action)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let until = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err("no unique supported SD filesystem".into())
            };
        }
        if Instant::now() >= until {
            let _ = child.kill();
            let _ = child.wait();
            return Err("mount helper timeout".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
pub fn sd_present() -> bool {
    fs::read_dir("/sys/class/block")
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .any(|e| {
            fs::canonicalize(e.path()).is_ok_and(|p| p.to_string_lossy().contains("/11240000.mmc/"))
        })
}

/// Durable changes only: incomplete copies stay hidden and never trigger a scan.
pub fn usb_changes() -> (u64, bool) {
    let receipt = fs::read_to_string("/run/y2/usb-transfer.json")
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
    let boot = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap_or_default();
    receipt
        .filter(|r| {
            r["schema"] == "org.y2linux.usb-transfer/v1"
                && !boot.trim().is_empty()
                && r["boot_id"].as_str() == Some(boot.trim())
        })
        .map(|r| {
            (
                r["refresh_generation"].as_u64().unwrap_or(0),
                r["active"] == true,
            )
        })
        .unwrap_or((0, false))
}

#[derive(Default)]
pub struct TransferRefresh {
    seen: u64,
    requested: u64,
    quiet_since: Option<Instant>,
    copying: bool,
}
impl TransferRefresh {
    pub fn observe(&mut self, generation: u64, copying: bool, now: Instant) {
        if generation != 0 && generation != self.seen {
            self.seen = generation;
            self.quiet_since = Some(now);
        }
        if self.copying && !copying {
            self.quiet_since = Some(now);
        }
        self.copying = copying;
    }
    pub fn ready(&self, scanner_busy: bool, media_ready: bool, now: Instant) -> bool {
        self.seen != self.requested
            && !self.copying
            && !scanner_busy
            && media_ready
            && self
                .quiet_since
                .is_some_and(|start| now.duration_since(start) >= Duration::from_secs(2))
    }
    pub fn requested(&mut self) {
        self.requested = self.seen;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn transfer_refresh_coalesces_and_retries_after_busy_or_unstable_media() {
        let mut refresh = TransferRefresh::default();
        let now = Instant::now();
        refresh.observe(5, true, now);
        assert!(!refresh.ready(false, true, now + Duration::from_secs(10)));
        refresh.observe(6, false, now + Duration::from_secs(10));
        assert!(!refresh.ready(false, true, now + Duration::from_secs(11)));
        assert!(!refresh.ready(true, true, now + Duration::from_secs(12)));
        assert!(!refresh.ready(false, false, now + Duration::from_secs(12)));
        assert!(refresh.ready(false, true, now + Duration::from_secs(12)));
        refresh.requested();
        assert!(!refresh.ready(false, true, now + Duration::from_secs(15)));
        refresh.observe(7, false, now + Duration::from_secs(16));
        assert!(refresh.ready(false, true, now + Duration::from_secs(18)));
    }
    #[test]
    fn mount_names_not_block_numbers() {
        let m = parse_mounts(
            "/dev/mmcblk1p7 /data ext4 rw 0 0\n/dev/mmcblk0p1 /media/sd vfat rw 0 0\n",
        );
        assert_eq!(m[0].path, Path::new("/data"));
        assert_eq!(m[1].device, "/dev/mmcblk0p1");
    }
    #[test]
    fn escaped_mount() {
        assert_eq!(
            parse_mounts("/dev/a /media/a\\040b ext4 rw 0 0")[0].path,
            Path::new("/media/a b")
        );
    }

    #[test]
    fn mountinfo_keeps_mount_generation_and_decodes_mountpoint() {
        let parsed = parse_mountinfo(
            "36 25 179:7 / /media/sd\\040card rw,relatime shared:4 - vfat /dev/mmcblk0p1 rw\n",
        );
        assert_eq!(parsed, vec![(36, PathBuf::from("/media/sd card"))]);
    }
    #[test]
    fn disk_instance_survives_kernfs_recreation_and_detects_same_card_reinsert() {
        let root = std::env::temp_dir().join(format!("reborn-storage-{}", std::process::id()));
        let entry = root.join("disk/partition");
        fs::create_dir_all(&entry).unwrap();
        fs::write(entry.join("partition"), "1").unwrap();
        fs::write(entry.join("dev"), "179:9\n").unwrap();
        fs::write(root.join("disk/diskseq"), "17\n").unwrap();
        let before = block_instance(&entry).unwrap();
        fs::remove_dir_all(&entry).unwrap();
        fs::create_dir(&entry).unwrap();
        fs::write(entry.join("partition"), "1").unwrap();
        fs::write(entry.join("dev"), "179:9\n").unwrap();
        assert_eq!(block_instance(&entry).as_deref(), Some(before.as_str()));
        fs::write(root.join("disk/diskseq"), "18").unwrap();
        assert_ne!(block_instance(&entry).as_deref(), Some(before.as_str()));
        fs::remove_file(root.join("disk/diskseq")).unwrap();
        assert!(block_instance(&entry).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn uuidless_whole_card_and_geometry_claims_are_checked() {
        let root =
            std::env::temp_dir().join(format!("reborn-storage-claims-{}", std::process::id()));
        fs::create_dir_all(root.join("device")).unwrap();
        fs::write(root.join("device/cid"), "card-one\n").unwrap();
        fs::write(root.join("size"), "1000000\n").unwrap();
        let id = "a".repeat(64);
        let claim = serde_json::json!({"schema":1,"state":"Ready","uuid":null,
            "boot_id":"boot-one","mount_id":42,"source_instance":"block:179:8:diskseq:17",
            "identity":{"id":id,"cid":"card-one","geometry":{"partition":null,"start":"0","size":"1000000"}}});
        let identity = media_claim_identity_at(&claim, &root).unwrap();
        assert!(sd_claim_matches(
            Some(&claim),
            &identity,
            Some(42),
            "boot-one",
            Some("block:179:8:diskseq:17")
        ));
        fs::write(root.join("size"), "1000001").unwrap();
        assert!(media_claim_identity_at(&claim, &root).is_none());
        fs::write(root.join("size"), "1000000").unwrap();
        fs::write(root.join("device/cid"), "card-two").unwrap();
        assert!(media_claim_identity_at(&claim, &root).is_none());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn duplicate_uuid_cards_have_distinct_library_sources() {
        assert_eq!(sd_source_id("cid:one", Some("cid:one")), "uuid:cid:one");
        assert_ne!(
            sd_source_id("clone-uuid", Some("cid:one")),
            sd_source_id("clone-uuid", Some("cid:two"))
        );
        assert_eq!(
            sd_source_id("clone-uuid", Some("cid:one")),
            sd_source_id("clone-uuid", Some("cid:one"))
        );
    }
    #[test]
    fn new_platform_requires_matching_card_claim_and_boot() {
        let claim = serde_json::json!({"schema":1,"state":"Ready","uuid":"card-one","boot_id":"boot-one","mount_id":42,"source_instance":"1:2"});
        assert!(!sd_claim_matches(
            Some(&claim),
            "card-one",
            Some(42),
            "boot-one",
            Some("3:4")
        ));
        assert!(sd_claim_matches(
            Some(&claim),
            "card-one",
            Some(42),
            "boot-one",
            Some("1:2")
        ));
        assert!(!sd_claim_matches(
            Some(&claim),
            "card-two",
            Some(42),
            "boot-one",
            Some("1:2")
        ));
        assert!(!sd_claim_matches(
            Some(&claim),
            "card-one",
            Some(43),
            "boot-one",
            Some("1:2")
        ));
        assert!(!sd_claim_matches(
            Some(&claim),
            "card-one",
            Some(42),
            "boot-two",
            Some("1:2")
        ));
        assert!(!sd_claim_matches(
            None,
            "card-one",
            Some(42),
            "boot-one",
            Some("1:2")
        ));
    }
}
#[test]
fn playback_pin_rejects_stale_card_id_mount_and_path_escape() {
    let root = std::env::temp_dir().join(format!("reborn-source-pin-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::write(root.join("song.flac"), b"original card").unwrap();
    let source = Source {
        id: "uuid:volume:cid:new".into(),
        kind: MediaSource::SdCard("volume".into()),
        root: root.clone(),
        mount: "fixture".into(),
        mount_id: None,
        online: true,
    };
    let mut track = Track {
        source_id: "uuid:volume".into(),
        path: root.join("song.flac"),
        ..Default::default()
    };
    assert!(pin_source_path(&track, &source).is_err());
    track.source_id = "uuid:volume:cid:old".into();
    assert!(pin_source_path(&track, &source).is_err());
    track.source_id = source.id.clone();
    let wrong_mount = Source {
        mount_id: Some(u64::MAX),
        ..source.clone()
    };
    assert!(pin_source_path(&track, &wrong_mount).is_err());
    track.path = root.join("../outside.flac");
    assert!(pin_source_path(&track, &source).is_err());
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn playback_file_pin_keeps_original_after_file_and_directory_replacement() {
    let root = std::env::temp_dir().join(format!("reborn-path-pin-{}", std::process::id()));
    let media = root.join("media");
    fs::create_dir_all(&media).unwrap();
    fs::write(media.join("音楽.flac"), b"original card").unwrap();
    let track = Track {
        source_id: "internal".into(),
        path: media.join("音楽.flac"),
        ..Default::default()
    };
    let source = Source {
        id: "internal".into(),
        kind: MediaSource::Internal,
        root: media.clone(),
        mount: "fixture".into(),
        mount_id: None,
        online: true,
    };
    let pinned = pin_source_path(&track, &source).unwrap();
    fs::remove_file(&track.path).unwrap();
    fs::write(&track.path, b"replacement file").unwrap();
    assert_eq!(fs::read(pinned.path()).unwrap(), b"original card");
    fs::rename(&media, root.join("detached")).unwrap();
    fs::create_dir(&media).unwrap();
    fs::write(media.join("音楽.flac"), b"replacement card").unwrap();
    assert_eq!(fs::read(pinned.path()).unwrap(), b"original card");
    assert_eq!(fs::read(&track.path).unwrap(), b"replacement card");
    drop(pinned);
    fs::remove_dir_all(root).unwrap();
}
#[test]
fn playback_pin_refuses_symlinks_subdirectories_and_special_files() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let root = std::env::temp_dir().join(format!("reborn-symlink-pin-{}", std::process::id()));
    let media = root.join("media");
    let outside = root.join("outside");
    fs::create_dir_all(&media).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("song.flac"), b"outside source").unwrap();
    fs::write(media.join("song.flac"), b"inside source").unwrap();
    let source = Source {
        id: "internal".into(),
        kind: MediaSource::Internal,
        root: media.clone(),
        mount: "fixture".into(),
        mount_id: None,
        online: true,
    };
    symlink(outside.join("song.flac"), media.join("file.flac")).unwrap();
    symlink(&outside, media.join("subdir")).unwrap();
    symlink(&media, root.join("root-link")).unwrap();
    let socket = UnixListener::bind(media.join("socket.flac")).unwrap();
    assert!(Command::new("mkfifo")
        .arg(media.join("fifo.flac"))
        .status()
        .unwrap()
        .success());
    for name in ["file.flac", "subdir/song.flac", "socket.flac", "fifo.flac"] {
        let track = Track {
            source_id: source.id.clone(),
            path: media.join(name),
            ..Default::default()
        };
        assert!(pin_source_path(&track, &source).is_err(), "{name}");
    }
    let pinned = pin_source_path(
        &Track {
            source_id: source.id.clone(),
            path: media.join("song.flac"),
            ..Default::default()
        },
        &source,
    )
    .unwrap();
    for name in [
        "file.flac",
        "../outside/song.flac",
        "subdir/song.flac",
        "socket.flac",
        "fifo.flac",
    ] {
        assert!(pinned.open_sidecar(name).is_err(), "{name}");
    }
    let file = pinned.open_sidecar("song.flac").unwrap();
    assert_eq!(fs::read(file.path()).unwrap(), b"inside source");
    let linked_source = Source {
        root: root.join("root-link"),
        ..source
    };
    let track = Track {
        source_id: linked_source.id.clone(),
        path: linked_source.root.join("file.flac"),
        ..Default::default()
    };
    assert!(pin_source_path(&track, &linked_source).is_err());
    drop(socket);
    fs::remove_dir_all(root).unwrap();
}
