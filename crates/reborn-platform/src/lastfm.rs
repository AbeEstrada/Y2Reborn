//! Last.fm: sign-in, Now Playing and scrobbling over the Last.fm API 2.0.
//!
//! Requests go to `https://ws.audioscrobbler.com/2.0/` as signed POST forms
//! through `curl` (TLS with certificate checks, as for internet radio). The
//! form travels on curl's standard input, never on its command line, so no
//! key, session or password shows in the process list. Signing is Last.fm's
//! scheme: MD5 over the parameters sorted by name, each name followed by its
//! value, then the app's shared secret.
//!
//! Secrets: the app's key and shared secret come from the release image
//! (`/usr/share/reborn/lastfm-api.json`, not in the source); a build without
//! it has no Last.fm. The listener's session key lives only in
//! `/data/reborn/state/lastfm.json` (0600). A password (fallback sign-in) is
//! used for one request and never stored. Nothing here logs a secret.
use reborn_core::scrobble::Scrobble;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

pub const API: &str = "https://ws.audioscrobbler.com/2.0/";
pub const AUTH_PAGE: &str = "https://www.last.fm/api/auth/";
pub const CREDENTIALS: &str = "/usr/share/reborn/lastfm-api.json";
/// Last.fm takes at most 50 plays per scrobble request.
pub const BATCH: usize = 50;
const CURL: &str = "/usr/bin/curl";

fn curl() -> String {
    std::env::var("REBORN_CURL").unwrap_or_else(|_| CURL.into())
}

// ------------------------------------------------------------------ MD5

/// RFC 1321 MD5 (Last.fm's request signature; not used for anything else).
pub fn md5(data: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    // floor(abs(sin(i + 1)) * 2^32), RFC 1321.
    const K: [u32; 64] = [
        0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613,
        0xfd469501, 0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193,
        0xa679438e, 0x49b40821, 0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d,
        0x02441453, 0xd8a1e681, 0xe7d3fbc8, 0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed,
        0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a, 0xfffa3942, 0x8771f681, 0x6d9d6122,
        0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70, 0x289b7ec6, 0xeaa127fa,
        0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665, 0xf4292244,
        0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
        0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb,
        0xeb86d391,
    ];
    let mut state: [u32; 4] = [0x6745_2301, 0xefcd_ab89, 0x98ba_dcfe, 0x1032_5476];
    let mut message = data.to_vec();
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&((data.len() as u64).wrapping_mul(8)).to_le_bytes());
    for chunk in message.chunks(64) {
        let m: Vec<u32> = chunk
            .chunks(4)
            .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
            .collect();
        let [mut a, mut b, mut c, mut d] = state;
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        for (s, v) in state.iter_mut().zip([a, b, c, d]) {
            *s = s.wrapping_add(v);
        }
    }
    let mut out = [0u8; 16];
    for (i, s) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&s.to_le_bytes());
    }
    out
}
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ------------------------------------------------------- app credentials

#[derive(Clone, Deserialize)]
pub struct Credentials {
    pub api_key: String,
    pub shared_secret: String,
}
impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Credentials { .. }")
    }
}
/// The app's Last.fm key, when this build carries one.
pub fn credentials(path: &Path) -> Option<Credentials> {
    let c: Credentials = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    let ok = |v: &str| v.len() == 32 && v.bytes().all(|b| b.is_ascii_hexdigit());
    (ok(&c.api_key) && ok(&c.shared_secret)).then_some(c)
}

// --------------------------------------------------------------- session

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Session {
    pub name: String,
    pub key: String,
}
impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Session {{ name: {:?}, .. }}", self.name)
    }
}
pub fn load_session(path: &Path) -> Option<Session> {
    let s: Session = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (!s.name.is_empty() && !s.key.is_empty()).then_some(s)
}
pub fn save_session(path: &Path, session: &Session) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        file.write_all(&serde_json::to_vec(session).map_err(io::Error::other)?)?;
        file.sync_all()?;
    }
    fs::rename(tmp, path)
}
/// Last.fm revoked the session: keep only the account's name (no key, so
/// not connected), so plays waiting for it never go to another account.
pub fn invalidate_session(path: &Path) -> io::Result<()> {
    match load_session(path) {
        Some(s) => save_session(
            path,
            &Session {
                name: s.name,
                key: String::new(),
            },
        ),
        None => Ok(()),
    }
}
/// The account the waiting plays belong to, signed in or not.
pub fn account_of(path: &Path) -> Option<String> {
    let s: Session = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (!s.name.is_empty()).then_some(s.name)
}
pub fn forget_session(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

// --------------------------------------------------------------- errors

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// No connection, DNS or TLS failure: try again later.
    Offline(String),
    /// 9: the session was revoked or expired: sign in again.
    InvalidSession,
    /// 14: the listener has not approved the sign-in yet (keep asking).
    NotApproved,
    /// 15: the sign-in code expired: start over.
    TokenExpired,
    /// 4: wrong username or password (fallback sign-in).
    WrongLogin,
    /// 11, 16: Last.fm is unavailable for now.
    Unavailable,
    /// 29: too many requests: wait.
    RateLimited,
    /// 10, 26 (key) or anything else Last.fm refuses.
    Refused(i64, String),
}
impl Error {
    pub fn retry_later(&self) -> bool {
        matches!(
            self,
            Self::Offline(_) | Self::Unavailable | Self::RateLimited
        )
    }
    /// For the listener, in Reborn's words.
    pub fn message(&self) -> String {
        match self {
            Self::Offline(_) => "Last.fm can't be reached right now".into(),
            Self::InvalidSession => "Sign in to Last.fm again".into(),
            Self::NotApproved => "Waiting for approval on last.fm".into(),
            Self::TokenExpired => "That sign-in expired. Try again".into(),
            Self::WrongLogin => "Wrong Last.fm username or password".into(),
            Self::Unavailable => "Last.fm is unavailable right now".into(),
            Self::RateLimited => "Last.fm asked to wait a moment".into(),
            Self::Refused(code, _) => format!("Last.fm refused the request ({code})"),
        }
    }
}
fn classify(code: i64, message: &str) -> Error {
    match code {
        4 => Error::WrongLogin,
        9 => Error::InvalidSession,
        11 | 16 => Error::Unavailable,
        14 => Error::NotApproved,
        15 => Error::TokenExpired,
        29 => Error::RateLimited,
        _ => Error::Refused(code, message.chars().take(120).collect()),
    }
}

// --------------------------------------------------------------- requests

/// `application/x-www-form-urlencoded` value encoding.
pub fn form_encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Last.fm's `api_sig`: names sorted, each name then its value, then the
/// secret; `format` and `callback` are not signed.
pub fn signature(params: &[(String, String)], secret: &str) -> String {
    let mut sorted: Vec<&(String, String)> = params
        .iter()
        .filter(|(k, _)| k != "format" && k != "callback")
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut text = String::new();
    for (k, v) in sorted {
        text.push_str(k);
        text.push_str(v);
    }
    text.push_str(secret);
    hex(&md5(text.as_bytes()))
}

#[derive(Debug, Clone)]
pub struct Client {
    pub credentials: Credentials,
    pub endpoint: String,
    /// The curl program (tests use a stand-in).
    pub curl: String,
}
impl Client {
    pub fn new(credentials: Credentials) -> Self {
        Self {
            credentials,
            endpoint: API.into(),
            curl: curl(),
        }
    }
    fn call(&self, method: &str, mut params: Vec<(String, String)>) -> Result<Value, Error> {
        params.push(("method".into(), method.into()));
        params.push(("api_key".into(), self.credentials.api_key.clone()));
        let sig = signature(&params, &self.credentials.shared_secret);
        params.push(("api_sig".into(), sig));
        params.push(("format".into(), "json".into()));
        let body: Vec<String> = params
            .iter()
            .map(|(k, v)| format!("{}={}", form_encode(k), form_encode(v)))
            .collect();
        let reply = post(&self.curl, &self.endpoint, body.join("&").as_bytes())?;
        let value: Value = serde_json::from_slice(&reply)
            .map_err(|_| Error::Offline("unreadable reply".into()))?;
        if let Some(code) = value.get("error").and_then(Value::as_i64) {
            let message = value.get("message").and_then(Value::as_str).unwrap_or("");
            return Err(classify(code, message));
        }
        Ok(value)
    }
    /// Step 1 of the sign-in: a code the listener approves on last.fm.
    pub fn token(&self) -> Result<String, Error> {
        let v = self.call("auth.getToken", vec![])?;
        v.get("token")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .ok_or_else(|| Error::Refused(0, "no token".into()))
    }
    /// The page the listener opens (QR code) to approve `token`.
    pub fn approval_url(&self, token: &str) -> String {
        format!(
            "{AUTH_PAGE}?api_key={}&token={}",
            self.credentials.api_key,
            form_encode(token)
        )
    }
    /// Step 2: the session, once approved (`NotApproved` until then).
    pub fn session(&self, token: &str) -> Result<Session, Error> {
        let v = self.call("auth.getSession", vec![("token".into(), token.into())])?;
        session_of(&v)
    }
    /// Fallback sign-in with username and password (never stored).
    pub fn mobile_session(&self, username: &str, password: &str) -> Result<Session, Error> {
        let v = self.call(
            "auth.getMobileSession",
            vec![
                ("username".into(), username.into()),
                ("password".into(), password.into()),
            ],
        )?;
        session_of(&v)
    }
    pub fn now_playing(
        &self,
        session: &Session,
        play: &reborn_core::scrobble::Play,
    ) -> Result<(), Error> {
        let mut params = vec![
            ("artist".into(), play.artist.clone()),
            ("track".into(), play.title.clone()),
            ("duration".into(), play.duration_s.to_string()),
            ("sk".into(), session.key.clone()),
        ];
        if !play.album.is_empty() {
            params.push(("album".into(), play.album.clone()));
        }
        if !play.album_artist.is_empty() {
            params.push(("albumArtist".into(), play.album_artist.clone()));
        }
        if play.track_number > 0 {
            params.push(("trackNumber".into(), play.track_number.to_string()));
        }
        self.call("track.updateNowPlaying", params).map(|_| ())
    }
    /// Send up to [`BATCH`] plays. Returns (accepted, ignored).
    pub fn scrobble(&self, session: &Session, plays: &[Scrobble]) -> Result<(u32, u32), Error> {
        let mut params = vec![("sk".to_owned(), session.key.clone())];
        for (i, s) in plays.iter().take(BATCH).enumerate() {
            params.push((format!("artist[{i}]"), s.play.artist.clone()));
            params.push((format!("track[{i}]"), s.play.title.clone()));
            params.push((format!("timestamp[{i}]"), s.timestamp.to_string()));
            params.push((format!("duration[{i}]"), s.play.duration_s.to_string()));
            if !s.play.album.is_empty() {
                params.push((format!("album[{i}]"), s.play.album.clone()));
            }
            if !s.play.album_artist.is_empty() {
                params.push((format!("albumArtist[{i}]"), s.play.album_artist.clone()));
            }
            if s.play.track_number > 0 {
                params.push((format!("trackNumber[{i}]"), s.play.track_number.to_string()));
            }
        }
        let v = self.call("track.scrobble", params)?;
        let attr = v.get("scrobbles").and_then(|s| s.get("@attr"));
        let count = |name: &str| {
            attr.and_then(|a| a.get(name))
                .and_then(|n| {
                    n.as_u64()
                        .or_else(|| n.as_str().and_then(|s| s.parse().ok()))
                })
                .unwrap_or(0) as u32
        };
        Ok((count("accepted"), count("ignored")))
    }
}
fn session_of(v: &Value) -> Result<Session, Error> {
    let s = v
        .get("session")
        .ok_or_else(|| Error::Refused(0, "no session".into()))?;
    let name = s
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let key = s
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    if name.is_empty() || key.is_empty() {
        return Err(Error::Refused(0, "no session".into()));
    }
    Ok(Session { name, key })
}

/// One HTTPS POST: the form on curl's standard input, the reply body back.
fn post(curl: &str, url: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
    let mut child = Command::new(curl)
        .args(["-sS", "--proto", "=https", "--tlsv1.2", "--max-time", "20"])
        .args([
            "--connect-timeout",
            "10",
            "-H",
            "Content-Type: application/x-www-form-urlencoded",
        ])
        .args(["-A", concat!("Y2Reborn/", env!("CARGO_PKG_VERSION"))])
        .args(["--data-binary", "@-", url])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::Offline(e.to_string()))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(body)
            .map_err(|e| Error::Offline(e.to_string()))?;
    }
    let mut reply = vec![];
    if let Some(mut out) = child.stdout.take() {
        let _ = out.by_ref().take(256 * 1024).read_to_end(&mut reply);
    }
    let status = child.wait().map_err(|e| Error::Offline(e.to_string()))?;
    // Last.fm answers errors with a JSON body and HTTP 4xx; curl still exits 0.
    if !status.success() && reply.is_empty() {
        return Err(Error::Offline(format!(
            "curl {}",
            status.code().unwrap_or(-1)
        )));
    }
    Ok(reply)
}

// -------------------------------------------------------------- sending

/// Last.fm's error for a request it can never accept as it is.
const INVALID_PARAMETERS: i64 = 6;

/// Send waiting plays oldest first, in batches, until the queue is empty or
/// an error stops it. Too old plays (Last.fm would ignore them) and plays it
/// calls invalid are dropped. Returns how many were accepted.
pub fn flush(
    client: &Client,
    session: &Session,
    queue: &crate::scrobble::Queue,
    now_unix: i64,
) -> Result<u32, Error> {
    let mut accepted = 0;
    loop {
        let waiting = queue.load();
        let stale = waiting
            .iter()
            .take_while(|s| now_unix - s.timestamp > crate::scrobble::MAX_AGE_S)
            .count();
        if stale > 0 {
            if !queue
                .drop_sent(&waiting[..stale])
                .map_err(|e| Error::Offline(e.to_string()))?
            {
                return Ok(accepted); // the queue changed under us (Disconnect)
            }
            continue;
        }
        if waiting.is_empty() {
            return Ok(accepted);
        }
        let batch = &waiting[..waiting.len().min(BATCH)];
        accepted += match client.scrobble(session, batch) {
            Ok((ok, _ignored)) => ok,
            // A play Last.fm calls invalid would block every later one:
            // send this batch one by one and leave out only such plays.
            Err(Error::Refused(INVALID_PARAMETERS, _)) => {
                let mut ok = 0;
                for (i, play) in batch.iter().enumerate() {
                    match client.scrobble(session, std::slice::from_ref(play)) {
                        Ok((n, _)) => ok += n,
                        Err(Error::Refused(INVALID_PARAMETERS, _)) => {}
                        Err(e) => {
                            let _ = queue.drop_sent(&batch[..i]);
                            return Err(e);
                        }
                    }
                }
                ok
            }
            Err(e) => return Err(e),
        };
        if !queue
            .drop_sent(batch)
            .map_err(|e| Error::Offline(e.to_string()))?
        {
            return Ok(accepted);
        }
    }
}

/// Wait before the next attempt after `failures` failed ones in a row.
pub fn backoff(failures: u32) -> Duration {
    Duration::from_secs((60u64 << failures.min(6)).min(3600))
}

/// `backoff` plus up to a quarter more, at random: after a Last.fm outage the
/// players come back spread out instead of all in the same second.
pub fn backoff_spread(failures: u32) -> Duration {
    spread(backoff(failures), random_u32())
}
/// `base` plus `random`'s share of a quarter of it.
pub fn spread(base: Duration, random: u32) -> Duration {
    base + base / 4 * random / u32::MAX
}
fn random_u32() -> u32 {
    let mut value = [0u8; 4];
    // SAFETY: getrandom writes at most the buffer's length into it.
    let n = unsafe { libc::getrandom(value.as_mut_ptr().cast(), value.len(), 0) };
    if n == 4 {
        u32::from_ne_bytes(value)
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    }
}

pub fn session_path(root: &Path) -> PathBuf {
    root.join("state/lastfm.json")
}

#[cfg(test)]
mod tests {
    use super::*;
    use reborn_core::scrobble::Play;

    fn dir(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("reborn-lastfm-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn md5_rfc_1321_vectors() {
        for (input, want) in [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            (
                "abcdefghijklmnopqrstuvwxyz",
                "c3fcd3d76192e4007dfb496cca67e13b",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ] {
            assert_eq!(hex(&md5(input.as_bytes())), want, "{input:?}");
        }
        // Multi-block with UTF-8.
        let long = "Björk – Jóga ".repeat(40);
        assert_eq!(hex(&md5(long.as_bytes())).len(), 32);
    }

    #[test]
    fn signature_sorts_and_excludes_format() {
        let params = vec![
            ("method".to_owned(), "auth.getSession".to_owned()),
            ("token".to_owned(), "t0k".to_owned()),
            ("api_key".to_owned(), "k3y".to_owned()),
            ("format".to_owned(), "json".to_owned()),
        ];
        let want = hex(&md5(b"api_keyk3ymethodauth.getSessiontokent0ksecret"));
        assert_eq!(signature(&params, "secret"), want);
        assert_eq!(
            form_encode("AC/DC & Björk ü"),
            "AC%2FDC+%26+Bj%C3%B6rk+%C3%BC"
        );
    }

    #[test]
    fn credentials_and_session_files() {
        let root = dir("files");
        let cred = root.join("lastfm-api.json");
        assert!(credentials(&cred).is_none());
        fs::write(&cred, r#"{"api_key":"short","shared_secret":"x"}"#).unwrap();
        assert!(credentials(&cred).is_none());
        let key = "0123456789abcdef0123456789abcdef";
        fs::write(
            &cred,
            format!(r#"{{"api_key":"{key}","shared_secret":"{key}"}}"#),
        )
        .unwrap();
        let c = credentials(&cred).unwrap();
        assert_eq!(format!("{c:?}"), "Credentials { .. }");
        let path = session_path(&root);
        let s = Session {
            name: "luca".into(),
            key: "sk".into(),
        };
        assert!(!format!("{s:?}").contains("sk\""));
        save_session(&path, &s).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(load_session(&path), Some(s));
        invalidate_session(&path).unwrap();
        assert_eq!(load_session(&path), None, "no key: not connected");
        assert_eq!(account_of(&path).as_deref(), Some("luca"));
        forget_session(&path).unwrap();
        forget_session(&path).unwrap();
        assert_eq!(load_session(&path), None);
        assert_eq!(account_of(&path), None);
        fs::remove_dir_all(root).unwrap();
    }

    /// A stand-in curl: records argv and stdin, answers from a file.
    fn fake_curl(root: &Path, reply: &str) -> PathBuf {
        let script = root.join("curl");
        fs::write(root.join("reply.json"), reply).unwrap();
        fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$@\" > {r}/argv\ncat > {r}/body\ncat {r}/reply.json\n",
                r = root.display()
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        script
    }

    fn client_with(curl: &Path) -> Client {
        Client {
            curl: curl.display().to_string(),
            ..Client::new(Credentials {
                api_key: "0123456789abcdef0123456789abcdef".into(),
                shared_secret: "fedcba9876543210fedcba9876543210".into(),
            })
        }
    }

    #[test]
    fn requests_carry_secrets_only_on_stdin_and_errors_classify() {
        let root = dir("http");
        let c = client_with(&fake_curl(
            &root,
            r#"{"session":{"name":"luca","key":"SESSIONKEY","subscriber":0}}"#,
        ));
        let s = c.mobile_session("luca", "pa ss&word").unwrap();
        assert_eq!(
            s,
            Session {
                name: "luca".into(),
                key: "SESSIONKEY".into()
            }
        );
        let argv = fs::read_to_string(root.join("argv")).unwrap();
        assert!(
            !argv.contains("pa") && !argv.contains("fedcba") && argv.contains("--data-binary @-")
        );
        assert!(argv.contains("--proto =https"));
        let body = fs::read_to_string(root.join("body")).unwrap();
        assert!(
            body.contains("password=pa+ss%26word") && body.contains("method=auth.getMobileSession")
        );
        assert!(body.contains("api_sig=") && body.ends_with("format=json"));
        for (reply, want) in [
            (
                r#"{"error":9,"message":"Invalid session key"}"#,
                Error::InvalidSession,
            ),
            (
                r#"{"error":14,"message":"not authorized"}"#,
                Error::NotApproved,
            ),
            (r#"{"error":29,"message":"Rate limit"}"#, Error::RateLimited),
            (r#"{"error":16,"message":"temp"}"#, Error::Unavailable),
            (r#"{"error":4,"message":"auth"}"#, Error::WrongLogin),
        ] {
            fake_curl(&root, reply);
            assert_eq!(c.session("tok"), Err(want));
        }
        fake_curl(&root, "<html>");
        assert!(matches!(c.token(), Err(Error::Offline(_))));
        assert!(Error::RateLimited.retry_later() && !Error::InvalidSession.retry_later());
        assert_eq!(
            c.approval_url("abc"),
            format!("{AUTH_PAGE}?api_key=0123456789abcdef0123456789abcdef&token=abc")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flush_sends_batches_drops_stale_and_stops_on_errors() {
        let root = dir("flush");
        let c = client_with(&fake_curl(
            &root,
            r#"{"scrobbles":{"@attr":{"accepted":50,"ignored":0}}}"#,
        ));
        let queue = crate::scrobble::Queue::new(root.join("q.jsonl"));
        let now = 1_800_000_000;
        let play = |t: i64| Scrobble {
            play: Play {
                artist: "A".into(),
                title: "T".into(),
                duration_s: 200,
                ..Default::default()
            },
            timestamp: t,
        };
        queue
            .push(&play(now - crate::scrobble::MAX_AGE_S - 10))
            .unwrap();
        for i in 0..60 {
            queue.push(&play(now - 1000 + i)).unwrap();
        }
        let session = Session {
            name: "n".into(),
            key: "k".into(),
        };
        assert_eq!(flush(&c, &session, &queue, now), Ok(100));
        assert!(queue.is_empty());
        let body = fs::read_to_string(root.join("body")).unwrap();
        assert!(body.contains("timestamp%5B9%5D=") && !body.contains("timestamp%5B10%5D="));
        // An error keeps everything that was not sent.
        for i in 0..3 {
            queue.push(&play(now - 100 + i)).unwrap();
        }
        fake_curl(&root, r#"{"error":11,"message":"offline"}"#);
        assert_eq!(flush(&c, &session, &queue, now), Err(Error::Unavailable));
        assert_eq!(queue.len(), 3);
        assert_eq!(backoff(0), Duration::from_secs(60));
        assert_eq!(backoff(10), Duration::from_secs(3600));
        assert_eq!(spread(Duration::from_secs(60), 0), Duration::from_secs(60));
        assert_eq!(
            spread(Duration::from_secs(60), u32::MAX),
            Duration::from_secs(75)
        );
        let spread_out: std::collections::HashSet<_> =
            (0..20).map(|_| backoff_spread(2).as_millis()).collect();
        assert!(
            spread_out.len() > 1,
            "retries are not all at the same moment"
        );
        assert!(spread_out
            .iter()
            .all(|&ms| (240_000..=300_000).contains(&ms)));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_invalid_play_is_left_out_instead_of_blocking_the_queue() {
        let root = dir("invalid");
        // Refuses any batch holding the play titled "Bad" (error 6).
        let script = root.join("curl");
        fs::write(
            &script,
            "#!/bin/sh\nbody=$(cat)\ncase \"$body\" in\n\
             *track%5B0%5D=Bad*|*track%5B1%5D=*) echo '{\"error\":6,\"message\":\"Invalid parameters\"}' ;;\n\
             *) echo '{\"scrobbles\":{\"@attr\":{\"accepted\":1,\"ignored\":0}}}' ;;\nesac\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let c = client_with(&script);
        let queue = crate::scrobble::Queue::new(root.join("q.jsonl"));
        let now = 1_800_000_000;
        for (i, title) in ["One", "Bad", "Two"].into_iter().enumerate() {
            queue
                .push(&Scrobble {
                    play: Play {
                        artist: "A".into(),
                        title: title.into(),
                        duration_s: 200,
                        ..Default::default()
                    },
                    timestamp: now - 100 + i as i64,
                })
                .unwrap();
        }
        let session = Session {
            name: "n".into(),
            key: "k".into(),
        };
        assert_eq!(flush(&c, &session, &queue, now), Ok(2));
        assert!(queue.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
}
