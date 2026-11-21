//! Workload hints only. The platform's expiring QoS leases own CPU policy.
#![forbid(unsafe_code)]
use std::{
    fs::{File, OpenOptions},
    io::Write,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Idle,
    PlaybackNormal,
    PlaybackHeavy,
    Interactive,
    ArtworkDecode,
    LibraryScan,
    NetworkTransfer,
    Maintenance,
}
impl Class {
    fn name(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::PlaybackNormal => "PlaybackNormal",
            Self::PlaybackHeavy => "PlaybackHeavy",
            Self::Interactive => "Interactive",
            Self::ArtworkDecode => "ArtworkDecode",
            Self::LibraryScan => "LibraryScan",
            Self::NetworkTransfer => "NetworkTransfer",
            Self::Maintenance => "Maintenance",
        }
    }
    fn request(self) -> String {
        format!("{} {}\n", self.name(), self.lease_ms())
    }
    fn lease_ms(self) -> u32 {
        if self == Self::Interactive {
            250
        } else {
            3000
        }
    }
}
// A character-device write is a parser transaction. Formatting directly into
// File may issue one write per format fragment; construct the full request first.
fn write_request(writer: &mut impl Write, class: Class) -> std::io::Result<()> {
    writer.write_all(class.request().as_bytes())
}
#[derive(Default)]
pub struct Hints {
    file: Option<File>,
    last: Option<(Class, Instant)>,
    retry: Option<Instant>,
}
impl Hints {
    pub fn publish(&mut self, class: Class) {
        if self.last.is_some_and(|(old, when)| {
            old == class
                && when.elapsed()
                    < if class == Class::Interactive {
                        Duration::from_millis(50)
                    } else {
                        Duration::from_secs(1)
                    }
        }) {
            return;
        }
        if self.file.is_none() {
            if self
                .retry
                .is_some_and(|when| when.elapsed() < Duration::from_secs(10))
            {
                return;
            }
            self.retry = Some(Instant::now());
            self.file = OpenOptions::new().write(true).open("/dev/y2-workload").ok();
        }
        if let Some(file) = &mut self.file {
            if write_request(file, class).is_ok() {
                self.last = Some((class, Instant::now()));
                self.retry = None;
            } else {
                self.file = None;
                self.last = None;
            }
        }
    }
    pub fn clear(&mut self) {
        self.file = None;
        self.last = None;
        self.retry = None;
    }
    pub fn interactive(&mut self) {
        // Each input gets a short lease; no continuous fixed-MHz selection.
        self.publish(Class::Interactive);
    }
}
/// Independent scoped hint. Dropping its descriptor releases kernel QoS;
/// a wedged application also loses the hint after the bounded lease expiry.
pub struct CpuWorkloadLease {
    hints: Hints,
    class: Class,
}
impl CpuWorkloadLease {
    pub fn acquire_workload_hint(class: Class) -> Self {
        let mut hints = Hints::default();
        hints.publish(class);
        Self { hints, class }
    }
    pub fn renew(&mut self) {
        self.hints.publish(self.class);
    }
    pub fn release_workload_hint(self) {
        drop(self);
    }
}
pub fn playback_class(playing: bool, heavy: bool, scanning: bool) -> Class {
    if scanning {
        Class::LibraryScan
    } else if playing && heavy {
        Class::PlaybackHeavy
    } else if playing {
        Class::PlaybackNormal
    } else {
        Class::Idle
    }
}

/// How long playback is held at the heavy class after an underrun. The platform parks cores while
/// the screen is dark and only ordinary playback is live; a heavy lease keeps all four online.
pub const XRUN_HEAVY: Duration = Duration::from_secs(300);

/// An underrun is the sign that the cores the platform left online were too few for this music.
/// Playback then asks for the heavy class for `XRUN_HEAVY` after the latest one.
#[derive(Default)]
pub struct XrunGuard {
    seen: f64,
    until: Option<Instant>,
}
impl XrunGuard {
    /// Whether to publish the heavy class now, given the running underrun count.
    pub fn heavy(&mut self, xruns: f64, now: Instant) -> bool {
        if xruns > self.seen {
            self.until = Some(now + XRUN_HEAVY);
        }
        self.seen = xruns;
        self.until.is_some_and(|until| now < until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn an_underrun_holds_playback_heavy_for_five_minutes() {
        let start = Instant::now();
        let mut guard = XrunGuard::default();
        // No underrun: never heavy.
        assert!(!guard.heavy(0., start));
        assert!(!guard.heavy(0., start + Duration::from_secs(60)));
        let at = start + Duration::from_secs(61);
        assert!(guard.heavy(1., at));
        assert!(guard.heavy(1., at + XRUN_HEAVY - Duration::from_secs(1)));
        assert!(!guard.heavy(1., at + XRUN_HEAVY));
        // A new one starts the window again; the count not moving never does.
        let later = at + XRUN_HEAVY + Duration::from_secs(10);
        assert!(guard.heavy(3., later));
        assert!(guard.heavy(3., later + Duration::from_secs(299)));
        assert!(!guard.heavy(3., later + Duration::from_secs(300)));
    }
    #[test]
    fn every_class_is_one_complete_device_transaction() {
        struct Device {
            expected: &'static [u8],
            calls: usize,
        }
        impl Write for Device {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.calls += 1;
                assert_eq!(bytes, self.expected, "fragmented workload request");
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        for (class, expected) in [
            (Class::Idle, "Idle 3000\n"),
            (Class::PlaybackNormal, "PlaybackNormal 3000\n"),
            (Class::PlaybackHeavy, "PlaybackHeavy 3000\n"),
            (Class::Interactive, "Interactive 250\n"),
            (Class::ArtworkDecode, "ArtworkDecode 3000\n"),
            (Class::LibraryScan, "LibraryScan 3000\n"),
            (Class::NetworkTransfer, "NetworkTransfer 3000\n"),
            (Class::Maintenance, "Maintenance 3000\n"),
        ] {
            let mut device = Device {
                expected: expected.as_bytes(),
                calls: 0,
            };
            write_request(&mut device, class).unwrap();
            write_request(&mut device, class).unwrap(); // renewal uses same transaction
            assert_eq!(device.calls, 2);
        }
    }
    #[test]
    fn rejected_transaction_propagates_without_publishing_a_fragment() {
        struct Reject(usize);
        impl Write for Reject {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 += 1;
                assert_eq!(bytes, b"Interactive 250\n");
                Err(std::io::Error::from_raw_os_error(22))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut device = Reject(0);
        assert!(write_request(&mut device, Class::Interactive).is_err());
        assert_eq!(device.0, 1);
    }
    #[test]
    fn independent_leases_and_screen_off_close_the_owned_descriptor() {
        use std::io::Read;
        use std::os::fd::OwnedFd;
        use std::os::unix::net::UnixStream;
        let (playback, mut playback_device) = UnixStream::pair().unwrap();
        let (interaction, mut interaction_device) = UnixStream::pair().unwrap();
        let mut playing = Hints {
            file: Some(File::from(OwnedFd::from(playback))),
            ..Hints::default()
        };
        let mut input = Hints {
            file: Some(File::from(OwnedFd::from(interaction))),
            ..Hints::default()
        };
        playing.publish(Class::PlaybackNormal);
        input.publish(Class::Interactive);
        let mut play = [0; 20];
        playback_device.read_exact(&mut play).unwrap();
        assert_eq!(&play, b"PlaybackNormal 3000\n");
        let mut request = [0; 16];
        interaction_device.read_exact(&mut request).unwrap();
        assert_eq!(&request, b"Interactive 250\n");
        input.clear();
        assert_eq!(interaction_device.read(&mut [0]).unwrap(), 0);
        assert!(playing.file.is_some());
        assert!(input.retry.is_none());
        drop(playing);
        assert_eq!(playback_device.read(&mut [0]).unwrap(), 0);
    }
    #[test]
    fn hints_are_classes_with_bounded_leases() {
        assert_eq!(Class::Interactive.lease_ms(), 250);
        assert_eq!(playback_class(true, false, false), Class::PlaybackNormal);
        assert_eq!(playback_class(true, true, false), Class::PlaybackHeavy);
        assert_eq!(playback_class(false, false, true), Class::LibraryScan);
        assert_eq!(playback_class(false, true, false), Class::Idle);
    }
}
