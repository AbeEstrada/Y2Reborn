#![forbid(unsafe_code)]
use reborn_core::{MediaSource, Source};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};
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
#[cfg(test)]
mod tests {
    use super::*;
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
