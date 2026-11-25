//! The sleep timer: playback pauses after the chosen minutes (the volume
//! fading out over the last seconds) or when the chapter or song playing ends.
//! It lives only in memory: a restart ends it.
use crate::Runtime;
use reborn_core::{PlaybackState, SleepChoice, SleepItem, SleepTimer};
use std::time::{Duration, Instant};

/// The last stretch of a timed sleep fades the volume out.
const FADE: Duration = Duration::from_secs(10);

/// The volume `left` before a timed pause: full until the fade, then less.
pub fn faded(volume: u8, left: Duration) -> u8 {
    if left >= FADE {
        return volume;
    }
    (f32::from(volume) * left.as_secs_f32() / FADE.as_secs_f32()).round() as u8
}

/// Whether moving from `before` to `now` played out the queue: the entry that ended was the
/// last one (a chapter change inside it is not its end).
fn ended_the_queue(before: SleepItem, now: SleepItem, queue: &[reborn_core::QueueEntryId]) -> bool {
    before.0.is_some() && before.0 != now.0 && before.0.as_ref() == queue.last()
}

impl Runtime {
    pub(crate) fn set_sleep_timer(&mut self, choice: SleepChoice) {
        self.end_sleep_fade();
        let item = self.model.sleep_item();
        self.model.sleep_timer = match choice {
            SleepChoice::Off => None,
            SleepChoice::Minutes(m) => Some(SleepTimer {
                choice,
                until: Some(Instant::now() + Duration::from_secs(u64::from(m) * 60)),
                item,
            }),
            SleepChoice::EndOfItem | SleepChoice::EndOfQueue => Some(SleepTimer {
                choice,
                until: None,
                item,
            }),
        };
        self.dirty.mark_render();
    }

    /// The listener moved to another song or chapter: an "end of song" timer
    /// follows it while they listen and ends when they start afresh.
    pub(crate) fn sleep_timer_follows_listener(&mut self, before: SleepItem, was_playing: bool) {
        let Some(timer) = self.model.sleep_timer else {
            return;
        };
        let now = self.model.sleep_item();
        self.model.sleep_timer = timer.after_listener_moved(before, now, was_playing);
        if self.model.sleep_timer.is_none() {
            self.ui.flash("Sleep timer off");
        }
        if self.model.sleep_timer != Some(timer) {
            self.dirty.mark_render();
        }
    }

    /// The software level again after a fade (full while headphones hold the volume).
    fn end_sleep_fade(&mut self) {
        if std::mem::take(&mut self.sleep_fading) {
            self.playback.set_volume(self.sink_volume());
        }
    }

    /// Whether a timed sleep fades now or within a second: the fade steps with every pass of the
    /// main loop, so dark playback keeps its quick passes then.
    pub(crate) fn sleep_fade_near(&self) -> bool {
        self.model
            .sleep_timer
            .and_then(|timer| timer.until)
            .is_some_and(|until| {
                until.saturating_duration_since(Instant::now()) < FADE + Duration::from_secs(1)
            })
    }

    /// Every pass of the main loop: fade, pause when due.
    pub(crate) fn sleep_timer_tick(&mut self) {
        let Some(timer) = self.model.sleep_timer else {
            self.sleep_minutes_shown = None;
            return;
        };
        // Now Playing shows the minutes left: draw again when they change,
        // also while paused (#29).
        let minutes = timer.until.map(|until| {
            until
                .saturating_duration_since(Instant::now())
                .as_secs()
                .div_ceil(60)
        });
        if minutes != self.sleep_minutes_shown {
            self.sleep_minutes_shown = minutes;
            self.dirty.mark_render();
        }
        let playing = matches!(
            self.model.playback,
            PlaybackState::Playing | PlaybackState::Buffering
        );
        if let Some(until) = timer.until {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                self.model.sleep_timer = None;
                if playing {
                    self.pause();
                    self.ui.flash("Sleep timer paused playback");
                }
                self.end_sleep_fade();
                self.dirty.mark_both();
            } else if playing && left < FADE {
                self.playback.set_volume(faded(self.sink_volume(), left));
                self.sleep_fading = true;
            }
            return;
        }
        if self.model.playback == PlaybackState::Stopped {
            self.model.sleep_timer = None;
            self.dirty.mark_render();
            return;
        }
        if !playing || self.model.sleep_item() == timer.item {
            return;
        }
        // End of Queue: only the last entry's end pauses; before it the timer moves on.
        if timer.choice == SleepChoice::EndOfQueue
            && !ended_the_queue(
                timer.item,
                self.model.sleep_item(),
                &self.model.queue_entry_ids,
            )
        {
            self.model.sleep_timer = Some(SleepTimer {
                item: self.model.sleep_item(),
                ..timer
            });
            return;
        }
        // The chapter or song ended: pause, ready to start the next one from
        // its beginning.
        self.model.sleep_timer = None;
        let start = self
            .model
            .book_part()
            .and_then(|(book, part)| {
                let at = book.chapter_at(part, self.model.position_ms)?;
                book.chapter_span(at).map(|(start, _)| start)
            })
            .unwrap_or(0);
        self.pause();
        self.release_hold();
        self.model.position_ms = start;
        self.ui.flash("Sleep timer paused playback");
        self.dirty.mark_both();
        self.checkpoint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_last_entry_ending_ends_the_queue() {
        use reborn_core::QueueEntryId as E;
        let queue = [E(1), E(2), E(3)];
        let at = |id| (Some(E(id)), None);
        assert!(
            !ended_the_queue(at(1), at(2), &queue),
            "the next song: go on"
        );
        assert!(
            ended_the_queue(at(3), at(1), &queue),
            "Repeat All wrapped: pause"
        );
        assert!(
            !ended_the_queue((Some(E(3)), Some(1)), (Some(E(3)), Some(2)), &queue),
            "a chapter inside the last part is not its end"
        );
    }

    #[test]
    fn a_timed_sleep_fades_over_its_last_ten_seconds() {
        assert_eq!(faded(60, Duration::from_secs(60)), 60);
        assert_eq!(faded(60, Duration::from_secs(10)), 60);
        assert_eq!(faded(60, Duration::from_secs(5)), 30);
        assert_eq!(faded(60, Duration::from_millis(500)), 3);
        assert_eq!(faded(60, Duration::ZERO), 0);
    }
}
