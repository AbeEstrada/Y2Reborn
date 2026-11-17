//! The user's volume in the DAC. A wired headphone sink on a kernel with the headphone level control gets
//! its volume (and balance) from the CS43131's Master (0.5 dB steps, soft ramp by default) instead of from
//! gain applied to the already 16-bit stream: a gain after the 16-bit conversion rounds the samples a second
//! time, so each 6 dB of volume cost about one bit of the music. With the volume in the DAC the stream
//! reaches it as decoded, at any volume.
use super::*;
use crate::volume::{hardware_gain, HardwareGain};

/// A sink whose output applies the user's volume and balance, kept in step with them at every write.
struct DacVolumeSink {
    inner: Box<dyn AudioSink>,
    volume: Arc<AtomicU8>,
    balance: Arc<AtomicI8>,
    /// The headphone level's own Master (centi-dB), the base the volume is added to.
    trim_cdb: i32,
    applied: HardwareGain,
    /// Still applying the volume. False after a refusal: the Master is back at its base and the
    /// caller's software gain takes over.
    live: bool,
    log: Observer,
    correlation: u64,
}

/// `sink`, with the volume in the DAC where it can be: a wired headphone sink that reports the
/// headphone level it got (a kernel with the level control) and takes a Master. The speaker,
/// Bluetooth and kernels without the level control are returned as they are.
pub(super) fn engage(
    mut sink: Box<dyn AudioSink>,
    volume: &Arc<AtomicU8>,
    balance: &Arc<AtomicI8>,
    log: &Observer,
    correlation: u64,
) -> Box<dyn AudioSink> {
    let params = sink.parameters();
    let (Some(level), Some(trim_db)) = (params.hp_level_live, params.hardware_mixer_gain_db) else {
        return sink;
    };
    let trim_cdb = (f64::from(trim_db) * 100.0).round() as i32;
    let applied = hardware_gain(
        trim_cdb,
        volume.load(Ordering::Relaxed),
        balance.load(Ordering::Relaxed),
    );
    match sink.set_hardware_gain(applied.left_cdb, applied.right_cdb) {
        Ok(true) => {
            log.emit(
                Level::Info,
                "audio",
                "hardware_volume",
                "the DAC applies the volume",
                Some(correlation),
                json!({"hp_level":level,"trim_cdb":trim_cdb,
                       "left_cdb":applied.left_cdb,"right_cdb":applied.right_cdb}),
            );
            Box::new(DacVolumeSink {
                inner: sink,
                volume: volume.clone(),
                balance: balance.clone(),
                trim_cdb,
                applied,
                live: true,
                log: log.clone(),
                correlation,
            })
        }
        Ok(false) => sink,
        Err(e) => {
            // A half-written Master must not stay: back to the level's own.
            let _ = sink.set_hardware_gain(trim_cdb, trim_cdb);
            log.emit(
                Level::Warn,
                "audio",
                "hardware_volume_refused",
                &e,
                Some(correlation),
                json!({}),
            );
            sink
        }
    }
}

impl DacVolumeSink {
    /// The Master follows the live volume and balance (the sink applies the volume). `false`: it
    /// could not, and from now on the sink no longer applies the volume: the caller's gain takes
    /// over, and what it rendered for the DAC's volume must not be written.
    fn follow(&mut self) -> bool {
        let want = hardware_gain(
            self.trim_cdb,
            self.volume.load(Ordering::Relaxed),
            self.balance.load(Ordering::Relaxed),
        );
        if want == self.applied {
            return true;
        }
        match self.inner.set_hardware_gain(want.left_cdb, want.right_cdb) {
            Ok(true) => {
                self.applied = want;
                true
            }
            other => {
                self.live = false;
                let _ = self.inner.set_hardware_gain(self.trim_cdb, self.trim_cdb);
                let why = other
                    .err()
                    .unwrap_or_else(|| "the output no longer takes it".into());
                self.log.emit(
                    Level::Warn,
                    "audio",
                    "hardware_volume_off",
                    &why,
                    Some(self.correlation),
                    json!({}),
                );
                false
            }
        }
    }
}

impl AudioSink for DacVolumeSink {
    fn parameters(&self) -> reborn_audio::Parameters {
        self.inner.parameters()
    }
    fn write(&mut self, pcm: &[u8]) -> Result<usize, String> {
        if self.live && !self.follow() {
            // This block was rendered for the DAC's volume: none of it is written; the next one
            // is rendered with the software gain.
            return Ok(0);
        }
        self.inner.write(pcm)
    }
    fn discard(&mut self) -> Result<(), String> {
        self.inner.discard()
    }
    fn delay(&self) -> u64 {
        self.inner.delay()
    }
    fn queued(&self) -> Option<u64> {
        self.inner.queued()
    }
    fn pause(&mut self, on: bool) -> Result<(), String> {
        self.inner.pause(on)
    }
    fn applies_volume(&self) -> bool {
        self.live
    }
}
