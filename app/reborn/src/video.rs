//! A film's pictures on Now Playing. The film's sound is an ordinary track to the player (queue,
//! transport, outputs); this module runs a second reader of the same file that decodes pictures
//! in step with the position the player reports, and hands the main loop one small RGBA image at
//! a time to put in the renderer's film texture.
//!
//! The reader only runs while the picture can be seen: Now Playing in front, screen on. Leaving
//! the screen stops it (the hardware decoder powers down); coming back starts it at the position
//! the sound has reached.
use crate::Runtime;
use reborn_core::{PlaybackState, Screen};
use reborn_media::Video;
use reborn_observability::{Level, Observer};
use reborn_platform::storage::{pin_track, PinnedTrack};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::Path,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// The picture is fitted to the screen.
const SCREEN: (u32, u32) = (480, 360);
/// A picture this much behind the sound is dropped, not shown.
const LATE_MS: i64 = 120;
/// Further from the sound than this (after a seek): continue from the nearest key picture.
const FAR_MS: i64 = 1500;
/// The reported position moves the reader's clock at once when it is this far off, slowly else.
const JUMP_MS: i64 = 300;

/// One picture ready for the screen.
pub(crate) struct Frame {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub subtitle: String,
}

#[derive(Default)]
struct Shared {
    /// The sound's position as last reported, when, and whether it is moving.
    position_ms: i64,
    reported: Option<Instant>,
    playing: bool,
    /// Counts the times the sound was moved (a seek, a pause or a play).
    moves: u64,
    /// While it played without a move: how far the reported position went, and in how much real
    /// time (Y2Linux #145: a Bluetooth delay that grows shows as sound slower than the clock).
    sound_ms: i64,
    /// Kept whole, not in milliseconds: a main-loop step is ~17 ms, and its cut-off fraction lost
    /// ~4 % of the clock (test build 279).
    wall: Duration,
    /// The position as last reported, unsmoothed.
    reported_ms: i64,
    stop: bool,
    /// The film could not be opened.
    failed: bool,
    frame: Option<Frame>,
    /// A picture buffer the main loop is done with, to be filled again.
    spare: Option<Vec<u8>>,
}

impl Shared {
    /// The player reported the sound at `reported` ms at `now`; `true` when it was moved (a seek, a
    /// pause or a play) rather than played on.
    fn report(&mut self, reported: i64, playing: bool, now: Instant) -> bool {
        let drift = reported - self.clock_at(now);
        let moved = self.playing != playing || drift.abs() > JUMP_MS;
        // Every step of continuous play counts, a stalled or hurried position too (they are what
        // this measures); only a seek or another film (a jump past FAR_MS) and a pause do not.
        if let (true, Some(at)) = (self.playing && playing, self.reported) {
            let wall = now.saturating_duration_since(at);
            let sound = reported - self.reported_ms;
            if (sound - wall.as_millis() as i64).abs() <= FAR_MS {
                self.wall += wall;
                self.sound_ms += sound;
            }
        }
        self.reported_ms = reported;
        // Small differences are the position's coarse steps: lean towards it, do not jump.
        self.position_ms = if moved {
            reported
        } else {
            self.clock_at(now) + drift / 8
        };
        self.reported = Some(now);
        self.playing = playing;
        self.moves += u64::from(moved);
        moved
    }

    /// Where the sound is now.
    fn clock(&self) -> i64 {
        self.clock_at(Instant::now())
    }

    fn clock_at(&self, now: Instant) -> i64 {
        match (self.playing, self.reported) {
            (true, Some(at)) => {
                self.position_ms + now.saturating_duration_since(at).as_millis() as i64
            }
            _ => self.position_ms,
        }
    }
}

struct Link {
    shared: Mutex<Shared>,
    changed: Condvar,
}

impl Link {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// How far the sound is ahead of a picture. A picture without a time (`pts_ms` below 0: the
/// container gave none) is never late: measured against the clock it would look minutes behind,
/// be dropped, and send the reader seeking again and again.
fn lateness(clock_ms: i64, pts_ms: i64) -> i64 {
    if pts_ms < 0 {
        0
    } else {
        clock_ms - pts_ms
    }
}

/// Whether the reader seeks to the sound: it is further away than `FAR_MS` and the reader is not
/// catching up from a seek, or the sound was moved again since that seek.
fn follow_move(late_ms: i64, catching_up: bool, moved_since: bool) -> bool {
    late_ms.abs() > FAR_MS && (!catching_up || moved_since)
}

/// The reader thread of one film.
struct Reader {
    video: Video,
    link: Arc<Link>,
    log: Observer,
    /// After a seek: decode without showing until the sound's position is reached.
    catching_up: bool,
    /// `Shared::moves` when the reader last seeked: a move after it is a new seek to follow.
    seeked_for: u64,
    shown: u64,
    dropped: u64,
    /// How late the shown pictures were behind the sound: the most and the sum.
    late_max_ms: i64,
    late_sum_ms: i64,
    hardware: bool,
    /// Time spent decoding and converting pictures, for the log.
    decode: Duration,
    convert: Duration,
}

impl Reader {
    /// Open the film and read it until told to stop. Opening reads the file's headers and wakes
    /// the decoder, so it happens here, not in the main loop.
    fn open_and_run(pin: PinnedTrack, stem: String, link: Arc<Link>, log: Observer) {
        match Video::open(pin.path(), SCREEN.0, SCREEN.1, true) {
            Ok(mut video) => {
                // Subtitles beside the film ("Film.srt" for "Film.mkv"), when it has none inside.
                for ending in ["srt", "ass", "ssa", "vtt"] {
                    if video.info.subtitles {
                        break;
                    }
                    if let Ok(file) = pin.open_sidecar(&format!("{stem}.{ending}")) {
                        video.subtitles_from(file.path());
                    }
                }
                let reader = Reader {
                    video,
                    link,
                    log,
                    catching_up: false,
                    seeked_for: 0,
                    shown: 0,
                    dropped: 0,
                    late_max_ms: 0,
                    late_sum_ms: 0,
                    hardware: false,
                    decode: Duration::ZERO,
                    convert: Duration::ZERO,
                };
                reader.run();
            }
            Err(e) => {
                log.emit(Level::Warn, "video", "open_failed", &e, None, json!({}));
                link.lock().failed = true;
            }
        }
        // The file stays the chosen one while it is read.
        drop(pin);
    }

    fn run(mut self) {
        let started = Instant::now();
        let mut result = Ok(());
        // A film opened in its middle starts at the key picture before the position.
        let (start, moves) = {
            let s = self.link.lock();
            (s.clock(), s.moves)
        };
        if start > FAR_MS {
            result = self.video.seek(start as u64);
            self.catching_up = true;
            self.seeked_for = moves;
        }
        while result.is_ok() {
            match self.picture() {
                Ok(true) => {}
                Ok(false) => break,
                Err(e) => result = Err(e),
            }
        }
        let (sound_ms, wall_ms) = {
            let s = self.link.lock();
            (s.sound_ms, s.wall.as_millis() as u64)
        };
        let late_avg_ms = self.late_sum_ms / self.shown.max(1) as i64;
        let fields = json!({
            "shown": self.shown, "dropped": self.dropped, "hardware": self.hardware,
            "late_max_ms": self.late_max_ms, "late_avg_ms": late_avg_ms,
            "sound_ms": sound_ms, "wall_ms": wall_ms,
            "codec": self.video.info.codec, "width": self.video.info.width,
            "height": self.video.info.height, "seconds": started.elapsed().as_secs(),
            "decode_ms": self.decode.as_millis() as u64,
            "convert_ms": self.convert.as_millis() as u64,
        });
        match result {
            Ok(()) => self
                .log
                .emit(Level::Info, "video", "reader_stopped", "", None, fields),
            Err(e) => self
                .log
                .emit(Level::Warn, "video", "reader_failed", &e, None, fields),
        }
    }

    /// Decode one picture and show, hold or drop it. `false`: the reader was told to stop.
    fn picture(&mut self) -> Result<bool, String> {
        let decoding = Instant::now();
        let next = self.video.next_picture()?;
        self.decode += decoding.elapsed();
        let Some(picture) = next else {
            return self.at_the_end();
        };
        self.hardware |= picture.hardware;
        let pts = picture.pts_ms;
        let shown_late = loop {
            let (stop, clock, playing, moves) = {
                let s = self.link.lock();
                (s.stop, s.clock(), s.playing, s.moves)
            };
            if stop {
                return Ok(false);
            }
            let late = lateness(clock, pts);
            // Catching up, the key picture may lie far before the sound; a move since that seek
            // (Y2Reborn #129: back again while catching up) is followed, or the picture held still
            // until the sound came back to it.
            if follow_move(late, self.catching_up, moves != self.seeked_for) {
                // The sound was moved: go on from the key picture before it.
                self.video.seek(clock.max(0) as u64)?;
                self.catching_up = true;
                self.seeked_for = moves;
                return Ok(true);
            }
            if late > LATE_MS && (playing || self.catching_up) {
                self.video.hurry(true);
                self.dropped += u64::from(!self.catching_up);
                return Ok(true);
            }
            if late >= 0 {
                break late;
            }
            // Early: hold it until the sound is there (or the position changes).
            let wait = Duration::from_millis((-late).min(50) as u64);
            let s = self.link.lock();
            drop(self.link.changed.wait_timeout(s, wait));
        };
        self.catching_up = false;
        self.video.hurry(false);
        if pts >= 0 {
            self.late_max_ms = self.late_max_ms.max(shown_late);
            self.late_sum_ms += shown_late;
        }
        self.show(pts)?;
        Ok(true)
    }

    fn show(&mut self, pts: i64) -> Result<(), String> {
        let info = &self.video.info;
        let (width, height) = (info.out_width, info.out_height);
        let size = width as usize * height as usize * 4;
        let mut rgba = self.link.lock().spare.take().unwrap_or_default();
        rgba.resize(size, 0);
        let converting = Instant::now();
        self.video.convert(&mut rgba)?;
        self.convert += converting.elapsed();
        let subtitle = self.video.subtitle(pts);
        self.link.lock().frame = Some(Frame {
            rgba,
            width,
            height,
            subtitle,
        });
        self.shown += 1;
        Ok(())
    }

    /// The film is over: wait for the stop, or for the sound to be moved back into it.
    fn at_the_end(&mut self) -> Result<bool, String> {
        let mut s = self.link.lock();
        let end = s.clock();
        while !s.stop && s.clock() > end - FAR_MS {
            s = self
                .link
                .changed
                .wait_timeout(s, Duration::from_millis(200))
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if s.stop {
            return Ok(false);
        }
        let back = s.clock().max(0) as u64;
        self.seeked_for = s.moves;
        drop(s);
        self.video.seek(back)?;
        self.catching_up = true;
        Ok(true)
    }
}

/// Where each film was left, by its path: `state/videos.json`. A film starts there again.
#[derive(Default)]
struct Shelf {
    places: BTreeMap<String, u64>,
    loaded: bool,
    /// The film playing now: its path, length and last position.
    playing: Option<(String, u64, u64)>,
}

impl Shelf {
    /// A film is worth resuming once it ran ten seconds and while fifteen are left.
    fn keep(position_ms: u64, length_ms: u64) -> bool {
        position_ms >= 10_000 && position_ms + 15_000 < length_ms
    }

    fn load(&mut self, file: &Path) {
        if !std::mem::replace(&mut self.loaded, true) {
            self.places = std::fs::read(file)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .unwrap_or_default();
        }
    }

    /// Note where `film` (path, length, position) is, or that no film plays. Returns the place
    /// a film that starts now was left at, and writes the file when a film was left.
    fn follow(&mut self, film: Option<(String, u64, u64)>, file: &Path) -> Option<u64> {
        self.load(file);
        let same = matches!((&self.playing, &film), (Some(a), Some(b)) if a.0 == b.0);
        if same {
            self.playing = film;
            return None;
        }
        if let Some((path, length, position)) = self.playing.take() {
            if Self::keep(position, length) {
                self.places.insert(path, position);
            } else {
                self.places.remove(&path);
            }
            if let Ok(bytes) = serde_json::to_vec(&self.places) {
                let _ = reborn_core::atomic_write(file, &bytes);
            }
        }
        let resume = film.as_ref().and_then(|(path, length, position)| {
            let left = *self.places.get(path)?;
            (*position < 2_000 && Self::keep(left, *length)).then_some(left)
        });
        self.playing = film;
        resume
    }
}

/// The film being shown, if any.
#[derive(Default)]
pub(crate) struct Player {
    running: Option<(i64, Arc<Link>, JoinHandle<()>)>,
    /// The film that failed to open: not tried again until another one plays.
    failed: Option<i64>,
    /// The buffer of the picture shown before, handed back to the reader.
    spare: Option<Vec<u8>>,
    shelf: Shelf,
}

impl Player {
    /// Whether the film `id` still has to be started (it neither runs nor failed to open).
    fn wants(&self, id: i64) -> bool {
        !self
            .running
            .as_ref()
            .is_some_and(|(running, ..)| *running == id)
            && self.failed != Some(id)
    }

    /// Show the film `id` (its file name without the ending is `stem`) from `position_ms`.
    fn start(&mut self, id: i64, pin: PinnedTrack, stem: String, position_ms: u64, log: &Observer) {
        self.stop();
        self.failed = None;
        let link = Arc::new(Link {
            shared: Mutex::new(Shared {
                position_ms: position_ms as i64,
                ..Shared::default()
            }),
            changed: Condvar::new(),
        });
        let (reader_link, reader_log) = (link.clone(), log.clone());
        let spawned = std::thread::Builder::new()
            .name("video".into())
            .spawn(move || Reader::open_and_run(pin, stem, reader_link, reader_log));
        match spawned {
            Ok(thread) => self.running = Some((id, link, thread)),
            Err(e) => {
                let error = e.to_string();
                log.emit(
                    Level::Warn,
                    "video",
                    "thread_failed",
                    &error,
                    None,
                    json!({}),
                );
                self.failed = Some(id);
            }
        }
    }

    /// Stop showing; `true` when a film was being shown.
    fn stop(&mut self) -> bool {
        let Some((_, link, thread)) = self.running.take() else {
            return false;
        };
        link.lock().stop = true;
        link.changed.notify_all();
        let _ = thread.join();
        true
    }

    /// Tell the reader where the sound is.
    fn sync(&self, position_ms: u64, playing: bool) {
        let Some((_, link, _)) = &self.running else {
            return;
        };
        let moved = link
            .lock()
            .report(position_ms as i64, playing, Instant::now());
        if moved {
            link.changed.notify_all();
        }
    }

    /// The newest picture since the last call, and the buffer of the one before to fill again.
    fn take(&mut self, spare: Option<Vec<u8>>) -> Option<Frame> {
        let (id, link, _) = self.running.as_ref()?;
        let mut s = link.lock();
        if s.failed {
            // Not tried again until another film plays.
            let id = *id;
            drop(s);
            self.stop();
            self.failed = Some(id);
            return None;
        }
        if spare.is_some() && s.spare.is_none() {
            s.spare = spare;
        }
        s.frame.take()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop();
    }
}

thread_local! {
    /// The main loop's one film player. It lives beside the runtime, not in it: the runtime is
    /// built in one long function that may not grow, and nothing else needs the player.
    static PLAYER: std::cell::RefCell<Player> = std::cell::RefCell::default();
}

impl Runtime {
    /// What the main loop owes every pass besides its own steps: audiobook chapters, the sleep
    /// timer and the film's picture.
    pub(crate) fn background_ticks(&mut self) {
        self.receive_chapters();
        self.sleep_timer_tick();
        self.buttons_trial_tick();
        self.video_tick();
    }

    fn video_tick(&mut self) {
        PLAYER.with_borrow_mut(|player| self.show_film(player));
    }

    /// Remember where the playing film is; a film that starts goes on where it was left.
    fn resume_film(&mut self, player: &mut Player) {
        let film = self.model.current().filter(|t| t.is_video()).map(|t| {
            let path = t.path.to_string_lossy().into_owned();
            (path, t.duration_ms, self.model.position_ms)
        });
        let file = self.root.join("state/videos.json");
        if let Some(left) = player.shelf.follow(film, &file) {
            self.seek_by(left as i64 - self.model.position_ms as i64);
        }
    }

    fn show_film(&mut self, player: &mut Player) {
        self.resume_film(player);
        let visible = self.model.screen == Screen::NowPlaying
            && !self.model.screen_off
            && self.graphics.is_some();
        let film = self
            .model
            .current()
            .filter(|t| visible && t.online && t.is_video())
            .map(|t| t.id);
        let Some(id) = film else {
            if player.stop() || self.ui.video.width > 0 {
                if let Some(renderer) = self.graphics.as_mut() {
                    renderer.forget_video();
                }
                self.ui.video = Default::default();
                self.ui.film_running = false;
                player.spare = None;
                self.dirty.mark_render();
            }
            return;
        };
        if player.wants(id) {
            let film = self.model.current().map(|t| {
                let stem = t.path.file_stem().unwrap_or_default().to_string_lossy();
                (
                    pin_track(t, &self.model.settings.music_directory),
                    stem.into_owned(),
                )
            });
            match film {
                Some((Ok(pin), stem)) => {
                    player.start(id, pin, stem, self.model.position_ms, &self.log)
                }
                _ => player.failed = Some(id),
            }
        }
        let playing = self.model.playback == PlaybackState::Playing;
        player.sync(self.model.position_ms, playing);
        let spare = player.spare.take();
        if let Some(frame) = player.take(spare) {
            if let Some(renderer) = self.graphics.as_mut() {
                if renderer
                    .video(&frame.rgba, frame.width, frame.height)
                    .is_ok()
                {
                    self.ui.video = reborn_ui::views_videos::VideoPicture {
                        width: frame.width,
                        height: frame.height,
                        subtitle: frame.subtitle,
                    };
                    self.dirty.mark_render();
                }
            }
            player.spare = Some(frame.rgba);
        }
        self.ui.film_running = playing && self.ui.video.width > 0;
        if playing {
            // A film keeps the screen on, as a hand on the wheel would.
            self.last_activity = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_runs_with_the_sound_and_stands_with_it() {
        let mut s = Shared {
            position_ms: 5000,
            ..Shared::default()
        };
        assert_eq!(s.clock(), 5000, "paused: where the sound stands");
        s.playing = true;
        s.reported = Some(Instant::now() - Duration::from_millis(250));
        assert!((5240..5400).contains(&s.clock()), "{}", s.clock());
    }

    #[test]
    fn sound_slower_than_the_clock_is_counted_and_a_seek_is_not() {
        let start = Instant::now();
        let mut s = Shared::default();
        assert!(s.report(0, true, start), "play: a move");
        // Ten seconds of sound reported over ten and a half seconds: a delay that grew.
        for step in 1..=10 {
            let at = start + Duration::from_millis(step * 1050);
            assert!(!s.report(step as i64 * 1000, true, at));
        }
        assert_eq!((s.sound_ms, s.wall.as_millis() as i64), (10_000, 10_500));
        // A seek far ahead is a move: neither counts.
        assert!(s.report(60_000, true, start + Duration::from_millis(11_000)));
        assert_eq!((s.sound_ms, s.wall.as_millis() as i64), (10_000, 10_500));
        // The position stands still 400 ms, then catches up: sound and clock stay equal.
        let at = |ms: u64| start + Duration::from_millis(11_000 + ms);
        s.report(60_000, true, at(400));
        s.report(60_600, true, at(600));
        assert_eq!((s.sound_ms, s.wall.as_millis() as i64), (10_600, 11_100));
    }

    #[test]
    fn many_short_main_loop_steps_lose_no_clock() {
        // 600 steps of 16.667 ms with the sound right on time: ten seconds each.
        let start = Instant::now();
        let mut s = Shared::default();
        s.report(0, true, start);
        for step in 1..=600u64 {
            let at = start + Duration::from_micros(step * 16_667);
            s.report((step * 16_667 / 1000) as i64, true, at);
        }
        assert_eq!(s.sound_ms, 10_000);
        assert_eq!(s.wall.as_millis(), 10_000, "test build 279 lost ~4 % here");
    }

    #[test]
    fn a_picture_without_a_time_is_never_late() {
        assert_eq!(lateness(61_000, 60_500), 500, "known: the sound is ahead");
        assert_eq!(lateness(60_000, 60_400), -400, "known: early");
        // The container gave no time (pts -1) minutes into the film.
        let late = lateness(61_000, -1);
        assert!(late <= LATE_MS, "not dropped as late: {late}");
        assert!(!follow_move(late, false, false), "no seek to the sound");
        assert!(!follow_move(late, true, true), "none while catching up");
    }

    #[test]
    fn a_seek_while_catching_up_is_followed() {
        assert!(follow_move(-60_000, false, false), "moved back: seek");
        assert!(
            !follow_move(8_000, true, false),
            "catching up from a key picture 8 s before the sound: decode on"
        );
        // Y2Reborn #129: forward, then back while still catching up; the picture stood still.
        assert!(follow_move(-60_000, true, true), "moved again: seek again");
        assert!(
            !follow_move(-60_000, true, false),
            "no new move: no seek loop"
        );
        assert!(!follow_move(400, false, true), "near the sound: no seek");
    }

    #[test]
    fn a_film_goes_on_where_it_was_left() {
        let dir = std::env::temp_dir().join(format!("reborn-videos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("videos.json");
        let film = |position| Some(("/m/Film.mkv".to_owned(), 600_000, position));
        let mut shelf = Shelf::default();
        assert_eq!(
            shelf.follow(film(0), &file),
            None,
            "a new film starts at its start"
        );
        assert_eq!(shelf.follow(film(125_000), &file), None);
        assert_eq!(
            shelf.follow(None, &file),
            None,
            "stopped: its place is written"
        );
        // Another start of the player reads the file.
        let mut shelf = Shelf::default();
        assert_eq!(shelf.follow(film(300), &file), Some(125_000));
        // Watched to the end: it starts over next time.
        shelf.follow(film(595_000), &file);
        shelf.follow(None, &file);
        assert_eq!(shelf.follow(film(0), &file), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_player_without_a_film_has_nothing_to_show_or_stop() {
        let mut player = Player::default();
        assert!(player.take(None).is_none());
        assert!(!player.stop());
        player.sync(1000, true);
    }
}
