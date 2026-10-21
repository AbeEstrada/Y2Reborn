//! Internet radio: stations, the community directory and the stream fetcher.
//!
//! FFmpeg in Y2Linux is built without network protocols, so the network side
//! lives here: `curl` (TLS with certificate checks, redirects, timeouts)
//! fetches a station, this module strips the in-stream ICY title blocks and
//! hands plain audio bytes to the decoder through a pipe
//! (`reborn_media::Decoder::open_stream`). A dropped connection is retried
//! with a back-off; the decoder only sees the stream end when the station is
//! gone for good.

use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Identifies radio entries in the play queue (`Track::source_id`).
pub const SOURCE_ID: &str = reborn_core::RADIO_SOURCE;
const CURL: &str = "/usr/bin/curl";
const USER_AGENT: &str = concat!("Y2Reborn/", env!("CARGO_PKG_VERSION"));

fn curl() -> String {
    std::env::var("REBORN_CURL").unwrap_or_else(|_| CURL.into())
}

pub use reborn_core::RadioStation as Station;

fn http_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("http://") || lower.starts_with("https://"))
        && url.len() <= 2048
        && !url.chars().any(|c| c.is_control() || c.is_whitespace())
}

// ---------------------------------------------------------------- favorites

#[derive(Default, Serialize, Deserialize)]
struct SavedStations {
    schema: u32,
    favorites: Vec<Station>,
}

pub fn load_favorites(path: &Path) -> Vec<Station> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SavedStations>(&bytes).ok())
        .filter(|saved| saved.schema == 1)
        .map(|saved| {
            saved
                .favorites
                .into_iter()
                .filter(|s| http_url(&s.url))
                .take(200)
                .collect()
        })
        .unwrap_or_default()
}

pub fn save_favorites(path: &Path, favorites: &[Station]) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(&SavedStations {
        schema: 1,
        favorites: favorites.to_vec(),
    })
    .map_err(io::Error::other)?;
    reborn_core::atomic_write(path, &bytes)
}

// ---------------------------------------------------------------- playlists

/// Stations from .m3u/.m3u8/.pls text: the owner's own lists in a Radio
/// folder, and playlist URLs some stations hand out instead of a stream.
pub fn parse_playlist(text: &str) -> Vec<Station> {
    let mut stations = vec![];
    if text
        .lines()
        .any(|l| l.trim().eq_ignore_ascii_case("[playlist]"))
    {
        let mut files = std::collections::BTreeMap::<u32, (String, String)>::new();
        for line in text.lines() {
            let Some((key, value)) = line.trim().split_once('=') else {
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let index = |prefix: &str| key.strip_prefix(prefix).and_then(|n| n.parse::<u32>().ok());
            if let Some(n) = index("file") {
                files.entry(n).or_default().0 = value.trim().to_owned();
            } else if let Some(n) = index("title") {
                files.entry(n).or_default().1 = value.trim().to_owned();
            }
        }
        stations.extend(files.into_values().map(|(url, name)| Station {
            name,
            url,
            ..Default::default()
        }));
    } else {
        let mut title = String::new();
        for line in text.lines().map(str::trim) {
            if let Some(info) = line.strip_prefix("#EXTINF:") {
                title = info
                    .split_once(',')
                    .map(|(_, t)| t.trim().to_owned())
                    .unwrap_or_default();
            } else if !line.is_empty() && !line.starts_with('#') {
                stations.push(Station {
                    name: std::mem::take(&mut title),
                    url: line.to_owned(),
                    ..Default::default()
                });
            }
        }
    }
    stations.retain(|s| http_url(&s.url));
    for s in &mut stations {
        if s.name.is_empty() {
            s.name = s
                .url
                .split("://")
                .nth(1)
                .unwrap_or(&s.url)
                .split('/')
                .next()
                .unwrap_or("")
                .to_owned();
        }
    }
    stations
}

/// HLS playlists are segment lists, not stations: not supported yet.
fn is_hls(text: &str) -> bool {
    text.contains("#EXT-X-")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sniffed {
    Playlist,
    Hls,
    Audio,
}

/// A reply whose type says nothing useful (text/plain, octet-stream, none) or
/// that answers a playlist address with a non-audio type is looked at: many
/// servers send .m3u/.pls files that way, and they went to the decoder.
fn ambiguous(content_type: &str, url: &str) -> bool {
    let path = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .to_ascii_lowercase();
    let playlist_address = [".m3u", ".m3u8", ".pls"].iter().any(|e| path.ends_with(e));
    content_type.is_empty()
        || content_type.starts_with("text/plain")
        || content_type.starts_with("application/octet-stream")
        || content_type.starts_with("binary/")
        || (playlist_address && !content_type.starts_with("audio/"))
}

/// What the first bytes of an ambiguous reply are.
fn sniff(head: &[u8]) -> Sniffed {
    let text = String::from_utf8_lossy(&head[..head.len().min(4096)]);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    if is_hls(text) {
        return Sniffed::Hls;
    }
    let first = text
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if first.starts_with("#extm3u")
        || first == "[playlist]"
        || first.starts_with("http://")
        || first.starts_with("https://")
    {
        Sniffed::Playlist
    } else {
        Sniffed::Audio
    }
}

/// The owner's stations: every .m3u, .m3u8 and .pls file in a folder named
/// Radio (any letter case) at the top of Internal Storage or the SD card.
pub fn own_stations(storage_roots: &[PathBuf]) -> Vec<Station> {
    let mut stations = vec![];
    for root in storage_roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for folder in entries.flatten() {
            if !folder
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("radio")
            {
                continue;
            }
            let Ok(files) = std::fs::read_dir(folder.path()) else {
                continue;
            };
            let mut files: Vec<_> = files.flatten().map(|f| f.path()).collect();
            files.sort();
            for file in files {
                let extension = file
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase());
                if !matches!(extension.as_deref(), Some("m3u" | "m3u8" | "pls")) {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(&file) else {
                    continue;
                };
                if !meta.is_file() || meta.len() > 1 << 20 {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&file) else {
                    continue;
                };
                // The same text rules as music playlists: a station list
                // saved by older Windows tools keeps its accented names.
                let (text, _) = crate::playlists::decode(&bytes);
                if is_hls(&text) {
                    continue;
                }
                stations.extend(parse_playlist(&text));
            }
        }
    }
    stations.truncate(500);
    stations
}

// ---------------------------------------------------------------- ICY

/// Removes the metadata blocks a server interleaves every `metaint` audio
/// bytes when asked with `Icy-MetaData: 1`, and keeps the last StreamTitle.
pub struct IcyFilter {
    metaint: usize,
    until_meta: usize,
    meta_len: Option<usize>,
    meta: Vec<u8>,
    pub title: Option<String>,
}
impl IcyFilter {
    pub fn new(metaint: usize) -> Self {
        Self {
            metaint,
            until_meta: metaint,
            meta_len: None,
            meta: vec![],
            title: None,
        }
    }
    /// Appends the audio bytes of `input` to `audio`. Returns true when a new
    /// title arrived.
    pub fn feed(&mut self, mut input: &[u8], audio: &mut Vec<u8>) -> bool {
        if self.metaint == 0 {
            audio.extend_from_slice(input);
            return false;
        }
        let mut changed = false;
        while !input.is_empty() {
            if self.until_meta > 0 {
                let n = self.until_meta.min(input.len());
                audio.extend_from_slice(&input[..n]);
                self.until_meta -= n;
                input = &input[n..];
                continue;
            }
            match self.meta_len {
                None => {
                    self.meta_len = Some(input[0] as usize * 16);
                    self.meta.clear();
                    input = &input[1..];
                }
                Some(len) => {
                    let n = (len - self.meta.len()).min(input.len());
                    self.meta.extend_from_slice(&input[..n]);
                    input = &input[n..];
                }
            }
            if self.meta_len == Some(self.meta.len()) {
                if let Some(title) = stream_title(&self.meta) {
                    if self.title.as_deref() != Some(&title) {
                        self.title = Some(title);
                        changed = true;
                    }
                }
                self.meta_len = None;
                self.until_meta = self.metaint;
            }
        }
        changed
    }
}

/// `StreamTitle='Artist - Title';` → "Artist - Title". UTF-8, else Latin-1.
pub fn stream_title(block: &[u8]) -> Option<String> {
    let block = &block[..block.iter().position(|&b| b == 0).unwrap_or(block.len())];
    let text = match std::str::from_utf8(block) {
        Ok(text) => text.to_owned(),
        Err(_) => block.iter().map(|&b| b as char).collect(),
    };
    let start = text.find("StreamTitle='")? + "StreamTitle='".len();
    let rest = &text[start..];
    let end = rest.find("';").unwrap_or(rest.len());
    let title: String = rest[..end]
        .chars()
        .filter(|c| !c.is_control())
        .take(200)
        .collect();
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_owned())
}

// ---------------------------------------------------------------- HTTP

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub metaint: usize,
    pub name: String,
    pub bitrate: u32,
}

/// Reads curl's `--dump-header -` output up to the final response's body:
/// redirects and informational responses come first as their own blocks.
pub fn read_headers(reader: &mut impl BufRead) -> io::Result<Response> {
    loop {
        let mut response = Response::default();
        let mut first = true;
        let mut total = 0;
        loop {
            let mut line = Vec::new();
            let n = reader.take(8192).read_until(b'\n', &mut line)?;
            total += n;
            if n == 0 {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "no response"));
            }
            if total > 65536 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "headers too large",
                ));
            }
            let line = String::from_utf8_lossy(&line);
            let line = line.trim_end_matches(['\r', '\n']);
            if first {
                first = false;
                // "HTTP/1.1 200 OK", "HTTP/2 302", "ICY 200 OK"
                response.status = line
                    .split_whitespace()
                    .nth(1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                continue;
            }
            if line.is_empty() {
                break;
            }
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim();
            match key.trim().to_ascii_lowercase().as_str() {
                "content-type" => response.content_type = value.to_ascii_lowercase(),
                "icy-metaint" => response.metaint = value.parse().unwrap_or(0).min(1 << 20),
                "icy-name" => response.name = value.chars().take(120).collect(),
                "icy-br" => {
                    response.bitrate = value
                        .split(',')
                        .next()
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0)
                }
                _ => {}
            }
        }
        if (100..200).contains(&response.status) || (300..400).contains(&response.status) {
            continue;
        }
        return Ok(response);
    }
}

/// The player's curl for http(s) only: following redirects (at most five),
/// never another protocol, its own user agent.
fn curl_command() -> Command {
    let mut command = Command::new(curl());
    command
        .args(["--silent", "--location", "--max-redirs", "5"])
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--user-agent", USER_AGENT]);
    command
}

fn spawn_curl(url: &str, stream: bool) -> io::Result<Child> {
    let mut command = curl_command();
    command.args(["--show-error", "--connect-timeout", "10"]);
    if stream {
        // A live stream never ends; a stalled one is retried by the fetcher.
        command
            .args(["--header", "Icy-MetaData: 1", "--dump-header", "-"])
            .args(["--speed-limit", "1", "--speed-time", "15"]);
    } else {
        command.args(["--fail", "--max-time", "20", "--max-filesize", "4000000"]);
    }
    command
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
}

/// A small HTTP GET (directory queries, playlists), bounded in size and time.
pub fn fetch(url: &str) -> Result<Vec<u8>, String> {
    if !http_url(url) {
        return Err("not an http(s) address".into());
    }
    let output = spawn_curl(url, false)
        .and_then(|child| child.wait_with_output())
        .map_err(|e| format!("network tool unavailable: {e}"))?;
    if !output.status.success() {
        return Err(match output.status.code() {
            Some(6) => "Couldn't find the server. Check Wi-Fi.".into(),
            Some(7) | Some(28) => "The server didn't answer.".into(),
            Some(22) => "The server refused the request.".into(),
            Some(60) => "The server's certificate isn't trusted.".into(),
            _ => "The request failed.".into(),
        });
    }
    Ok(output.stdout)
}

// ---------------------------------------------------------------- logos

/// Logos larger than this are not downloaded (a logo is a few kilobytes).
const LOGO_MAX_BYTES: usize = 300_000;
/// A logo that could not be fetched is not asked for again this long.
const LOGO_RETRY: Duration = Duration::from_secs(3 * 24 * 3600);

/// Where a station logo is kept: `<cache>/radio-logos/<key>.img`.
pub fn logo_file(cache: &Path, key: u64) -> PathBuf {
    cache.join("radio-logos").join(format!("{key:016x}.img"))
}

/// The logo is on disk, or fetching it failed recently: do not fetch.
pub fn logo_settled(cache: &Path, key: u64) -> bool {
    let file = logo_file(cache, key);
    file.exists()
        || std::fs::metadata(file.with_extension("none"))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < LOGO_RETRY)
}

/// Download a station logo once and keep it as a picture the decoder reads:
/// PNG, JPEG or WebP as sent, the largest PNG inside an .ico. Anything else
/// (SVG, GIF, BMP icons, web pages) is remembered as missing for a while.
pub fn fetch_logo(url: &str, cache: &Path, key: u64) -> Result<PathBuf, String> {
    let file = logo_file(cache, key);
    let result = (|| {
        if !http_url(url) {
            return Err("not an http(s) address".to_owned());
        }
        let output = curl_command()
            .args(["--fail", "--connect-timeout", "8", "--max-time", "15"])
            .args(["--max-filesize", &LOGO_MAX_BYTES.to_string(), "--", url])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|e| format!("network tool unavailable: {e}"))?;
        if !output.status.success() || output.stdout.len() > LOGO_MAX_BYTES {
            return Err("logo not available".into());
        }
        logo_picture(&output.stdout)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| "unsupported logo format".to_owned())
    })();
    let _ = std::fs::create_dir_all(file.parent().unwrap_or(cache));
    match result {
        Ok(bytes) => {
            reborn_core::atomic_write(&file, &bytes).map_err(|e| e.to_string())?;
            Ok(file)
        }
        Err(e) => {
            let _ = std::fs::write(file.with_extension("none"), b"");
            Err(e)
        }
    }
}

/// The decodable picture in a downloaded logo, if any.
pub fn logo_picture(bytes: &[u8]) -> Option<&[u8]> {
    let png = |b: &[u8]| b.starts_with(b"\x89PNG\r\n\x1a\n");
    if png(bytes)
        || bytes.starts_with(&[0xFF, 0xD8, 0xFF])
        || (bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP")
    {
        return Some(bytes);
    }
    // ICO: reserved 0, type 1, count; 16-byte entries (width, height, ...,
    // size at 8, offset at 12). Pick the largest entry that is a PNG.
    if bytes.len() < 6 || bytes[..4] != [0, 0, 1, 0] {
        return None;
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let entry = |i: usize| -> Option<(u32, &[u8])> {
        let e = bytes.get(6 + i * 16..6 + i * 16 + 16)?;
        let side = |b: u8| if b == 0 { 256 } else { u32::from(b) };
        let size = u32::from_le_bytes(e[8..12].try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(e[12..16].try_into().ok()?) as usize;
        let data = bytes.get(offset..offset.checked_add(size)?)?;
        png(data).then_some((side(e[0]) * side(e[1]), data))
    };
    (0..count.min(32))
        .filter_map(entry)
        .max_by_key(|(area, _)| *area)
        .map(|(_, data)| data)
}

// ---------------------------------------------------------------- stream

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamState {
    Connecting,
    Playing,
    Reconnecting { attempt: u32 },
    Failed(String),
}

struct Shared {
    stop: AtomicBool,
    child: Mutex<Option<Child>>,
    state: Mutex<StreamState>,
    title: Mutex<Option<String>>,
    /// HTTP status and content type of the last reply, for diagnostics.
    reply: Mutex<Option<(u16, String)>>,
}

/// The station most recently started, for the player's Now Playing line.
static LIVE: Mutex<Option<std::sync::Weak<Shared>>> = Mutex::new(None);

/// State and announced title of the station being received, if any.
pub fn live() -> Option<(StreamState, Option<String>)> {
    let shared = LIVE.lock().ok()?.as_ref()?.upgrade()?;
    let state = shared.state.lock().ok()?.clone();
    let title = shared.title.lock().ok()?.clone();
    Some((state, title))
}

/// HTTP status and content type of the current station's last reply.
pub fn last_reply() -> Option<(u16, String)> {
    let shared = LIVE.lock().ok()?.as_ref()?.upgrade()?;
    let reply = shared.reply.lock().ok()?.clone();
    reply
}

/// One station being received. Dropping it stops the network transfer.
pub struct Stream {
    /// Taken (closed) first on drop: a fetcher blocked writing into a full
    /// pipe only wakes up when no reader is left.
    read: Option<OwnedFd>,
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl Stream {
    pub fn start(url: &str) -> io::Result<Self> {
        if !http_url(url) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not an http(s) address",
            ));
        }
        let mut fds = [0; 2];
        // SAFETY: pipe2 fills two new descriptors that are owned below.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both descriptors were just created and are not shared.
        let (read, write) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            child: Mutex::new(None),
            state: Mutex::new(StreamState::Connecting),
            title: Mutex::new(None),
            reply: Mutex::new(None),
        });
        if let Ok(mut live) = LIVE.lock() {
            *live = Some(Arc::downgrade(&shared));
        }
        let worker_shared = shared.clone();
        let url = url.to_owned();
        let worker = std::thread::Builder::new()
            .name("radio-fetch".into())
            .spawn(move || run(&url, std::fs::File::from(write), &worker_shared))?;
        Ok(Self {
            read: Some(read),
            shared,
            worker: Some(worker),
        })
    }
    /// The read end for `Decoder::open_stream`.
    pub fn reader(&self) -> BorrowedFd<'_> {
        self.read.as_ref().expect("open until drop").as_fd()
    }
    pub fn state(&self) -> StreamState {
        self.shared
            .state
            .lock()
            .map(|s| s.clone())
            .unwrap_or(StreamState::Connecting)
    }
    /// The song the station currently announces, when it does.
    pub fn title(&self) -> Option<String> {
        self.shared.title.lock().ok().and_then(|t| t.clone())
    }
}
impl Drop for Stream {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        // Beta 4 hung here: playback had stopped reading, the pipe was full,
        // the fetcher waited in write() and this join waited for it forever.
        // The decoder closes its own copy before this (field order in the
        // player's BoundDecoder); closing ours turns the write into EPIPE.
        drop(self.read.take());
        if let Ok(mut child) = self.shared.child.lock() {
            if let Some(child) = child.as_mut() {
                let _ = child.kill();
            }
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

const BACKOFF_S: [u64; 6] = [1, 2, 4, 8, 15, 30];

fn run(url: &str, mut pipe: std::fs::File, shared: &Shared) {
    let set = |state: StreamState| {
        if let Ok(mut s) = shared.state.lock() {
            *s = state;
        }
    };
    let mut url = url.to_owned();
    let mut failures = 0usize;
    let mut playlist_hops = 0;
    while !shared.stop.load(Ordering::SeqCst) {
        let outcome = receive(&url, &mut pipe, shared);
        if shared.stop.load(Ordering::SeqCst) {
            return;
        }
        match outcome {
            Receive::Playlist(next) if playlist_hops < 3 => {
                playlist_hops += 1;
                url = next;
                continue;
            }
            Receive::Playlist(_) => {
                return set(StreamState::Failed(
                    "The station's address leads nowhere.".into(),
                ))
            }
            Receive::Unsupported(reason) => return set(StreamState::Failed(reason)),
            Receive::PipeClosed => return,
            // A session that played for a while starts the back-off over.
            Receive::Ended { played } if played > Duration::from_secs(30) => failures = 0,
            Receive::Ended { .. } => {}
        }
        if failures >= BACKOFF_S.len() {
            return set(StreamState::Failed("The station isn't reachable.".into()));
        }
        set(StreamState::Reconnecting {
            attempt: failures as u32 + 1,
        });
        let until = Instant::now() + Duration::from_secs(BACKOFF_S[failures]);
        failures += 1;
        while Instant::now() < until && !shared.stop.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

enum Receive {
    Ended { played: Duration },
    Playlist(String),
    Unsupported(String),
    PipeClosed,
}

fn receive(url: &str, pipe: &mut std::fs::File, shared: &Shared) -> Receive {
    let started = Instant::now();
    let Ok(mut child) = spawn_curl(url, true) else {
        return Receive::Unsupported("The network tool is missing.".into());
    };
    let Some(stdout) = child.stdout.take() else {
        return Receive::Ended {
            played: Duration::ZERO,
        };
    };
    if let Ok(mut slot) = shared.child.lock() {
        *slot = Some(child);
    }
    let finish = |shared: &Shared| {
        if let Ok(mut slot) = shared.child.lock() {
            if let Some(mut child) = slot.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    };
    let mut reader = BufReader::with_capacity(16384, stdout);
    let response = match read_headers(&mut reader) {
        Ok(response) => response,
        Err(_) => {
            finish(shared);
            return Receive::Ended {
                played: Duration::ZERO,
            };
        }
    };
    if let Ok(mut reply) = shared.reply.lock() {
        *reply = Some((response.status, response.content_type.clone()));
    }
    if !(200..300).contains(&response.status) {
        finish(shared);
        return if response.status == 404 || response.status == 410 {
            Receive::Unsupported("The station is gone (address not found).".into())
        } else {
            Receive::Ended {
                played: Duration::ZERO,
            }
        };
    }
    let kind = &response.content_type;
    let sniffed = if ambiguous(kind, url) {
        reader.fill_buf().map(sniff).unwrap_or(Sniffed::Audio)
    } else {
        Sniffed::Audio
    };
    if kind.contains("mpegurl")
        || kind.contains("scpls")
        || kind.contains("x-pls")
        || kind.contains("dash+xml")
        || sniffed != Sniffed::Audio
    {
        let mut text = Vec::new();
        let _ = reader.take(1 << 20).read_to_end(&mut text);
        finish(shared);
        let text = String::from_utf8_lossy(&text);
        if is_hls(&text) || kind.contains("dash") {
            // BBC stations, for example, stream only this way now.
            return Receive::Unsupported(
                "This station only streams as HLS/DASH, which isn't supported yet.".into(),
            );
        }
        return match parse_playlist(&text).into_iter().next() {
            Some(station) => Receive::Playlist(station.url),
            None => Receive::Unsupported("The station's playlist is empty.".into()),
        };
    }
    if kind.starts_with("text/html") {
        finish(shared);
        return Receive::Unsupported("That address is a web page, not a stream.".into());
    }
    let mut filter = IcyFilter::new(response.metaint);
    let mut buffer = vec![0u8; 16384];
    let mut audio = Vec::with_capacity(32768);
    let mut announced = false;
    loop {
        let n = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        audio.clear();
        if filter.feed(&buffer[..n], &mut audio) {
            if let Ok(mut title) = shared.title.lock() {
                *title = filter.title.clone();
            }
        }
        if audio.is_empty() {
            continue;
        }
        if pipe.write_all(&audio).is_err() {
            finish(shared);
            return Receive::PipeClosed;
        }
        if !announced {
            announced = true;
            if let Ok(mut state) = shared.state.lock() {
                *state = StreamState::Playing;
            }
        }
    }
    finish(shared);
    Receive::Ended {
        played: started.elapsed(),
    }
}

// ---------------------------------------------------------------- directory

/// radio-browser.info, the free community station directory. Mirrors come
/// from DNS (all.api.radio-browser.info), as the service asks clients to do.
pub struct Directory {
    servers: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Category {
    /// Country code or tag sent back to the directory.
    pub id: String,
    pub name: String,
    pub stations: u32,
}

const FALLBACK_SERVERS: [&str; 3] = [
    "de1.api.radio-browser.info",
    "de2.api.radio-browser.info",
    "fi1.api.radio-browser.info",
];

impl Directory {
    pub fn discover() -> Self {
        let mut servers = resolve_mirrors();
        for fallback in FALLBACK_SERVERS {
            if !servers.iter().any(|s| s == fallback) {
                servers.push(fallback.to_owned());
            }
        }
        Self { servers }
    }
    fn get(&self, path_and_query: &str) -> Result<serde_json::Value, String> {
        let mut last = "Couldn't reach the station directory.".to_string();
        for server in &self.servers {
            match fetch(&format!("https://{server}{path_and_query}")) {
                Ok(bytes) => {
                    return serde_json::from_slice(&bytes)
                        .map_err(|_| "The directory sent something unreadable.".into())
                }
                Err(error) => last = error,
            }
        }
        Err(last)
    }
    pub fn popular(&self) -> Result<Vec<Station>, String> {
        self.get("/json/stations/topclick/150?hidebroken=true")
            .map(|v| stations(&v))
    }
    pub fn countries(&self) -> Result<Vec<Category>, String> {
        self.get("/json/countries?order=stationcount&reverse=true&hidebroken=true")
            .map(|v| categories(&v, "iso_3166_1", 20))
    }
    pub fn genres(&self) -> Result<Vec<Category>, String> {
        self.get("/json/tags?order=stationcount&reverse=true&hidebroken=true&limit=80")
            .map(|v| categories(&v, "name", 50))
    }
    pub fn by_country(&self, code: &str) -> Result<Vec<Station>, String> {
        self.search(&format!("countrycode={}", encode(code)))
    }
    pub fn by_genre(&self, tag: &str) -> Result<Vec<Station>, String> {
        self.search(&format!("tag={}&tagExact=true", encode(tag)))
    }
    pub fn by_name(&self, name: &str) -> Result<Vec<Station>, String> {
        self.search(&format!("name={}", encode(name)))
    }
    fn search(&self, filter: &str) -> Result<Vec<Station>, String> {
        self.get(&format!("/json/stations/search?{filter}&order=clickcount&reverse=true&hidebroken=true&limit=200"))
            .map(|v| stations(&v))
    }
    /// Counts the play in the directory (its popularity lists) and returns
    /// the station's current stream address, as the service asks clients to.
    pub fn play_url(&self, uuid: &str) -> Option<String> {
        if uuid.is_empty() || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return None;
        }
        let value = self.get(&format!("/json/url/{uuid}")).ok()?;
        value["url"]
            .as_str()
            .filter(|u| http_url(u))
            .map(str::to_owned)
    }
}

fn resolve_mirrors() -> Vec<String> {
    use std::net::ToSocketAddrs;
    let Ok(addresses) = ("all.api.radio-browser.info", 443).to_socket_addrs() else {
        return vec![];
    };
    let mut names = vec![];
    for address in addresses.take(8) {
        if let Some(name) = reverse_name(&address) {
            if name.ends_with(".api.radio-browser.info") && !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
}

fn reverse_name(address: &std::net::SocketAddr) -> Option<String> {
    let (storage, length) = match address {
        std::net::SocketAddr::V4(v4) => {
            // SAFETY: sockaddr_in is plain data; all fields are set below.
            let mut raw: libc::sockaddr_in = unsafe { std::mem::zeroed() };
            raw.sin_family = libc::AF_INET as libc::sa_family_t;
            raw.sin_port = v4.port().to_be();
            raw.sin_addr.s_addr = u32::from_ne_bytes(v4.ip().octets());
            // SAFETY: sockaddr_in fits in sockaddr_storage.
            let mut storage: libc::sockaddr_storage = unsafe { std::mem::zeroed() };
            unsafe { std::ptr::write(&mut storage as *mut _ as *mut libc::sockaddr_in, raw) };
            (storage, std::mem::size_of::<libc::sockaddr_in>())
        }
        std::net::SocketAddr::V6(_) => return None,
    };
    let mut host = [0 as libc::c_char; 256];
    // SAFETY: storage holds a valid sockaddr_in of `length` bytes; host is writable.
    let r = unsafe {
        libc::getnameinfo(
            &storage as *const _ as *const libc::sockaddr,
            length as libc::socklen_t,
            host.as_mut_ptr(),
            host.len() as libc::socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        )
    };
    if r != 0 {
        return None;
    }
    // SAFETY: getnameinfo wrote a NUL-terminated string into host.
    let name = unsafe { std::ffi::CStr::from_ptr(host.as_ptr()) };
    name.to_str().ok().map(str::to_owned)
}

/// The directory uses official country names ("The United Kingdom Of Great
/// Britain And Northern Ireland"); the screen needs the everyday one.
pub fn short_country(name: &str, code: &str) -> String {
    let known = [
        ("The United States Of America", "USA"),
        (
            "The United Kingdom Of Great Britain And Northern Ireland",
            "UK",
        ),
        ("The Russian Federation", "Russia"),
        ("Republic Of Korea", "South Korea"),
        ("The Islamic Republic Of Iran", "Iran"),
        ("Bolivarian Republic Of Venezuela", "Venezuela"),
        ("The Democratic Republic Of The Congo", "DR Congo"),
        ("United Republic Of Tanzania", "Tanzania"),
        ("Plurinational State Of Bolivia", "Bolivia"),
        ("Taiwan, Republic Of China", "Taiwan"),
    ];
    if let Some((_, short)) = known
        .iter()
        .find(|(long, _)| long.eq_ignore_ascii_case(name))
    {
        return (*short).to_owned();
    }
    let name = name.strip_prefix("The ").unwrap_or(name);
    if name.chars().count() > 20 && !code.is_empty() {
        code.to_ascii_uppercase()
    } else {
        name.to_owned()
    }
}

/// Percent-encoding for one query value.
pub fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Directory station objects → playable stations. HLS-only stations and
/// codecs the player can't decode are left out.
pub fn stations(value: &serde_json::Value) -> Vec<Station> {
    let Some(list) = value.as_array() else {
        return vec![];
    };
    let mut seen = std::collections::HashSet::new();
    list.iter()
        .filter(|s| s["hls"].as_i64() != Some(1))
        .filter_map(|s| {
            let text = |key: &str| s[key].as_str().unwrap_or("").trim().to_owned();
            let codec = text("codec").to_ascii_uppercase();
            if !matches!(
                codec.as_str(),
                "MP3" | "AAC" | "AAC+" | "OGG" | "FLAC" | "UNKNOWN" | ""
            ) {
                return None;
            }
            let url = Some(text("url_resolved"))
                .filter(|u| http_url(u))
                .or_else(|| Some(text("url")).filter(|u| http_url(u)))?;
            let name: String = text("name")
                .chars()
                .filter(|c| !c.is_control())
                .take(120)
                .collect();
            if name.is_empty() || !seen.insert(url.clone()) {
                return None;
            }
            Some(Station {
                uuid: text("stationuuid"),
                name,
                url,
                country: short_country(&text("country"), &text("countrycode")),
                codec: if codec == "UNKNOWN" {
                    String::new()
                } else {
                    codec
                },
                bitrate: s["bitrate"].as_u64().unwrap_or(0).min(10_000) as u32,
                tags: text("tags").chars().take(120).collect(),
                favicon: Some(text("favicon"))
                    .filter(|u| http_url(u) && u.len() <= 512)
                    .unwrap_or_default(),
                homepage: Some(text("homepage"))
                    .filter(|u| http_url(u) && u.len() <= 512)
                    .unwrap_or_default(),
                language: text("language").chars().take(60).collect(),
                countrycode: text("countrycode").chars().take(2).collect(),
                votes: s["votes"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
                clicks: s["clickcount"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32,
            })
        })
        .collect()
}

fn categories(value: &serde_json::Value, id_key: &str, minimum: u32) -> Vec<Category> {
    let Some(list) = value.as_array() else {
        return vec![];
    };
    list.iter()
        .filter_map(|c| {
            let id = c[id_key].as_str()?.trim().to_owned();
            let name = c["name"].as_str()?.trim();
            let name = if id_key == "iso_3166_1" {
                short_country(name, &id)
            } else {
                name.to_owned()
            };
            let stations = c["stationcount"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
            (!name.is_empty() && !id.is_empty() && stations >= minimum).then_some(Category {
                id,
                name,
                stations,
            })
        })
        .take(250)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `live()` names the station started last in this process; tests that
    /// start one take turns, or a parallel test's station replaces theirs.
    static STREAMS: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn playlists_served_as_plain_text_or_bytes_are_recognised() {
        assert!(ambiguous("text/plain", "http://example.org/live"));
        assert!(ambiguous(
            "application/octet-stream",
            "http://example.org/x"
        ));
        assert!(ambiguous("", "http://example.org/x"));
        assert!(ambiguous("text/html", "http://example.org/radio2.pls?x=1"));
        assert!(!ambiguous("audio/mpeg", "http://example.org/stream.m3u"));
        assert!(!ambiguous("audio/aac", "http://example.org/live"));
        assert_eq!(
            sniff(b"#EXTM3U\r\n#EXTINF:-1,Radio\r\nhttp://a/b\r\n"),
            Sniffed::Playlist
        );
        assert_eq!(
            sniff("\u{feff}[playlist]\nFile1=http://a/b\n".as_bytes()),
            Sniffed::Playlist
        );
        assert_eq!(sniff(b"\nhttps://a.example/stream\n"), Sniffed::Playlist);
        assert_eq!(
            sniff(b"#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-STREAM-INF:BANDWIDTH=1\nx.m3u8\n"),
            Sniffed::Hls
        );
        // MP3 frames and ID3 tags are audio.
        assert_eq!(sniff(&[0xff, 0xfb, 0x90, 0x64, 0x00]), Sniffed::Audio);
        assert_eq!(sniff(b"ID3\x04\x00\x00\x00\x00\x00\x00"), Sniffed::Audio);
    }

    #[test]
    fn icy_blocks_are_removed_and_titles_kept_across_any_split() {
        // metaint 4: 4 audio bytes, a length byte, 16*len metadata bytes, ...
        let mut wire = b"AAAA".to_vec();
        let meta = b"StreamTitle='Artist - Song';";
        wire.push(2);
        let mut block = meta.to_vec();
        block.resize(32, 0);
        wire.extend_from_slice(&block);
        wire.extend_from_slice(b"BBBB");
        wire.push(0); // empty metadata
        wire.extend_from_slice(b"CC");
        for split in 1..wire.len() {
            let mut filter = IcyFilter::new(4);
            let mut audio = vec![];
            let mut changes = 0;
            for chunk in wire.chunks(split) {
                changes += filter.feed(chunk, &mut audio) as usize;
            }
            assert_eq!(audio, b"AAAABBBBCC", "split {split}");
            assert_eq!(filter.title.as_deref(), Some("Artist - Song"));
            assert_eq!(changes, 1);
        }
        // No metadata interval: bytes pass through untouched.
        let mut audio = vec![];
        assert!(!IcyFilter::new(0).feed(b"\x02StreamTitle", &mut audio));
        assert_eq!(audio, b"\x02StreamTitle");
    }

    #[test]
    fn stream_titles_handle_latin1_and_junk() {
        assert_eq!(
            stream_title(b"StreamTitle='Caf\xe9 del Mar';StreamUrl='';").as_deref(),
            Some("Café del Mar")
        );
        assert_eq!(
            stream_title("StreamTitle='Björk – Jóga';".as_bytes()).as_deref(),
            Some("Björk – Jóga")
        );
        assert_eq!(stream_title(b"StreamTitle='';"), None);
        assert_eq!(stream_title(b"garbage"), None);
        assert_eq!(stream_title(b"StreamTitle='No end"), Some("No end".into()));
    }

    #[test]
    fn headers_follow_redirects_to_the_final_response() {
        let text = "HTTP/1.1 302 Found\r\nLocation: http://x/\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nicy-metaint: 16000\r\nicy-name: Test FM\r\nicy-br: 128\r\n\r\nAUDIO";
        let mut reader = io::Cursor::new(text.as_bytes());
        let response = read_headers(&mut reader).unwrap();
        assert_eq!(
            response,
            Response {
                status: 200,
                content_type: "audio/mpeg".into(),
                metaint: 16000,
                name: "Test FM".into(),
                bitrate: 128
            }
        );
        let mut rest = String::new();
        reader.read_to_string(&mut rest).unwrap();
        assert_eq!(rest, "AUDIO");
        let mut icy = io::Cursor::new(b"ICY 200 OK\r\nicy-metaint:8192\r\n\r\n".to_vec());
        assert_eq!(read_headers(&mut icy).unwrap().metaint, 8192);
        assert!(read_headers(&mut io::Cursor::new(b"".to_vec())).is_err());
    }

    #[test]
    fn playlists_in_both_formats_and_only_web_addresses() {
        let m3u = "#EXTM3U\n#EXTINF:-1,Jazz FM\nhttp://jazz.example/stream\nfile:///etc/passwd\n\nhttps://other.example:8000/live.aac\n";
        let stations = parse_playlist(m3u);
        assert_eq!(stations.len(), 2);
        assert_eq!(
            (stations[0].name.as_str(), stations[0].url.as_str()),
            ("Jazz FM", "http://jazz.example/stream")
        );
        assert_eq!(stations[1].name, "other.example:8000");
        let pls = "[playlist]\nNumberOfEntries=2\nFile2=http://b.example/\nTitle2=B\nFile1=http://a.example/\nTitle1=A\n";
        let stations = parse_playlist(pls);
        assert_eq!(
            stations.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["A", "B"]
        );
        assert!(is_hls("#EXTM3U\n#EXT-X-TARGETDURATION:6\nseg1.ts"));
    }

    #[test]
    fn own_station_lists_in_windows_encoding_keep_their_names() {
        let root = std::env::temp_dir().join(format!("reborn-radio-cp1252-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Radio")).unwrap();
        // "#EXTINF:-1,Café Bleu" in Windows-1252 (é = 0xE9), not UTF-8.
        let mut bytes = b"#EXTM3U\n#EXTINF:-1,Caf".to_vec();
        bytes.extend([0xE9]);
        bytes.extend(b" Bleu\nhttp://cafe.example/stream\n");
        std::fs::write(root.join("Radio/mine.m3u"), bytes).unwrap();
        let stations = own_stations(std::slice::from_ref(&root));
        assert_eq!(stations.len(), 1);
        assert_eq!(stations[0].name, "Café Bleu");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn own_stations_come_from_radio_folders_only() {
        let root = std::env::temp_dir().join(format!("rb-radio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("RADIO")).unwrap();
        std::fs::create_dir_all(root.join("Music")).unwrap();
        std::fs::write(root.join("RADIO/news.m3u"), "http://news.example/\n").unwrap();
        std::fs::write(
            root.join("RADIO/hls.m3u8"),
            "#EXTM3U\n#EXT-X-VERSION:3\nhttp://x/a.ts\n",
        )
        .unwrap();
        std::fs::write(root.join("RADIO/notes.txt"), "http://no.example/\n").unwrap();
        std::fs::write(root.join("Music/elsewhere.m3u"), "http://no.example/\n").unwrap();
        let stations = own_stations(&[root.clone(), root.join("missing")]);
        assert_eq!(
            stations.iter().map(|s| s.url.as_str()).collect::<Vec<_>>(),
            ["http://news.example/"]
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn directory_entries_become_playable_stations() {
        let value = serde_json::json!([
            {"stationuuid":"a-1","name":"One","url":"http://one/pls","url_resolved":"http://one/stream","country":"Germany","codec":"MP3","bitrate":128,"tags":"jazz,smooth","hls":0},
            {"stationuuid":"b-2","name":"HLS only","url_resolved":"https://two/live.m3u8","codec":"AAC","hls":1},
            {"stationuuid":"c-3","name":"Video","url_resolved":"http://three/","codec":"H.264"},
            {"stationuuid":"d-4","name":"Duplicate","url_resolved":"http://one/stream","codec":"MP3"},
            {"stationuuid":"e-5","name":"","url_resolved":"http://five/","codec":"OGG"},
            {"stationuuid":"f-6","name":"Plain","url":"https://six/","codec":"UNKNOWN","bitrate":0}
        ]);
        let stations = stations(&value);
        assert_eq!(
            stations.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["One", "Plain"]
        );
        assert_eq!(stations[0].summary(), "Germany · MP3 128 kbps · jazz");
        assert_eq!(stations[1].codec, "");
        assert_eq!(stations[1].url, "https://six/");
        assert_eq!(encode("drum & bass/ü"), "drum%20%26%20bass%2F%C3%BC");
        assert_eq!(
            short_country(
                "The United Kingdom Of Great Britain And Northern Ireland",
                "GB"
            ),
            "UK"
        );
        assert_eq!(short_country("The Netherlands", "NL"), "Netherlands");
        assert_eq!(
            short_country("Saint Vincent And The Grenadines", "VC"),
            "VC"
        );
        assert_eq!(short_country("Germany", "DE"), "Germany");
    }

    #[test]
    fn favorites_survive_a_restart_and_ignore_bad_entries() {
        let path =
            std::env::temp_dir().join(format!("rb-radio-fav-{}/radio.json", std::process::id()));
        let station = Station {
            uuid: "a".into(),
            name: "A".into(),
            url: "http://a/".into(),
            ..Default::default()
        };
        save_favorites(&path, std::slice::from_ref(&station)).unwrap();
        assert_eq!(load_favorites(&path), vec![station]);
        std::fs::write(
            &path,
            r#"{"schema":1,"favorites":[{"name":"x","url":"file:///etc/passwd"}]}"#,
        )
        .unwrap();
        assert!(load_favorites(&path).is_empty());
        std::fs::write(&path, "not json").unwrap();
        assert!(load_favorites(&path).is_empty());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// A tiny Shoutcast-like server on localhost: ICY titles every 1000
    /// bytes, then the connection drops; the fetcher must reconnect.
    #[test]
    fn stream_strips_titles_and_reconnects_after_a_drop() {
        let _turn = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
        use std::net::TcpListener;
        if !Path::new(&curl()).exists() {
            return; // no curl on this host
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            for connection in 0..2u8 {
                let (mut socket, _) = listener.accept().unwrap();
                let mut request = [0u8; 2048];
                let n = socket.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..n]).contains("Icy-MetaData: 1"));
                let _ = socket.write_all(
                    b"HTTP/1.0 200 OK\r\nContent-Type: audio/mpeg\r\nicy-metaint: 1000\r\n\r\n",
                );
                for block in 0..3u8 {
                    let _ = socket.write_all(&[b'0' + connection; 1000]);
                    let mut meta = format!("StreamTitle='Song {connection}{block}';").into_bytes();
                    meta.resize(48, 0);
                    let _ = socket.write_all(&[3]);
                    let _ = socket.write_all(&meta);
                }
                // dropped here: the station "goes away" for a moment
            }
        });
        let stream = Stream::start(&format!("http://127.0.0.1:{port}/live")).unwrap();
        let mut file = std::fs::File::from(stream.reader().try_clone_to_owned().unwrap());
        let mut received = vec![0u8; 6000];
        file.read_exact(&mut received).unwrap();
        assert!(received[..3000].iter().all(|&b| b == b'0'));
        assert!(
            received[3000..].iter().all(|&b| b == b'1'),
            "second connection after a reconnect"
        );
        assert_eq!(stream.title().as_deref(), Some("Song 12"));
        assert_eq!(live().map(|(_, title)| title), Some(Some("Song 12".into())));
        server.join().unwrap();
        drop(stream);
        assert!(live().is_none(), "a stopped station is not live");
    }

    /// Playback stops reading (paused, or the output stalled), the pipe fills
    /// and the fetcher waits in write(); stopping the station must still
    /// return at once. Beta 4 deadlocked here on the player.
    #[test]
    fn stopping_a_station_with_a_full_pipe_returns_promptly() {
        let _turn = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
        use std::net::TcpListener;
        if !Path::new(&curl()).exists() {
            return;
        }
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            let mut request = [0u8; 2048];
            let _ = socket.read(&mut request);
            let _ = socket.write_all(b"HTTP/1.0 200 OK\r\nContent-Type: audio/mpeg\r\n\r\n");
            let block = [0x55u8; 16384];
            while socket.write_all(&block).is_ok() {}
        });
        let stream = Stream::start(&format!("http://127.0.0.1:{port}/live")).unwrap();
        // Nobody reads: let the pipe (64 KiB) and the socket buffers fill.
        std::thread::sleep(Duration::from_millis(1500));
        assert_eq!(stream.state(), StreamState::Playing);
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            drop(stream);
            let _ = done.send(());
        });
        assert!(
            finished.recv_timeout(Duration::from_secs(5)).is_ok(),
            "stopping the station hung"
        );
    }

    #[test]
    fn logos_are_kept_as_png_jpeg_or_webp_and_icons_give_their_largest_png() {
        let png: &[u8] = b"\x89PNG\r\n\x1a\nrest";
        assert_eq!(logo_picture(png), Some(png));
        assert!(logo_picture(&[0xFF, 0xD8, 0xFF, 0xE0]).is_some());
        assert!(logo_picture(b"<svg xmlns=...").is_none());
        assert!(logo_picture(b"GIF89a....").is_none());
        // An .ico with a 16 px BMP and a 64 px PNG entry: the PNG.
        let small_png = [png, b"16"].concat();
        let big_png = [png, b"64px"].concat();
        let mut ico = vec![0, 0, 1, 0, 3, 0];
        let header = 6 + 3 * 16;
        let entries: [(u8, &[u8]); 3] = [(16, b"BMPDATA"), (64, &big_png), (32, &small_png)];
        let mut offset = header;
        for (side, data) in entries {
            ico.extend([side, side, 0, 0, 1, 0, 32, 0]);
            ico.extend((data.len() as u32).to_le_bytes());
            ico.extend((offset as u32).to_le_bytes());
            offset += data.len();
        }
        for (_, data) in entries {
            ico.extend(data);
        }
        assert_eq!(logo_picture(&ico), Some(&big_png[..]));
        // A corrupt directory entry never reads outside the file.
        let mut broken = ico.clone();
        broken[6 + 16 + 12] = 0xFF;
        assert_eq!(logo_picture(&broken), Some(&small_png[..]));
    }

    /// Live check against the real directory and a real station (needs the
    /// internet): `cargo test -p reborn-platform -- --ignored live_`.
    #[test]
    #[ignore]
    fn live_directory_and_station() {
        let _turn = STREAMS.lock().unwrap_or_else(|e| e.into_inner());
        let directory = Directory::discover();
        let popular = directory.popular().expect("directory reachable");
        assert!(popular.len() > 50, "{} stations", popular.len());
        let countries = directory.countries().unwrap();
        assert!(countries.iter().any(|c| c.id == "DE"));
        let jazz = directory.by_genre("jazz").unwrap();
        assert!(!jazz.is_empty());
        let station = popular
            .iter()
            .find(|s| s.codec == "MP3")
            .expect("an MP3 station");
        eprintln!(
            "station: {} {} {}",
            station.name,
            station.summary(),
            station.url
        );
        let stream = Stream::start(&station.url).unwrap();
        let mut file = std::fs::File::from(stream.reader().try_clone_to_owned().unwrap());
        let mut audio = vec![0u8; 64 * 1024];
        file.read_exact(&mut audio).unwrap();
        eprintln!("state {:?}, title {:?}", stream.state(), stream.title());
        assert_eq!(stream.state(), StreamState::Playing);
    }
}
