//! The one Reborn client of the Y2Linux platform service contract.
//!
//! A single worker thread runs bounded `y2-platform` operations with a fixed
//! argv (no shell). Records are validated against their `/v1` schema and
//! projected into `reborn_core::platform` types here; the UI never receives
//! platform JSON. Observation is on demand: the application asks for a
//! refresh only while a platform-backed screen is visible and never while the
//! display is off.
use reborn_core::{
    platform::{
        BluetoothQuality, DiagnosticSection, DownloadLive, Fact, HealthLevel, LdacQuality,
        OperationResult, PlatformInfo, PlatformSnapshot, RadioFirmware, SbcQuality, SdCard,
        SleepPhase, SleepProblem, SleepState, StorageState, UpdatePhase, UpdateProblem,
        UpdateState, UsbTransfer, VolumeSpace, VolumeState, WakeReason,
    },
    PlatformTask,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    path::Path,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

const LIMIT: u64 = 1024 * 1024;
const PROGRAM: &str = "/usr/sbin/y2-platform";
pub const STATUS_SCHEMA: &str = "org.y2linux.status/v1";
pub const CAPABILITIES_SCHEMA: &str = "org.y2linux.capabilities/v1";
pub const HEALTH_SCHEMA: &str = "org.y2linux.health/v1";

/// A typed reply for one task.
pub enum Reply {
    Snapshot(Box<PlatformSnapshot>),
    Health(HealthLevel, DiagnosticSection),
    Operation(PlatformTask, Result<OperationResult, String>),
    Failed(PlatformTask, String),
    Sleep(Result<SleepState, String>),
}

pub struct Client {
    pub commands: SyncSender<PlatformTask>,
    pub replies: Receiver<Reply>,
}
impl Client {
    pub fn spawn() -> std::io::Result<Self> {
        let (tx, rx) = sync_channel::<PlatformTask>(1);
        let (reply_tx, replies) = sync_channel(1);
        thread::Builder::new()
            .name("platform-client".into())
            .spawn(move || {
                while let Ok(task) = rx.recv() {
                    if reply_tx.send(execute(task)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(Self {
            commands: tx,
            replies,
        })
    }
}

/// Set from the UI thread to stop the running download (`update stage`): the
/// worker sends it SIGTERM and the platform removes the partial package.
static ABORT_DOWNLOAD: AtomicBool = AtomicBool::new(false);

/// Stop the update download in progress, if any (Cancel on the update screen).
pub fn abort_download() {
    ABORT_DOWNLOAD.store(true, Ordering::SeqCst);
}

fn call(args: &[&str], seconds: u64) -> Result<Value, String> {
    run(PROGRAM, args, Duration::from_secs(seconds), false)
}

fn run(program: &str, args: &[&str], deadline: Duration, abortable: bool) -> Result<Value, String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "System service is starting. Try again in a moment.".to_owned())?;
    let stdout = child.stdout.take().ok_or("No response from the system")?;
    let (tx, rx) = sync_channel(1);
    thread::spawn(move || {
        let mut data = Vec::new();
        let result = stdout.take(LIMIT + 1).read_to_end(&mut data).map(|_| data);
        let _ = tx.send(result);
    });
    let started = Instant::now();
    let mut bytes = None;
    loop {
        if bytes.is_none() {
            if let Ok(output) = rx.try_recv() {
                bytes = Some(output.map_err(|_| "Could not read the system response")?);
            }
        }
        if bytes.as_ref().is_some_and(|b| b.len() > LIMIT as usize) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("The system response was too large".into());
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|_| "System service status unavailable")?
        {
            let raw = match bytes {
                Some(v) => v,
                None => rx
                    .recv_timeout(Duration::from_secs(2))
                    .map_err(|_| "The system did not answer in time")?
                    .map_err(|_| "Could not read the system response")?,
            };
            if raw.len() > LIMIT as usize {
                return Err("The system response was too large".into());
            }
            let value: Value =
                serde_json::from_slice(&raw).map_err(|_| "The system response was unreadable")?;
            if !value.is_object() {
                return Err("The system response has an unsupported shape".into());
            }
            // Health exit 1 is valid evidence, not a transport failure.
            if !status.success() && value["schema"] != HEALTH_SCHEMA {
                return Err(value["failure"]
                    .as_str()
                    .unwrap_or("operation_failed")
                    .to_owned());
            }
            return Ok(value);
        }
        if started.elapsed() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("timeout".into());
        }
        if abortable && ABORT_DOWNLOAD.swap(false, Ordering::SeqCst) {
            if let Ok(pid) = libc::pid_t::try_from(child.id()) {
                // SAFETY: `pid` is our own unreaped child, so it cannot be a
                // recycled process; SIGTERM only asks it to stop, and its
                // handler cleans up before it exits and answers.
                unsafe {
                    libc::kill(pid, libc::SIGTERM);
                }
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn schema(value: Value, expected: &str) -> Result<Value, String> {
    if value["schema"] == expected {
        Ok(value)
    } else {
        Err("incompatible_platform_version".into())
    }
}

fn execute(task: PlatformTask) -> Reply {
    use PlatformTask::*;
    let result = match task {
        Refresh => {
            let status =
                call(&["status", "--reborn", "--pss"], 25).and_then(|v| schema(v, STATUS_SCHEMA));
            let capabilities =
                call(&["capabilities"], 10).and_then(|v| schema(v, CAPABILITIES_SCHEMA));
            return match (status, capabilities) {
                (Ok(status), Ok(capabilities)) => {
                    Reply::Snapshot(Box::new(snapshot(&status, &capabilities)))
                }
                (Err(error), _) | (_, Err(error)) => Reply::Failed(task, friendly_error(&error)),
            };
        }
        Health => {
            return match call(&["health"], 30).and_then(|v| schema(v, HEALTH_SCHEMA)) {
                Ok(value) => {
                    let (level, section) = health(&value);
                    Reply::Health(level, section)
                }
                Err(error) => Reply::Failed(task, friendly_error(&error)),
            }
        }
        UpdateCheck => call(&["update", "check"], 100),
        UpdateStage => {
            // A Cancel left over from an earlier download must not stop this one.
            ABORT_DOWNLOAD.store(false, Ordering::SeqCst);
            run(
                PROGRAM,
                &["update", "stage"],
                Duration::from_secs(950),
                true,
            )
        }
        UpdateApply => call(&["update", "apply"], 30),
        UpdateCancel => call(&["update", "cancel"], 30),
        UpdateRollback => call(&["update", "rollback"], 30),
        Export => call(&["export-state", "--include-database"], 120),
        DiagnosticsExport => call(&["export-diagnostics"], 120),
        LdacQuality(quality) => call(
            &["codec-settings", "--ldac-quality", quality.argument()],
            10,
        ),
        SbcQuality(quality) => call(&["codec-settings", "--sbc-quality", quality.argument()], 10),
        AacQuality(quality) => call(&["codec-settings", "--aac-quality", quality.argument()], 10),
        AacAbr(enabled) => call(
            &[
                "codec-settings",
                "--aac-abr",
                if enabled { "on" } else { "off" },
            ],
            10,
        ),
        LdacAbr(enabled) => call(
            &[
                "codec-settings",
                "--ldac-abr",
                if enabled { "on" } else { "off" },
            ],
            10,
        ),
        SleepRequest => {
            return Reply::Sleep(
                call(&["sleep", "request"], 30)
                    .and_then(|v| schema(v, "org.y2linux.sleep/v1"))
                    .map(|v| sleep(&v))
                    .map_err(|e| friendly_error(&e)),
            );
        }
        StorageBenchmark => call(
            &[
                "bench-storage",
                "--volume",
                "/data",
                "--size-mib",
                "16",
                "--operations",
                "64",
                "--seconds",
                "60",
            ],
            90,
        ),
        LibraryBenchmark => call(
            &["bench-library", "--volume", "/data", "--tracks", "1000"],
            120,
        ),
        // Passive IP/route/DNS readiness only; never pings an external host.
        NetworkCheck => call(&["status", "wifi"], 25).and_then(|v| schema(v, STATUS_SCHEMA)),
    };
    Reply::Operation(
        task,
        result
            .map(|v| operation(&v))
            .map_err(|e| friendly_error(&e)),
    )
}

/// User-language text for a platform failure code. Raw service text is never
/// echoed; Diagnostics keeps the observation itself.
pub fn friendly_error(error: &str) -> String {
    let e = error.to_ascii_lowercase();
    if e.contains("password") || e.contains("wrong_key") || e.contains("wrong_credentials") {
        "Check the password and try again."
    } else if e.contains("dns") {
        "Network connected, but internet name lookup isn't working."
    } else if e.contains("dhcp") || e.contains("no_address") {
        "Connected to Wi-Fi, but couldn't get a network address."
    } else if e.contains("space") || e.contains("reserve") {
        "Not enough free storage. Remove some files using a computer."
    } else if e.contains("signature") || e.contains("hash") || e.contains("manifest") {
        "The update couldn't be verified. Nothing was changed."
    } else if e.contains("kernel") || e.contains("compatib") || e.contains("incompatible") {
        "This needs a system update from a computer."
    } else if e.contains("exactly_one") || e.contains("channel") || e.contains("url") {
        "Software updates are installed using a computer."
    } else if e.contains("clock") || e.contains("tls") || e.contains("network") {
        "Connect to Wi-Fi to check for updates."
    } else if e.contains("timeout") {
        "That took too long. Please try again."
    } else if e.contains("starting") || e.contains("unavailable") || e.contains("not_found") {
        "Not ready yet. Try again in a moment."
    } else {
        "Couldn't complete that. Please try again."
    }
    .into()
}

pub fn update_problem(error: &str) -> UpdateProblem {
    let e = error.to_ascii_lowercase();
    if e.contains("exactly_one") || e.contains("channel") || e.contains("local_package") {
        UpdateProblem::NoUpdateSource
    } else if e.contains("clock") || e.contains("tls") || e.contains("http") || e.contains("url") {
        UpdateProblem::NeedsNetwork
    } else if e.contains("space") || e.contains("reserve") {
        UpdateProblem::NotEnoughSpace
    } else if e.contains("signature") || e.contains("hash") || e.contains("manifest") {
        UpdateProblem::VerificationFailed
    } else if e.contains("charger") || e.contains("battery") {
        UpdateProblem::NeedsCharge
    } else if e.contains("kernel")
        || e.contains("compatib")
        || e.contains("maintenance")
        || e.contains("recovery")
    {
        UpdateProblem::NeedsComputer
    } else {
        UpdateProblem::Other
    }
}

fn text(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(if *b { "Yes" } else { "No" }.into()),
        Value::Array(a) if !a.is_empty() => Some(
            a.iter()
                .take(8)
                .filter_map(text)
                .collect::<Vec<_>>()
                .join(", "),
        ),
        _ => None,
    }
}
/// A diagnostic value. Absence is stated plainly; this is Diagnostics, where
/// an engineer wants to know a field was not reported.
fn shown(v: &Value) -> String {
    text(v).unwrap_or_else(|| "Not reported".into())
}
fn fact(label: &str, v: &Value) -> Fact {
    Fact::new(label, shown(v))
}
fn entries(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}
fn mib_from_kib(v: &Value) -> String {
    v.as_u64()
        .map(|n| format!("{:.1} MiB", n as f64 / 1024.))
        .unwrap_or_else(|| "Not reported".into())
}
fn bytes(v: &Value) -> String {
    v.as_u64()
        .map(reborn_core::platform::human_bytes)
        .unwrap_or_else(|| "Not reported".into())
}

fn volume(status: &Value, path: &str) -> Option<Value> {
    entries(&status["storage"]["volumes"])
        .find(|v| v["path"] == path)
        .cloned()
}

fn space(v: &Value) -> VolumeSpace {
    let state = match (v["state"].as_str(), v["space_state"].as_str()) {
        (Some("Ready"), Some("Normal")) => VolumeState::Ready,
        (Some("Ready"), Some("LowSpace")) => VolumeState::LowSpace,
        (Some("Ready"), Some("CriticalSpace")) => VolumeState::AlmostFull,
        (_, Some("ReadOnlyRisk")) => VolumeState::ReadOnly,
        (Some("Ready"), _) => VolumeState::Ready,
        (Some("Failed"), _) | (_, Some("Failed")) => VolumeState::Error,
        _ => VolumeState::Unknown,
    };
    VolumeSpace {
        state,
        free_bytes: v["available_bytes"].as_u64(),
        total_bytes: v["total_bytes"].as_u64(),
    }
}

pub fn storage(status: &Value) -> StorageState {
    // Internal storage is the Y2Media partition once the platform reports it Ready; before
    // that (an unprovisioned device) it is the Y2DATA volume that holds the music folder.
    let internal = volume(status, "/media/internal")
        .filter(|v| v["state"] == "Ready")
        .or_else(|| volume(status, "/data"))
        .map(|v| space(&v))
        .unwrap_or_default();
    let sd = match volume(status, "/media/sd") {
        None => SdCard::Unknown,
        Some(v) if v["state"] == "Ready" => SdCard::Ready(space(&v)),
        Some(v) => match v["reason"].as_str() {
            // The platform's own vocabulary for "no card inserted".
            Some(r)
                if r.contains("absent") || r.contains("no_media") || r.contains("not_present") =>
            {
                SdCard::Absent
            }
            Some(r) if r.contains("not_uniquely_mounted") || r.contains("not_mounted") => {
                SdCard::Absent
            }
            // Inserted, but refused by its filesystem check (still this card).
            Some("filesystem_check_failed") => SdCard::NeedsRepair,
            _ if v["state"] == "Unavailable" => SdCard::Absent,
            _ => SdCard::Error,
        },
    };
    StorageState { internal, sd }
}

pub fn usb(status: &Value) -> UsbTransfer {
    let cable = entries(&status["system"]["usb"]["udcs"])
        .filter_map(|u| u["state"].as_str())
        .find(|s| !s.is_empty());
    let attached = match cable {
        None => return UsbTransfer::Unknown,
        Some("not attached") => false,
        Some(_) => true,
    };
    if !attached {
        return UsbTransfer::Disconnected;
    }
    let transfer = &status["system"]["usb"]["file_transfer"];
    if transfer["schema"] == "org.y2linux.usb-transfer/v1"
        && transfer["boot_id"] == status["record"]["boot_id"]
        && transfer["boot_id"].as_str().is_some()
    {
        if transfer["error"].as_i64().is_some_and(|code| code > 0) {
            return UsbTransfer::Error;
        }
        return match transfer["state"].as_str() {
            Some("Copying") => UsbTransfer::Copying,
            Some("Ready") if transfer["mode"] == "mtp" => UsbTransfer::Ready,
            Some("Ready") => UsbTransfer::Developer,
            Some("Failed" | "Unavailable") => UsbTransfer::Error,
            _ => UsbTransfer::Starting,
        };
    }
    match status["system"]["ssh"]["state"].as_str() {
        Some("Ready") if cable == Some("configured") => UsbTransfer::Developer,
        Some("Failed") => UsbTransfer::Error,
        _ => UsbTransfer::Starting,
    }
}

/// The USB state from the cable's kernel state and the platform's transfer
/// receipt in RAM, without the status call (≈1.6 s): read twice a second, so
/// the computer screen follows the cable on every screen (Beta 7.1).
pub fn usb_now() -> UsbTransfer {
    usb_from(
        Path::new("/sys/class/udc"),
        Path::new(USB_RECEIPT),
        Path::new(BOOT_ID),
    )
}

const USB_RECEIPT: &str = "/run/y2/usb-transfer.json";
const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";

fn usb_from(udc: &Path, receipt: &Path, boot_id: &Path) -> UsbTransfer {
    let udcs: Vec<Value> = fs::read_dir(udc)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| fs::read_to_string(e.path().join("state")).ok())
        .map(|state| json!({"state": state.trim()}))
        .collect();
    let transfer = fs::read(receipt)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .unwrap_or(Value::Null);
    let boot = fs::read_to_string(boot_id).unwrap_or_default();
    usb(&json!({
        "record": {"boot_id": boot.trim()},
        "system": {"usb": {"udcs": udcs, "file_transfer": transfer}},
    }))
}

const DOWNLOAD_PROGRESS: &str = "/run/y2/update-progress.json";
const CHECK_PROGRESS: &str = "/run/y2/update-check-progress";

/// The platform's live download record and the package check's
/// "<percent> check" line. Reads two small files in RAM; never blocks on the
/// platform worker, which is busy with the download itself.
pub fn download_progress() -> Option<DownloadLive> {
    let record: Value = serde_json::from_slice(&fs::read(DOWNLOAD_PROGRESS).ok()?).ok()?;
    let check = fs::read_to_string(CHECK_PROGRESS).ok();
    parse_download(&record, check.as_deref())
}

pub fn parse_download(record: &Value, check: Option<&str>) -> Option<DownloadLive> {
    if record["schema"] != 1 {
        return None;
    }
    let verifying = match record["phase"].as_str()? {
        "download" => false,
        "verify" => true,
        _ => return None,
    };
    Some(DownloadLive {
        verifying,
        bytes_done: record["bytes_done"].as_u64()?,
        bytes_total: record["bytes_total"].as_u64()?,
        speed_bytes_per_s: record["speed_bytes_per_s"].as_u64().filter(|&s| s > 0),
        seconds_left: record["seconds_left"].as_u64(),
        check_percent: if verifying {
            check
                .and_then(|line| line.split_whitespace().next())
                .and_then(|p| p.parse::<u8>().ok())
                .map(|p| p.min(100))
                .or(Some(0))
        } else {
            None
        },
    })
}

pub fn update(status: &Value, ota_enabled: bool) -> UpdateState {
    let u = &status["system"]["update"];
    let state = u["state"].as_str().unwrap_or("");
    let download = &u["download"];
    let current = status["system"]["versions"]["release_version"].as_str();
    let terminal = matches!(state, "Idle" | "Acknowledged" | "RolledBack" | "Failed");
    // A cancel is the owner's choice, not a failure, and a check made after a
    // failed update (download.json) supersedes it: otherwise the page kept
    // saying "didn't finish" and never offered the newer release.
    let failure = u["failure"].as_str().unwrap_or("");
    let superseded = failure.contains("cancelled")
        || matches!(
            download["state"].as_str(),
            Some("Checking" | "Checked" | "Staging")
        );
    let phase = match state {
        "Queued" => UpdatePhase::ReadyToInstall,
        "PendingHealth" => UpdatePhase::Installing,
        "RollbackPending" => UpdatePhase::Restoring,
        "Failed" if !superseded => UpdatePhase::Failed(update_problem(failure)),
        _ => match download["state"].as_str() {
            Some("Checking") => UpdatePhase::Checking,
            Some("Staging") => UpdatePhase::Downloading,
            Some("Checked") => match download["release_version"].as_str() {
                Some(v) if Some(v) != current => UpdatePhase::Available {
                    version: v.to_owned(),
                },
                Some(_) => UpdatePhase::UpToDate,
                None => UpdatePhase::Unknown,
            },
            Some("Failed") => {
                UpdatePhase::Failed(update_problem(download["failure"].as_str().unwrap_or("")))
            }
            _ => UpdatePhase::Unknown,
        },
    };
    UpdateState {
        can_check: ota_enabled && terminal,
        can_download: ota_enabled && terminal && matches!(phase, UpdatePhase::Available { .. }),
        can_install: ota_enabled && state == "Queued",
        can_cancel: ota_enabled && state == "Queued",
        can_rollback: ota_enabled
            && u["rollback_allowed"] == true
            && matches!(state, "Acknowledged" | "PendingHealth"),
        download_percent: match (
            download["bytes_done"].as_u64(),
            download["bytes_total"].as_u64(),
        ) {
            (Some(done), Some(total)) if total > 0 && download["state"] == "Staging" => {
                Some((done.min(total) * 100 / total) as u8)
            }
            _ => None,
        },
        download_bytes: download["download_bytes"].as_u64(),
        space_needed_mb: download["needed_bytes"]
            .as_u64()
            .map(|b| b.div_ceil(1_000_000)),
        phase,
    }
}

pub fn health(value: &Value) -> (HealthLevel, DiagnosticSection) {
    let level = match value["state"].as_str() {
        Some("OK") => HealthLevel::Ok,
        Some("DEGRADED") => HealthLevel::Degraded,
        Some("FAILED") => HealthLevel::Failed,
        _ => HealthLevel::Unknown,
    };
    let mut facts = vec![fact("Overall", &value["state"])];
    for c in entries(&value["checks"]) {
        facts.push(Fact::new(
            shown(&c["name"]),
            format!("{} · {}", shown(&c["state"]), shown(&c["reason"])),
        ));
    }
    (
        level,
        DiagnosticSection {
            id: "health",
            title: "Health",
            facts,
        },
    )
}

fn enabled_capabilities(caps: &Value) -> Vec<String> {
    caps["capabilities"]
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, c)| c["enabled"] == true)
        .map(|(k, _)| k.clone())
        .collect()
}

/// Project one status/capabilities pair. Pure; covered by host tests.
pub fn snapshot(status: &Value, caps: &Value) -> PlatformSnapshot {
    let enabled = enabled_capabilities(caps);
    let ota = enabled.iter().any(|c| c == "ota");
    PlatformSnapshot {
        observed: true,
        radio_firmware: radio_firmware(status),
        info: PlatformInfo {
            release_version: text(&status["system"]["versions"]["release_version"]),
        },
        storage: storage(status),
        usb: usb(status),
        usb_refresh_generation: usb_refresh_generation(status),
        sleep: sleep(&status["power"]["sleep"]),
        bluetooth_quality: bluetooth_quality(&status["bluetooth"]["codec_settings"]),
        update: update(status, ota),
        health: HealthLevel::Unknown,
        diagnostics: diagnostics(status, caps),
        enabled,
    }
}

pub fn radio_firmware(status: &Value) -> RadioFirmware {
    match status["system"]["radio_firmware"]["state"].as_str() {
        Some("Ready") => RadioFirmware::Ready,
        Some("Missing") => RadioFirmware::Missing,
        Some("Installing") => RadioFirmware::Installing,
        Some("Failed") => RadioFirmware::Failed,
        _ => RadioFirmware::Unknown,
    }
}

fn usb_refresh_generation(status: &Value) -> u64 {
    let transfer = &status["system"]["usb"]["file_transfer"];
    if transfer["schema"] == "org.y2linux.usb-transfer/v1"
        && transfer["boot_id"] == status["record"]["boot_id"]
        && transfer["boot_id"].as_str().is_some()
    {
        transfer["refresh_generation"].as_u64().unwrap_or(0)
    } else {
        0
    }
}

fn ldac_quality(value: &Value) -> Option<LdacQuality> {
    match value.as_str() {
        Some("mobile") => Some(LdacQuality::Mobile),
        Some("standard") => Some(LdacQuality::Standard),
        Some("high") => Some(LdacQuality::High),
        _ => None,
    }
}

fn aac_quality(value: &Value) -> Option<reborn_core::platform::AacQuality> {
    match value.as_str() {
        Some("standard") => Some(reborn_core::platform::AacQuality::Standard),
        Some("high") => Some(reborn_core::platform::AacQuality::High),
        _ => None,
    }
}

fn sbc_quality(value: &Value) -> Option<SbcQuality> {
    match value.as_str() {
        Some("high") => Some(SbcQuality::High),
        Some("xq") => Some(SbcQuality::Xq),
        Some("xq+") => Some(SbcQuality::XqPlus),
        _ => None,
    }
}

pub fn bluetooth_quality(value: &Value) -> BluetoothQuality {
    if value["schema"] != "org.y2linux.codec-settings/v1" {
        return BluetoothQuality::default();
    }
    BluetoothQuality {
        sbc_supported: value["supported"]["sbc_quality"]
            .as_array()
            .is_some_and(|v| {
                ["high", "xq", "xq+"]
                    .iter()
                    .all(|name| v.iter().any(|x| x == name))
            }),
        requested_sbc: sbc_quality(&value["requested"]["sbc_quality"]),
        effective_sbc: sbc_quality(&value["effective"]["sbc_quality"]),
        ldac_supported: value["supported"]["ldac_quality"]
            .as_array()
            .is_some_and(|v| {
                ["mobile", "standard", "high"]
                    .iter()
                    .all(|name| v.iter().any(|x| x == name))
            }),
        abr_supported: value["supported"]["ldac_abr"] == true,
        requested_quality: ldac_quality(&value["requested"]["ldac_quality"]),
        effective_quality: ldac_quality(&value["effective"]["ldac_quality"]),
        requested_abr: value["requested"]["ldac_abr"].as_bool(),
        effective_abr: value["effective"]["ldac_abr"].as_bool(),
        aac_supported: value["supported"]["aac_quality"]
            .as_array()
            .is_some_and(|v| {
                ["standard", "high"]
                    .iter()
                    .all(|name| v.iter().any(|x| x == name))
            }),
        requested_aac: aac_quality(&value["requested"]["aac_quality"]),
        aac_abr_supported: value["supported"]["aac_abr"] == true,
        requested_aac_abr: value["requested"]["aac_abr"].as_bool(),
        pending_restart: value["pending_restart"] == true,
    }
}

/// A successful kernel exit is insufficient: only the service's complete
/// restoration record and same-boot identity may present a restored player.
pub fn sleep(value: &Value) -> SleepState {
    if value["schema"] != "org.y2linux.sleep/v1" {
        return SleepState::default();
    }
    let mut phase = match value["state"].as_str() {
        Some("requested") => SleepPhase::SleepRequested,
        Some("preparing") => SleepPhase::SleepPreparing,
        Some("refused") => SleepPhase::SleepRefused,
        Some("sleeping") => SleepPhase::Sleeping,
        Some("restoring" | "waking") => SleepPhase::Waking,
        Some("restored") => SleepPhase::Restored,
        Some("restore_failed") => SleepPhase::RestoreFailed,
        _ => SleepPhase::Awake,
    };
    let same_boot = value["same_boot"].as_bool();
    let problem = if phase == SleepPhase::Restored
        && (same_boot != Some(true) || value["kernel_completed"] != true)
    {
        phase = SleepPhase::RestoreFailed;
        Some(SleepProblem::DifferentBoot)
    } else {
        value["reason"].as_str().filter(|s| !s.is_empty()).map(|s| {
            if s.contains("qualification") {
                SleepProblem::QualificationPending
            } else if s.contains("charger") {
                SleepProblem::ChargerActive
            } else if s.contains("playback") || s.contains("audio_active") {
                SleepProblem::PlaybackActive
            } else if s.contains("busy")
                || s.contains("maintenance")
                || s.contains("in_progress")
                || s.contains("workload")
                || s.contains("firmware_update")
                || s.contains("activity_operation")
            {
                SleepProblem::Busy
            } else if s.contains("boot") {
                SleepProblem::DifferentBoot
            } else if s.contains("restore") {
                SleepProblem::RestorationFailed
            } else {
                SleepProblem::Other
            }
        })
    };
    SleepState {
        phase,
        problem,
        same_boot,
        wake: match value["wake_reason"].as_str() {
            Some("power") => WakeReason::Power,
            Some("rtc") => WakeReason::Rtc,
            _ => WakeReason::Unknown,
        },
    }
}

fn diagnostics(s: &Value, caps: &Value) -> Vec<DiagnosticSection> {
    let mut out = vec![];
    // Battery and power source
    let p = &s["power"];
    let mut battery = vec![
        Fact::new(
            "State of charge",
            p["soc_percent"]
                .as_u64()
                .filter(|v| *v <= 100)
                .map(|v| format!("{v}%"))
                .unwrap_or_else(|| "Not reported".into()),
        ),
        fact("SOC source", &p["soc_source"]),
        fact("SOC confidence", &p["soc_confidence"]),
        fact("Calibration", &p["battery"]["calibration_source"]),
        fact("Low-battery policy", &p["low_battery"]["state"]),
        Fact::new(
            "Battery voltage",
            p["low_battery"]["voltage_uv"]
                .as_f64()
                .map(|v| format!("{:.3} V", v / 1e6))
                .unwrap_or_else(|| "Not reported".into()),
        ),
        fact("Measured current (µA)", &p["measured_current_ua"]),
        fact("Pack temperature (m°C)", &p["pack_temperature"]),
    ];
    for supply in entries(&p["supplies"]) {
        battery.push(Fact::new(
            format!("Supply {}", shown(&supply["name"])),
            format!(
                "{} · {}",
                shown(&supply["type"]),
                text(&supply["status"])
                    .or_else(|| text(&supply["online"]).map(|o| format!("online {o}")))
                    .unwrap_or_else(|| "Not reported".into())
            ),
        ));
    }
    out.push(DiagnosticSection {
        id: "battery",
        title: "Battery",
        facts: battery,
    });
    // Storage
    let mut storage = vec![];
    for (path, label) in [
        ("/media/internal", "Internal"),
        ("/data", "System data"),
        ("/media/sd", "SD card"),
        ("/", "System"),
    ] {
        if let Some(v) = volume(s, path) {
            storage.push(Fact::new(
                label,
                format!("{} · {}", shown(&v["state"]), shown(&v["space_state"])),
            ));
            storage.push(Fact::new(
                format!("{label} free / total"),
                format!(
                    "{} / {}",
                    bytes(&v["available_bytes"]),
                    bytes(&v["total_bytes"])
                ),
            ));
            storage.push(fact(&format!("{label} filesystem"), &v["filesystem"]));
            for key in ["uuid", "device", "mount_generation", "reason"] {
                if !v[key].is_null() {
                    storage.push(fact(&format!("{label} {}", key.replace('_', " ")), &v[key]));
                }
            }
        }
    }
    for c in entries(&s["storage"]["controllers"]) {
        let name = match c["name"].as_str() {
            Some("11230000.mmc") => "eMMC".to_owned(),
            Some("11240000.mmc") => "SD".to_owned(),
            _ => shown(&c["name"]),
        };
        let m = &c["mode"];
        if m.is_object() {
            let mhz = |v: &Value| {
                v.as_u64()
                    .map(|hz| format!("{} MHz", hz / 1_000_000))
                    .unwrap_or_else(|| "?".into())
            };
            storage.push(Fact::new(
                format!("{name} mode"),
                format!(
                    "{} · {}-bit · {} · {} mV · ceiling {}",
                    shown(&m["level"]),
                    shown(&m["width"]),
                    mhz(&m["actual_hz"]),
                    shown(&m["signal_mv"]),
                    shown(&m["ceiling"])
                ),
            ));
            storage.push(Fact::new(
                format!("{name} card"),
                format!(
                    "supports {} · caps {}",
                    shown(&m["card_level"]),
                    m["card_caps"]
                        .as_u64()
                        .map(|v| format!("{v:#x}"))
                        .unwrap_or_else(|| "?".into())
                ),
            ));
            storage.push(Fact::new(
                format!("{name} tuning"),
                format!(
                    "{} · runs {} · readback {}",
                    shown(&m["tuning"]),
                    shown(&m["tuning_runs"]),
                    shown(&m["verify"])
                ),
            ));
            storage.push(Fact::new(
                format!("{name} faults"),
                format!(
                    "CRC {} · timeout {} · controller {} · tuning {} · voltage {} · readback {} · fallbacks {}",
                    shown(&m["crc_events"]),
                    shown(&m["timeout_events"]),
                    shown(&m["controller_events"]),
                    shown(&m["tuning_events"]),
                    shown(&m["voltage_events"]),
                    shown(&m["verify_events"]),
                    shown(&m["fallbacks"])
                ),
            ));
        } else {
            storage.push(Fact::new(
                name,
                format!(
                    "cap {} Hz · actual {} Hz · errors {} · fallbacks {}",
                    shown(&c["cap_hz"]),
                    shown(&c["actual_hz"]),
                    shown(&c["transport_errors"]),
                    shown(&c["fallbacks"])
                ),
            ));
        }
    }
    out.push(DiagnosticSection {
        id: "storage",
        title: "Storage",
        facts: storage,
    });
    // Network
    let w = &s["wifi"];
    out.push(DiagnosticSection {
        id: "network",
        title: "Network",
        facts: vec![
            fact("Readiness", &w["state"]),
            fact("Reason", &w["reason"]),
            fact("IP address", &w["ip_addresses"]),
            Fact::new(
                "Default route",
                entries(&w["default_route"])
                    .take(4)
                    .map(|r| format!("via {} · {}", shown(&r["gateway"]), shown(&r["dev"])))
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            fact("DNS ready", &w["dns_ready"]),
            fact("Signal (dBm)", &w["rssi_dbm"]),
            fact("RX bytes", &w["traffic_counters"]["rx_bytes"]),
            fact("TX bytes", &w["traffic_counters"]["tx_bytes"]),
            fact("Power save", &w["power_save"]["enabled"]),
        ],
    });
    // Bluetooth service state (transport facts are added by the application)
    out.push(DiagnosticSection {
        id: "bluetooth",
        title: "Bluetooth",
        facts: vec![
            fact("Trusted peer", &s["bluetooth"]["selected_peer"]["trusted"]),
            fact("Reconnect state", &s["bluetooth"]["reconnect"]["state"]),
            fact(
                "Saved SBC quality",
                &s["bluetooth"]["codec_settings"]["requested"]["sbc_quality"],
            ),
            fact(
                "Effective SBC quality",
                &s["bluetooth"]["codec_settings"]["effective"]["sbc_quality"],
            ),
            fact(
                "Saved LDAC quality",
                &s["bluetooth"]["codec_settings"]["requested"]["ldac_quality"],
            ),
            fact(
                "Effective LDAC quality",
                &s["bluetooth"]["codec_settings"]["effective"]["ldac_quality"],
            ),
            fact(
                "Saved LDAC ABR",
                &s["bluetooth"]["codec_settings"]["requested"]["ldac_abr"],
            ),
            fact(
                "Effective LDAC ABR",
                &s["bluetooth"]["codec_settings"]["effective"]["ldac_abr"],
            ),
            fact(
                "Quality restart pending",
                &s["bluetooth"]["codec_settings"]["pending_restart"],
            ),
            fact(
                "Optional codecs built",
                &caps["capabilities"]["bluetooth"]["optional_codecs"],
            ),
            fact(
                "Auto-eligible codecs",
                &caps["capabilities"]["bluetooth"]["codec_auto"]["eligible_codecs"],
            ),
        ],
    });
    // Audio hardware parameters (pipeline facts are added by the application)
    let mut audio = vec![
        fact(
            "Enabled formats",
            &caps["capabilities"]["audio"]["enabled_formats"],
        ),
        fact(
            "Enabled rates (Hz)",
            &caps["capabilities"]["audio"]["enabled_rates_hz"],
        ),
    ];
    for (path, v) in s["audio"]["hw_params"].as_object().into_iter().flatten() {
        audio.push(Fact::new(path.clone(), shown(v)));
    }
    out.push(DiagnosticSection {
        id: "audio",
        title: "Audio",
        facts: audio,
    });
    // CPU, memory and thermal
    let mut cpu = vec![
        fact("Sleep state", &s["power"]["sleep"]["state"]),
        fact("Sleep reason", &s["power"]["sleep"]["reason"]),
        fact("Wake reason", &s["power"]["sleep"]["wake_reason"]),
        fact("Sleep same boot", &s["power"]["sleep"]["same_boot"]),
        fact("Cores online", &s["cpu"]["online"]),
        fact("Load average", &s["cpu"]["load_average"]),
        Fact::new(
            "Memory available",
            mib_from_kib(&s["memory"]["meminfo"]["MemAvailable"]),
        ),
        Fact::new(
            "Memory total",
            mib_from_kib(&s["memory"]["meminfo"]["MemTotal"]),
        ),
        fact("Clocksource", &s["cpu"]["timer"]["clocksource"]),
        fact("High resolution", &s["cpu"]["timer"]["highres_active"]),
        fact("Tickless idle", &s["cpu"]["timer"]["no_hz_active"]),
    ];
    for c in entries(&s["cpu"]["policies"]) {
        cpu.push(Fact::new(
            format!("CPU {}", shown(&c["affected_cpus"])),
            format!(
                "{} MHz · {}",
                c["scaling_cur_freq"]
                    .as_str()
                    .and_then(|v| v.parse::<u64>().ok())
                    .map(|k| (k / 1000).to_string())
                    .unwrap_or_else(|| "?".into()),
                shown(&c["scaling_governor"])
            ),
        ));
    }
    for idle in entries(&s["cpu"]["idle"]) {
        cpu.push(Fact::new(
            format!("{} {}", shown(&idle["cpu"]), shown(&idle["name"])),
            format!(
                "{} entries · {} µs",
                shown(&idle["usage"]),
                shown(&idle["time_us"])
            ),
        ));
    }
    for zone in entries(&s["thermal"]["zones"]) {
        cpu.push(Fact::new(
            format!("{} (die)", shown(&zone["type"])),
            zone["temperature_millicelsius"]
                .as_f64()
                .map(|n| format!("{:.1} °C", n / 1000.))
                .unwrap_or_else(|| "Not reported".into()),
        ));
    }
    for process in entries(&s["memory"]["processes"]) {
        cpu.push(Fact::new("Reborn PSS", mib_from_kib(&process["pss_kib"])));
    }
    out.push(DiagnosticSection {
        id: "cpu",
        title: "CPU & Power",
        facts: cpu,
    });
    // USB
    let dma = &s["system"]["usb"]["dma"];
    let transfer = &s["system"]["usb"]["file_transfer"];
    let mut usb = vec![
        fact("File transfer", &transfer["state"]),
        fact("USB mode", &transfer["mode"]),
        fact("Transfer error", &transfer["error"]),
        fact("Advanced transfer", &s["system"]["ssh"]["state"]),
        fact("Address", &s["system"]["ssh"]["bind"]),
        fact("SFTP", &s["system"]["ssh"]["sftp"]),
        fact("Transfer path", &dma["transfer"]),
        fact("DMA errors", &dma["dma_errors"]),
        fact("DMA RX bytes", &dma["dma_rx_programmed_bytes"]),
        fact("DMA TX bytes", &dma["dma_tx_programmed_bytes"]),
        fact("PIO RX bytes", &dma["pio_rx_bytes"]),
        fact("PIO TX bytes", &dma["pio_tx_bytes"]),
    ];
    for udc in entries(&s["system"]["usb"]["udcs"]) {
        usb.insert(0, fact("Cable", &udc["state"]));
        usb.insert(1, fact("USB speed", &udc["speed"]));
    }
    out.push(DiagnosticSection {
        id: "usb",
        title: "USB",
        facts: usb,
    });
    // Update
    let u = &s["system"]["update"];
    out.push(DiagnosticSection {
        id: "update",
        title: "Update",
        facts: vec![
            fact("Journal state", &u["state"]),
            fact("Download", &u["download"]["state"]),
            fact("Offered release", &u["download"]["release_version"]),
            fact("Failure", &u["failure"]),
            fact("Download failure", &u["download"]["failure"]),
            fact("Signing key ID", &u["key_id"]),
            fact("Sequence", &u["sequence"]),
            fact("Rollback allowed", &u["rollback_allowed"]),
            Fact::new("Scope", "System root only; boot image stays manual"),
        ],
    });
    // Boot and services
    let mut boot = vec![
        fact("Boot ID", &s["record"]["boot_id"]),
        fact(
            "Previous boot",
            &s["system"]["boot_history"]["previous_boot_id"],
        ),
        fact(
            "Clean shutdown",
            &s["system"]["previous_boot_evidence"]["previous_orderly_shutdown"],
        ),
        fact("Last stage", &s["system"]["boot_history"]["last_stage"]),
        fact("Reset cause", &s["system"]["reset_cause"]),
        fact("Kernel taint", &s["system"]["kernel_taint"]),
        fact("Clock trusted", &s["system"]["time"]["tls_ready"]),
        fact("Clock source", &s["system"]["time"]["source"]),
    ];
    for (name, v) in s["readiness"].as_object().into_iter().flatten() {
        boot.push(Fact::new(
            name.clone(),
            format!("{} · {}", shown(&v["state"]), shown(&v["reason"])),
        ));
    }
    out.push(DiagnosticSection {
        id: "boot",
        title: "Boot & Services",
        facts: boot,
    });
    // Build information
    let v = &s["system"]["versions"];
    out.push(DiagnosticSection {
        id: "build",
        title: "Build Information",
        facts: vec![
            Fact::new("Reborn", reborn_core::BUILD_LABEL),
            fact("Y2Linux release", &v["release_version"]),
            fact("Build ID", &v["build_id"]),
            fact("Kernel", &s["record"]["kernel"]),
            fact("Root filesystem", &v["rootfs_version"]),
            fact("Reborn source", &v["reborn_source_commit"]),
            fact("Y2Linux source", &v["build_git_commit"]),
        ],
    });
    // Capabilities
    let mut capabilities = vec![];
    for (key, c) in caps["capabilities"].as_object().into_iter().flatten() {
        capabilities.push(Fact::new(
            key.replace('_', " "),
            format!(
                "{} · {}{}",
                if c["implemented"] != true {
                    "not implemented"
                } else if c["enabled"] == true {
                    "enabled"
                } else {
                    "disabled"
                },
                if c["qualified"] == true {
                    "qualified"
                } else {
                    "qualification pending"
                },
                if c["experimental"] == true {
                    " · experimental"
                } else {
                    ""
                }
            ),
        ));
    }
    out.push(DiagnosticSection {
        id: "capabilities",
        title: "Capabilities",
        facts: capabilities,
    });
    out
}

/// Reduce an explicit operation's record to public facts.
fn operation(r: &Value) -> OperationResult {
    let mut facts = vec![];
    for (label, key) in [
        ("State", "state"),
        ("Release", "release_version"),
        ("Archive", "path"),
        ("SHA-256", "sha256"),
        ("Redacted", "redacted"),
    ] {
        if let Some(v) = text(&r[key]) {
            facts.push(Fact::new(label, v));
        }
    }
    if r["redacted"] == true {
        facts.push(Fact::new(
            "Retrieve report",
            "Connect to your computer and copy this archive from the exports folder.",
        ));
    }
    if let Some(v) = text(&r["record"]["result"]) {
        facts.push(Fact::new("Result", v));
    }
    for m in entries(&r["measurements"]) {
        facts.push(Fact::new(
            shown(&m["operation"]),
            format!(
                "{} · {} MB/s · p95 {} ms",
                shown(&m["result"]),
                shown(&m["MB_per_second"]),
                shown(&m["p95_ms"])
            ),
        ));
    }
    if !r["wifi"].is_null() {
        facts.push(fact("Wi-Fi readiness", &r["wifi"]["state"]));
        facts.push(fact("IP address", &r["wifi"]["ip_addresses"]));
        facts.push(fact("DNS ready", &r["wifi"]["dns_ready"]));
    }
    OperationResult {
        succeeded: r["record"]["result"] != "FAILED" && r["state"] != "Failed",
        facts,
    }
}

/// A projection with no observation yet; used before the first refresh.
pub fn unobserved() -> PlatformSnapshot {
    let mut s = snapshot(&json!({}), &json!({}));
    s.observed = false;
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status() -> Value {
        json!({"schema":STATUS_SCHEMA,
            "storage":{"volumes":[
                {"path":"/data","state":"Ready","space_state":"LowSpace","available_bytes":1_000_000_000_u64,"total_bytes":6_000_000_000_u64,"uuid":"abcd"},
                {"path":"/media/sd","state":"Unavailable","reason":"not_uniquely_mounted"}]},
            "system":{"versions":{"release_version":"1.0"},
                "usb":{"udcs":[{"state":"configured"}]},"ssh":{"state":"Ready","bind":"10.42.0.1:22"},
                "update":{"state":"Idle","download":{"state":"Checked","release_version":"1.1"}}}})
    }
    fn caps() -> Value {
        json!({"schema":CAPABILITIES_SCHEMA,"capabilities":{"ota":{"implemented":true,"enabled":true},"usb_host":{"implemented":false}}})
    }

    #[test]
    fn typed_projection_presents_states_not_platform_fields() {
        let s = snapshot(&status(), &caps());
        assert!(s.observed);
        assert_eq!(s.storage.internal.state, VolumeState::LowSpace);
        assert_eq!(s.storage.internal.free_bytes, Some(1_000_000_000));
        assert_eq!(s.storage.sd, SdCard::Absent);
        // A card the platform refused after a failed check (issue #44).
        let mut refused = status();
        for v in refused["storage"]["volumes"].as_array_mut().unwrap() {
            if v["path"] == "/media/sd" {
                *v = json!({"path":"/media/sd","state":"Degraded",
                    "reason":"filesystem_check_failed","filesystem":"vfat"});
            }
        }
        assert_eq!(snapshot(&refused, &caps()).storage.sd, SdCard::NeedsRepair);
        assert_eq!(s.usb, UsbTransfer::Developer);
        assert_eq!(
            s.update.phase,
            UpdatePhase::Available {
                version: "1.1".into()
            }
        );
        assert!(s.update.can_download && s.update.can_check && !s.update.can_install);
        assert!(s.enabled("ota") && !s.enabled("usb_host"));
        // Engineering details live only in Diagnostics.
        let usb = s.section("usb").unwrap();
        assert!(usb.facts.iter().any(|f| f.value.contains("10.42.0.1")));
    }

    #[test]
    fn internal_storage_is_y2media_when_ready_and_y2data_before_provisioning() {
        let data = json!({"path":"/data","state":"Ready","space_state":"Normal","available_bytes":200_u64,"total_bytes":800_u64});
        let media = |state: &str| json!({"path":"/media/internal","state":state,"space_state":"Normal","available_bytes":5_000_u64,"total_bytes":5_400_u64});
        let status = |volumes: Vec<Value>| json!({"storage":{"volumes":volumes}});
        let ready = storage(&status(vec![data.clone(), media("Ready")]));
        assert_eq!(ready.internal.free_bytes, Some(5_000));
        assert_eq!(ready.internal.total_bytes, Some(5_400));
        // Not provisioned, failed or absent: the Y2DATA music folder still backs internal storage.
        for volumes in [
            vec![data.clone()],
            vec![data.clone(), media("Failed")],
            vec![data.clone(), media("Unavailable")],
        ] {
            let fallback = storage(&status(volumes));
            assert_eq!(fallback.internal.free_bytes, Some(200));
            assert_eq!(fallback.internal.total_bytes, Some(800));
        }
    }

    #[test]
    fn optional_absent_and_unknown_fields_stay_unknown_not_invented() {
        let s = snapshot(&json!({"schema":STATUS_SCHEMA}), &json!({}));
        assert_eq!(s.storage.internal.state, VolumeState::Unknown);
        assert_eq!(s.storage.sd, SdCard::Unknown);
        assert_eq!(s.usb, UsbTransfer::Unknown);
        assert_eq!(s.update.phase, UpdatePhase::Unknown);
        assert!(!s.update.can_check, "no capability, no action");
        assert!(!unobserved().observed);
    }

    #[test]
    fn update_states_and_failures_become_user_states() {
        let mut v = status();
        v["system"]["update"] = json!({"state":"Queued"});
        let u = update(&v, true);
        assert_eq!(u.phase, UpdatePhase::ReadyToInstall);
        assert!(u.can_install && u.can_cancel && !u.can_check);
        v["system"]["update"] =
            json!({"state":"Idle","download":{"state":"Checked","release_version":"1.0"}});
        assert_eq!(update(&v, true).phase, UpdatePhase::UpToDate);
        v["system"]["update"] = json!({"state":"Idle","download":{"state":"Failed","failure":"clock_or_entropy_not_ready_for_TLS"}});
        assert_eq!(
            update(&v, true).phase,
            UpdatePhase::Failed(UpdateProblem::NeedsNetwork)
        );
        assert_eq!(
            update_problem("exactly_one_local_package_or_HTTPS_manifest_required"),
            UpdateProblem::NoUpdateSource
        );
        assert!(!update(&v, false).can_check);
    }

    #[test]
    fn full_system_update_progress_restore_and_power_gate() {
        let mut v = status();
        v["system"]["update"] = json!({"state":"Idle","download":{"state":"Checking"}});
        assert_eq!(update(&v, true).phase, UpdatePhase::Checking);
        v["system"]["update"] = json!({"state":"Idle","download":{"state":"Checked",
            "release_version":"1.2","download_bytes":57_000_000_u64}});
        let u = update(&v, true);
        assert_eq!(
            u.phase,
            UpdatePhase::Available {
                version: "1.2".into()
            }
        );
        assert_eq!(u.download_bytes, Some(57_000_000));
        v["system"]["update"] = json!({"state":"Idle","download":{"state":"Staging",
            "bytes_done":25,"bytes_total":100}});
        let u = update(&v, true);
        assert_eq!(
            (u.phase, u.download_percent),
            (UpdatePhase::Downloading, Some(25))
        );
        v["system"]["update"] = json!({"state":"RollbackPending"});
        assert_eq!(update(&v, true).phase, UpdatePhase::Restoring);
        v["system"]["update"] = json!({"state":"PendingHealth"});
        assert_eq!(update(&v, true).phase, UpdatePhase::Installing);
        assert_eq!(
            update_problem("update_needs_charger_or_half_battery"),
            UpdateProblem::NeedsCharge
        );
        for (state, want) in [
            ("Ready", RadioFirmware::Ready),
            ("Missing", RadioFirmware::Missing),
            ("Failed", RadioFirmware::Failed),
            ("odd", RadioFirmware::Unknown),
        ] {
            let s = json!({"system":{"radio_firmware":{"state":state}}});
            assert_eq!(radio_firmware(&s), want);
        }
        assert_eq!(radio_firmware(&json!({})), RadioFirmware::Unknown);
        // A cancelled queue followed by a check: the newer release is offered.
        v["system"]["update"] = json!({"state":"Failed","failure":"owner cancelled before installation",
            "download":{"state":"Checked","release_version":"1.0.0-platform-v1-power-final-candidate.49"}});
        let u = update(&v, true);
        assert!(matches!(u.phase, UpdatePhase::Available { .. }) && u.can_download);
        v["system"]["update"] =
            json!({"state":"Failed","failure":"owner cancelled before installation"});
        assert_eq!(update(&v, true).phase, UpdatePhase::Unknown);
        // A real failure still shows until a later check.
        v["system"]["update"] = json!({"state":"Failed","failure":"verification failed: hash"});
        assert_eq!(
            update(&v, true).phase,
            UpdatePhase::Failed(UpdateProblem::VerificationFailed)
        );
        let live = parse_download(
            &json!({"schema":1,"phase":"download","bytes_done":25,"bytes_total":100,
                "speed_bytes_per_s":3_300_000,"seconds_left":12}),
            None,
        )
        .unwrap();
        assert_eq!(
            (live.verifying, live.speed_bytes_per_s, live.seconds_left),
            (false, Some(3_300_000), Some(12))
        );
        assert!((live.fraction() - 0.25).abs() < 1e-6);
        let checking = parse_download(
            &json!({"schema":1,"phase":"verify","bytes_done":100,"bytes_total":100,
                "speed_bytes_per_s":null,"seconds_left":null}),
            Some("42 check\n"),
        )
        .unwrap();
        assert_eq!(
            (checking.verifying, checking.check_percent),
            (true, Some(42))
        );
        // Finished, failed or foreign records are not a running download.
        for phase in ["done", "failed", "other"] {
            assert!(parse_download(
                &json!({"schema":1,"phase":phase,"bytes_done":1,"bytes_total":1}),
                None
            )
            .is_none());
        }
        v["system"]["update"] = json!({"state":"Idle","download":{"state":"Failed",
            "failure":"update_data_space_reserve","needed_bytes":280_100_000u64}});
        let u = update(&v, true);
        assert_eq!(
            (u.phase, u.space_needed_mb),
            (
                UpdatePhase::Failed(UpdateProblem::NotEnoughSpace),
                Some(281)
            )
        );
        assert_eq!(
            update_problem("RECOVERY holds no Y2 recovery image"),
            UpdateProblem::NeedsComputer
        );
    }

    #[test]
    fn mtp_state_and_refresh_require_current_boot_receipt() {
        let mut v = status();
        v["record"]["boot_id"] = json!("this-boot");
        v["system"]["usb"]["file_transfer"] = json!({"schema":"org.y2linux.usb-transfer/v1","boot_id":"this-boot","mode":"mtp","state":"Ready","refresh_generation":9});
        assert_eq!(usb(&v), UsbTransfer::Ready);
        assert_eq!(usb_refresh_generation(&v), 9);
        v["system"]["usb"]["file_transfer"]["state"] = json!("Copying");
        assert_eq!(usb(&v), UsbTransfer::Copying);
        v["system"]["usb"]["file_transfer"]["error"] = json!(28);
        assert_eq!(usb(&v), UsbTransfer::Error);
        v["system"]["usb"]["file_transfer"]["boot_id"] = json!("old-boot");
        assert_eq!(usb(&v), UsbTransfer::Developer);
        assert_eq!(usb_refresh_generation(&v), 0);
    }
    #[test]
    fn usb_cable_and_service_states() {
        let mut v = status();
        v["system"]["usb"]["udcs"] = json!([{"state":"not attached"}]);
        assert_eq!(usb(&v), UsbTransfer::Disconnected);
        v["system"]["usb"]["udcs"] = json!([{"state":"configured"}]);
        v["system"]["ssh"]["state"] = json!("Starting");
        assert_eq!(usb(&v), UsbTransfer::Starting);
        v["system"]["ssh"]["state"] = json!("Failed");
        assert_eq!(usb(&v), UsbTransfer::Error);
    }

    #[test]
    fn malformed_future_and_oversize_records_fail_closed() {
        assert!(schema(json!({"schema":"v2"}), STATUS_SCHEMA).is_err());
        assert!(run(
            "/bin/sh",
            &["-c", "printf '[]'"],
            Duration::from_secs(1),
            false
        )
        .is_err());
        assert!(run(
            "/bin/sh",
            &["-c", "head -c 1048577 /dev/zero"],
            Duration::from_secs(1),
            false
        )
        .is_err());
    }

    #[test]
    fn failed_health_is_observation_and_commands_have_deadlines() {
        let value = run(
            "/bin/sh",
            &[
                "-c",
                "printf '{\"schema\":\"org.y2linux.health/v1\",\"state\":\"FAILED\"}'; exit 1",
            ],
            Duration::from_secs(1),
            false,
        )
        .unwrap();
        assert_eq!(health(&value).0, HealthLevel::Failed);
        assert!(run("/bin/sleep", &["2"], Duration::from_millis(40), false).is_err());
    }

    #[test]
    fn usb_now_follows_the_cable_and_the_receipt_without_a_status_call() {
        let root = std::env::temp_dir().join(format!("reborn-usb-now-{}", std::process::id()));
        let udc = root.join("udc/musb-hdrc.4.auto");
        fs::create_dir_all(&udc).unwrap();
        let (receipt, boot) = (root.join("usb-transfer.json"), root.join("boot_id"));
        fs::write(&boot, "this-boot\n").unwrap();
        let state = |s: &str| fs::write(udc.join("state"), s).unwrap();
        let ready = |b: &str| {
            fs::write(
                &receipt,
                json!({"schema":"org.y2linux.usb-transfer/v1","boot_id":b,"mode":"mtp","state":"Ready"})
                    .to_string(),
            )
            .unwrap()
        };
        let now = || usb_from(&root.join("udc"), &receipt, &boot);
        state("configured\n");
        ready("this-boot");
        assert_eq!(now(), UsbTransfer::Ready);
        state("not attached\n");
        assert_eq!(now(), UsbTransfer::Disconnected);
        // A receipt of an earlier boot is not this connection.
        state("configured\n");
        ready("earlier-boot");
        assert_eq!(now(), UsbTransfer::Starting);
        fs::write(
            &receipt,
            json!({"schema":"org.y2linux.usb-transfer/v1","boot_id":"this-boot","mode":"mtp","state":"Copying"})
                .to_string(),
        )
        .unwrap();
        assert_eq!(now(), UsbTransfer::Copying);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn cancel_stops_an_abortable_download_and_reports_its_answer() {
        // Stands in for `update stage`: SIGTERM makes it answer and exit.
        let script = "trap 'printf \"{\\\"state\\\":\\\"Failed\\\",\\\"failure\\\":\\\"download_cancelled\\\"}\"; exit 1' TERM; \
                      sleep 5 >/dev/null & wait";
        let started = Instant::now();
        let abort = thread::spawn(|| {
            thread::sleep(Duration::from_millis(150));
            abort_download();
        });
        let result = run("/bin/sh", &["-c", script], Duration::from_secs(10), true);
        abort.join().unwrap();
        assert_eq!(result.unwrap_err(), "download_cancelled");
        assert!(started.elapsed() < Duration::from_secs(4));
        assert!(!ABORT_DOWNLOAD.load(Ordering::SeqCst));
    }

    #[test]
    fn errors_are_user_language_and_never_echo_service_text() {
        assert!(friendly_error("wrong_key").contains("password"));
        assert!(friendly_error("dhcp_timeout").contains("network address"));
        assert!(friendly_error("dns_unavailable").contains("name lookup"));
        let raw = friendly_error("org.bluez.Error.Failed private details");
        assert!(!raw.contains("org.bluez") && !raw.contains("private"));
    }

    #[test]
    fn diagnostics_name_the_negotiated_storage_mode() {
        let v = json!({"storage":{"controllers":[
            {"name":"11230000.mmc","cap_hz":200000000,"actual_hz":200000000,
             "transport_errors":0,"fallbacks":0,
             "mode":{"level":"HS200","ceiling":"HS200","width":8,"actual_hz":200000000,
                     "signal_mv":1800,"card_level":"HS200","card_caps":87,"tuning":"pass",
                     "tuning_runs":1,"verify":"pass","crc_events":0,"timeout_events":2,
                     "controller_events":0,"tuning_events":0,"voltage_events":0,
                     "verify_events":0,"fallbacks":0}},
            {"name":"11240000.mmc","cap_hz":50000000,"actual_hz":50000000,
             "transport_errors":0,"fallbacks":0,"mode":null}]}});
        let sections = diagnostics(&v, &json!({}));
        let facts = &sections.iter().find(|s| s.id == "storage").unwrap().facts;
        let get = |label: &str| {
            facts
                .iter()
                .find(|f| f.label == label)
                .map(|f| f.value.clone())
                .unwrap()
        };
        assert_eq!(
            get("eMMC mode"),
            "HS200 · 8-bit · 200 MHz · 1800 mV · ceiling HS200"
        );
        assert_eq!(get("eMMC card"), "supports HS200 · caps 0x57");
        assert_eq!(get("eMMC tuning"), "pass · runs 1 · readback pass");
        assert!(get("eMMC faults").contains("timeout 2"));
        // An older kernel without the mode record keeps the clock summary.
        assert!(get("SD").starts_with("cap 50000000 Hz"));
    }
}

#[cfg(test)]
mod feature_completion_tests {
    use super::*;

    #[test]
    fn sleep_preparation_and_waking_are_typed_and_legacy_restoring_is_supported() {
        for (state, expected) in [
            ("requested", SleepPhase::SleepRequested),
            ("preparing", SleepPhase::SleepPreparing),
            ("waking", SleepPhase::Waking),
            ("restoring", SleepPhase::Waking),
        ] {
            assert_eq!(
                sleep(&json!({"schema":"org.y2linux.sleep/v1","state":state})).phase,
                expected
            );
        }
    }

    #[test]
    fn sleep_requires_complete_same_boot_restoration_and_names_refusal() {
        let mut v = json!({"schema":"org.y2linux.sleep/v1", "state":"refused", "reason":"physical_qualification_required"});
        assert_eq!(sleep(&v).phase, SleepPhase::SleepRefused);
        assert_eq!(sleep(&v).problem, Some(SleepProblem::QualificationPending));
        v["state"] = json!("restored");
        assert_eq!(sleep(&v).phase, SleepPhase::RestoreFailed);
        v["same_boot"] = json!(true);
        v["kernel_completed"] = json!(true);
        v["reason"] = Value::Null;
        v["wake_reason"] = json!("rtc");
        assert_eq!(
            sleep(&v),
            SleepState {
                phase: SleepPhase::Restored,
                same_boot: Some(true),
                wake: WakeReason::Rtc,
                problem: None
            }
        );
        v["same_boot"] = json!(false);
        assert_eq!(sleep(&v).phase, SleepPhase::RestoreFailed);
        v["schema"] = json!("org.y2linux.sleep/v2");
        assert_eq!(sleep(&v), SleepState::default());
    }

    #[test]
    fn quality_settings_keep_saved_and_effective_values_separate() {
        let v = json!({"schema":"org.y2linux.codec-settings/v1",
            "requested":{"ldac_quality":"high","ldac_abr":true,"sbc_quality":"xq"},
            "effective":{"ldac_quality":"standard","ldac_abr":false,"sbc_quality":"high"},
            "supported":{"ldac_quality":["mobile","standard","high"],"ldac_abr":true,"sbc_quality":["high","xq","xq+"]},
            "pending_restart":true});
        let q = bluetooth_quality(&v);
        assert!(q.pending_restart && q.ldac_supported && q.abr_supported && q.sbc_supported);
        assert!(!q.aac_supported && q.requested_aac.is_none());
        let aac = json!({"schema":"org.y2linux.codec-settings/v1",
            "requested":{"aac_quality":"high","aac_abr":false},
            "supported":{"aac_quality":["standard","high"],"aac_abr":true}});
        let a = bluetooth_quality(&aac);
        assert!(a.aac_supported && a.aac_abr_supported);
        assert_eq!(
            a.requested_aac,
            Some(reborn_core::platform::AacQuality::High)
        );
        assert_eq!(a.requested_aac_abr, Some(false));
        assert_eq!(q.requested_quality, Some(LdacQuality::High));
        assert_eq!(q.effective_quality, Some(LdacQuality::Standard));
        assert_eq!(q.requested_sbc, Some(SbcQuality::Xq));
        assert_eq!(q.effective_sbc, Some(SbcQuality::High));
        assert_eq!(q.requested_abr, Some(true));
        assert_eq!(q.effective_abr, Some(false));
        assert_eq!(bluetooth_quality(&json!({})), BluetoothQuality::default());
    }

    #[test]
    fn export_failure_is_not_reported_as_complete_and_retrieval_is_explained() {
        let failed = operation(&json!({"state":"Failed","failure":"disk_full"}));
        assert!(!failed.succeeded);
        let success = operation(
            &json!({"state":"Complete","redacted":true,"path":"/data/exports/report.tar.gz"}),
        );
        assert!(success.succeeded);
        assert!(success.facts.iter().any(|f| f.label == "Retrieve report"));
        assert!(success
            .facts
            .iter()
            .any(|f| f.label == "Archive" && f.value == "/data/exports/report.tar.gz"));
    }
}
