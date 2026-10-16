//! Scrobbling on the player: following plays (`reborn_core::scrobble`), the
//! queue and log on disk, and Last.fm (sign-in, Now Playing, sending).
//!
//! Nothing is recorded unless the listener chose it (a Last.fm account or the
//! scrobbler log). Network work runs in short worker threads that answer on a
//! channel; it never wakes the player and never keeps Wi-Fi up — plays wait
//! in the queue until the player is online anyway. No secret is ever logged.
use crate::Runtime;
use reborn_core::{
    scrobble::{play_of, Scrobble},
    LastFmRequest, PlaybackState,
};
use reborn_observability::Level;
use reborn_platform::{lastfm, scrobble as disk};
use reborn_ui::WifiStatus;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

/// How often a sign-in in progress asks whether it was approved, and for how
/// long (Last.fm's codes last an hour; the page gives up sooner).
const POLL_EVERY: Duration = Duration::from_secs(3);
const SIGN_IN_FOR: Duration = Duration::from_secs(15 * 60);
/// Plays counted before the clock was known, kept in memory at most.
const UNSTAMPED_MAX: usize = 500;

pub enum Reply {
    Token(Result<(String, String), lastfm::Error>),
    /// From the password sign-in (`polled` false) or the approval poll.
    Session(Result<lastfm::Session, lastfm::Error>, bool),
    Flushed(Result<u32, lastfm::Error>),
    NowPlaying(Result<(), lastfm::Error>),
}

pub struct LastFm {
    client: Option<lastfm::Client>,
    session: Option<lastfm::Session>,
    session_path: PathBuf,
    /// A sign-in in progress: its code, approval address and start.
    token: Option<(String, String, Instant)>,
    next_poll: Instant,
    polling: bool,
    /// A token request, password sign-in or batch is running.
    busy: bool,
    flushing: bool,
    next_flush: Instant,
    failures: u32,
    waiting: usize,
    /// The queue occurrence last announced as Now Playing.
    announced: Option<u64>,
    replies: (Sender<Reply>, Receiver<Reply>),
}
impl LastFm {
    pub fn load(root: &Path) -> Self {
        let client = lastfm::credentials(Path::new(lastfm::CREDENTIALS)).map(lastfm::Client::new);
        let session_path = lastfm::session_path(root);
        Self {
            session: lastfm::load_session(&session_path),
            client,
            session_path,
            token: None,
            next_poll: Instant::now(),
            polling: false,
            busy: false,
            flushing: false,
            next_flush: Instant::now(),
            failures: 0,
            waiting: disk::Queue::new(root.join("state/scrobbles.jsonl")).len(),
            announced: None,
            replies: channel(),
        }
    }
    /// An account to send to (and a key to send with).
    pub fn connected(&self) -> bool {
        self.client.is_some() && self.session.is_some()
    }
}

impl Runtime {
    fn online(&self) -> bool {
        matches!(self.ui.wifi.status, WifiStatus::Connected(_))
    }
    /// What Settings > Last.fm shows.
    pub(crate) fn lastfm_view(&mut self) {
        let l = &self.lastfm;
        let approval = l.token.as_ref().map(|(_, url, _)| url.clone());
        // The QR code is encoded once per sign-in, not on every render.
        let code = if approval == self.ui.lastfm.approval {
            self.ui.lastfm.code.clone()
        } else {
            approval
                .as_deref()
                .and_then(|url| reborn_ui::qr::encode(url.as_bytes()))
        };
        let view = reborn_ui::LastFmView {
            available: l.client.is_some(),
            user: l
                .session
                .as_ref()
                .filter(|_| l.client.is_some())
                .map(|s| s.name.clone()),
            waiting: l.waiting,
            approval,
            code,
            busy: l.busy || l.flushing,
        };
        if view != self.ui.lastfm {
            self.ui.lastfm = view;
            self.dirty.mark_render();
        }
    }
    /// The wall clock now, when Y2Linux says it can be trusted (checked at
    /// most every 30 s).
    fn trusted_unix(&mut self) -> Option<i64> {
        let fresh = self
            .clock_trusted
            .filter(|(at, _)| at.elapsed() < Duration::from_secs(30));
        let trusted = match fresh {
            Some((_, t)) => t,
            None => {
                let t = disk::clock_trusted(Path::new(disk::TIME_STATUS));
                self.clock_trusted = Some((Instant::now(), t));
                t
            }
        };
        trusted
            .then(|| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()
            })
            .flatten()
            .map(|d| d.as_secs() as i64)
    }
    fn current_entry(&self) -> Option<(u64, &reborn_core::Track)> {
        let position = self.model.queue_position;
        self.model
            .queue_entry_ids
            .get(position)
            .map(|e| e.0)
            .zip(self.model.queue.get(position))
    }
    /// Follow the play: once it counts, keep it for Last.fm and the log.
    pub(crate) fn scrobble_tick(&mut self) {
        let wanted = self.model.settings.scrobble_log
            || (self.model.settings.lastfm_scrobble && self.lastfm.connected());
        if !wanted {
            self.scrobbles = Default::default();
            self.scrobbles_unstamped.clear();
            return;
        }
        let now_ms = disk::boottime_ms();
        let unix = self.trusted_unix();
        let playing = self.model.playback == PlaybackState::Playing;
        // Borrow the queue entry in place: this runs every frame.
        let position = self.model.queue_position;
        let current = self
            .model
            .queue_entry_ids
            .get(position)
            .map(|e| e.0)
            .zip(self.model.queue.get(position));
        if let Some(counted) =
            self.scrobbles
                .observe(current, playing, self.model.position_ms, now_ms, unix)
        {
            if self.scrobbles_unstamped.len() >= UNSTAMPED_MAX {
                self.scrobbles_unstamped.remove(0);
            }
            self.scrobbles_unstamped.push(counted);
        }
        if self.scrobbles_unstamped.is_empty() || unix.is_none() {
            return;
        }
        for counted in std::mem::take(&mut self.scrobbles_unstamped) {
            if let Some(scrobble) = counted.stamp(now_ms, unix) {
                self.keep_scrobble(&scrobble);
            }
        }
    }
    fn keep_scrobble(&mut self, scrobble: &Scrobble) {
        if self.model.settings.scrobble_log {
            let client = format!("Y2Reborn {}", env!("CARGO_PKG_VERSION"));
            if let Err(e) =
                disk::append_log(&self.model.settings.music_directory, &client, scrobble)
            {
                self.log.emit(
                    Level::Warn,
                    "scrobble",
                    "log_failed",
                    "Scrobbler log not written",
                    None,
                    json!({"error": e.to_string()}),
                );
            }
        }
        if self.model.settings.lastfm_scrobble && self.lastfm.connected() {
            match self.scrobble_queue.push(scrobble) {
                Ok(()) => {
                    self.lastfm.waiting += 1;
                    // Online: send it now; otherwise it waits for Wi-Fi.
                    if self.lastfm.failures == 0 {
                        self.lastfm.next_flush = Instant::now();
                    }
                    self.lastfm_view();
                }
                Err(e) => self.log.emit(
                    Level::Warn,
                    "scrobble",
                    "queue_failed",
                    "Scrobble not queued",
                    None,
                    json!({"error": e.to_string()}),
                ),
            }
        }
        self.log.emit(
            Level::Info,
            "scrobble",
            "counted",
            "Play counted",
            None,
            json!({"duration_s": scrobble.play.duration_s}),
        );
    }
    /// Run one request in its own thread; `false` when no thread could
    /// start (the caller clears its busy flag, nothing stays stuck).
    fn lastfm_spawn(&self, name: &str, job: impl FnOnce() -> Reply + Send + 'static) -> bool {
        let reply = self.lastfm.replies.0.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let _ = reply.send(job());
            })
            .is_ok()
    }
    /// Settings > Last.fm.
    pub(crate) fn lastfm_request(&mut self, request: LastFmRequest) -> Result<(), String> {
        let result = match request {
            LastFmRequest::Connect | LastFmRequest::Login { .. }
                if self.lastfm.client.is_none() =>
            {
                Err("This build has no Last.fm key".to_owned())
            }
            LastFmRequest::Connect | LastFmRequest::Login { .. } | LastFmRequest::SendNow
                if !self.online() =>
            {
                Err("Connect to Wi-Fi first".to_owned())
            }
            LastFmRequest::Connect => {
                let client = self.lastfm.client.clone().ok_or("no key")?;
                self.lastfm.busy = self.lastfm_spawn("lastfm-token", move || {
                    Reply::Token(client.token().map(|t| {
                        let url = client.approval_url(&t);
                        (t, url)
                    }))
                });
                Ok(())
            }
            LastFmRequest::Login { username, password } => {
                let client = self.lastfm.client.clone().ok_or("no key")?;
                self.lastfm.busy = self.lastfm_spawn("lastfm-login", move || {
                    Reply::Session(client.mobile_session(&username, &password), false)
                });
                Ok(())
            }
            LastFmRequest::Cancel => {
                self.lastfm.token = None;
                Ok(())
            }
            LastFmRequest::SendNow if !self.lastfm.connected() => {
                Err("Connect to Last.fm first".to_owned())
            }
            LastFmRequest::SendNow => {
                self.lastfm.failures = 0;
                self.lastfm.next_flush = Instant::now();
                Ok(())
            }
            LastFmRequest::Disconnect => {
                self.lastfm_forget();
                self.ui.flash("Disconnected from Last.fm");
                Ok(())
            }
        };
        self.lastfm_view();
        result
    }
    /// Forget the account: its session and the plays waiting for it.
    pub(crate) fn lastfm_forget(&mut self) {
        if let Err(e) = lastfm::forget_session(&self.lastfm.session_path) {
            self.log.emit(
                Level::Warn,
                "lastfm",
                "forget_failed",
                "Last.fm session not removed",
                None,
                json!({"error": e.to_string()}),
            );
        }
        let _ = self.scrobble_queue.drop_first(usize::MAX);
        self.lastfm.session = None;
        self.lastfm.token = None;
        self.lastfm.waiting = 0;
        self.lastfm.failures = 0;
        self.lastfm.announced = None;
        self.log.emit(
            Level::Info,
            "lastfm",
            "disconnected",
            "Last.fm disconnected",
            None,
            json!({}),
        );
        self.lastfm_view();
    }
    /// Replies, the approval poll, sending and Now Playing.
    pub(crate) fn lastfm_tick(&mut self) {
        let replies: Vec<Reply> = self.lastfm.replies.1.try_iter().collect();
        for reply in replies {
            self.lastfm_reply(reply);
        }
        let now = Instant::now();
        // Sign-in: ask whether the code was approved.
        // (Fields read in place: this runs every frame while signing in.)
        if let Some(started) = self.lastfm.token.as_ref().map(|t| t.2) {
            if started.elapsed() > SIGN_IN_FOR {
                self.lastfm.token = None;
                self.ui.flash("The Last.fm sign-in timed out");
                self.lastfm_view();
            } else if !self.lastfm.polling && now >= self.lastfm.next_poll && self.online() {
                let token = self.lastfm.token.as_ref().map(|t| t.0.clone());
                if let (Some(client), Some(token)) = (self.lastfm.client.clone(), token) {
                    self.lastfm.polling = self.lastfm_spawn("lastfm-poll", move || {
                        Reply::Session(client.session(&token), true)
                    });
                    if !self.lastfm.polling {
                        self.lastfm.next_poll = now + POLL_EVERY;
                    }
                }
            }
        }
        if !self.lastfm.connected() || !self.online() {
            return;
        }
        // Now Playing: once per queue occurrence, when it actually plays.
        let playing = self.model.playback == PlaybackState::Playing;
        let announce = self.current_entry().and_then(|(id, track)| {
            (playing && self.lastfm.announced != Some(id)).then(|| (id, play_of(track)))
        });
        if let Some((id, play)) = announce {
            self.lastfm.announced = Some(id);
            if let (Some(play), true) = (play, self.model.settings.lastfm_now_playing) {
                let client = self.lastfm.client.clone();
                let session = self.lastfm.session.clone();
                if let (Some(client), Some(session)) = (client, session) {
                    let _ = self.lastfm_spawn("lastfm-now", move || {
                        Reply::NowPlaying(client.now_playing(&session, &play))
                    });
                }
            }
        }
        // Sending: whatever waits, with back-off after failures.
        if self.lastfm.waiting == 0
            || self.lastfm.flushing
            || now < self.lastfm.next_flush
            || !self.model.settings.lastfm_scrobble
        {
            return;
        }
        let Some(now_unix) = self.trusted_unix() else {
            return;
        };
        let (Some(client), Some(session)) =
            (self.lastfm.client.clone(), self.lastfm.session.clone())
        else {
            return;
        };
        let queue = self.scrobble_queue.clone();
        self.lastfm.flushing = self.lastfm_spawn("lastfm-send", move || {
            Reply::Flushed(lastfm::flush(&client, &session, &queue, now_unix))
        });
        if !self.lastfm.flushing {
            self.lastfm.next_flush = now + lastfm::backoff_spread(0);
        }
        self.lastfm_view();
    }
    fn lastfm_reply(&mut self, reply: Reply) {
        match reply {
            Reply::Token(result) => {
                self.lastfm.busy = false;
                match result {
                    Ok((token, url)) => {
                        self.lastfm.token = Some((token, url, Instant::now()));
                        self.lastfm.next_poll = Instant::now() + POLL_EVERY;
                    }
                    Err(e) => self.ui.flash(e.message()),
                }
            }
            Reply::Session(result, polled) => {
                if polled {
                    self.lastfm.polling = false;
                } else {
                    self.lastfm.busy = false;
                }
                match result {
                    Ok(session) => {
                        // Another account than the one the waiting plays
                        // belong to: they are not this listener's to send.
                        let owner = lastfm::account_of(&self.lastfm.session_path);
                        if owner.is_some_and(|o| o != session.name) {
                            let _ = self.scrobble_queue.drop_first(usize::MAX);
                            self.lastfm.waiting = 0;
                        }
                        if let Err(e) = lastfm::save_session(&self.lastfm.session_path, &session) {
                            self.ui
                                .flash(format!("Couldn't save the Last.fm sign-in: {e}"));
                            return self.lastfm_view();
                        }
                        self.ui
                            .flash(format!("Scrobbling to Last.fm as {}", session.name));
                        self.log.emit(
                            Level::Info,
                            "lastfm",
                            "connected",
                            "Last.fm connected",
                            None,
                            json!({"method": if polled { "approval" } else { "password" }}),
                        );
                        self.lastfm.session = Some(session);
                        self.lastfm.token = None;
                        self.lastfm.failures = 0;
                        self.lastfm.announced = None;
                    }
                    Err(lastfm::Error::NotApproved) if polled => {
                        self.lastfm.next_poll = Instant::now() + POLL_EVERY;
                    }
                    Err(e) if polled && e.retry_later() => {
                        self.lastfm.next_poll = Instant::now() + POLL_EVERY * 3;
                    }
                    Err(e) => {
                        if polled {
                            self.lastfm.token = None;
                        }
                        self.ui.flash(e.message());
                    }
                }
            }
            Reply::Flushed(result) => {
                self.lastfm.flushing = false;
                self.lastfm.waiting = self.scrobble_queue.len();
                match result {
                    Ok(accepted) => {
                        self.lastfm.failures = 0;
                        self.log.emit(
                            Level::Info,
                            "lastfm",
                            "sent",
                            "Plays sent to Last.fm",
                            None,
                            json!({"accepted": accepted, "waiting": self.lastfm.waiting}),
                        );
                    }
                    Err(lastfm::Error::InvalidSession) => {
                        // Revoked on last.fm: stop sending, keep the plays
                        // for this account's next sign-in.
                        let _ = lastfm::invalidate_session(&self.lastfm.session_path);
                        self.lastfm.session = None;
                        self.ui.flash("Sign in to Last.fm again");
                        self.log.emit(
                            Level::Warn,
                            "lastfm",
                            "session_invalid",
                            "Last.fm session no longer valid",
                            None,
                            json!({}),
                        );
                    }
                    Err(e) => {
                        self.lastfm.failures += 1;
                        self.lastfm.next_flush =
                            Instant::now() + lastfm::backoff_spread(self.lastfm.failures - 1);
                        self.log.emit(
                            Level::Warn,
                            "lastfm",
                            "send_failed",
                            "Plays not sent",
                            None,
                            json!({"error": e.message(), "failures": self.lastfm.failures}),
                        );
                    }
                }
            }
            Reply::NowPlaying(result) => {
                if let Err(lastfm::Error::InvalidSession) = result {
                    let _ = lastfm::invalidate_session(&self.lastfm.session_path);
                    self.lastfm.session = None;
                    self.ui.flash("Sign in to Last.fm again");
                }
            }
        }
        self.lastfm_view();
    }
}
