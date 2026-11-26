//! The volume in the DAC: which sinks take it, how it follows the volume and balance, and the way back.
use super::dac_volume::engage;
use super::*;
use crate::volume::hardware_gain;
use reborn_audio::Parameters;

/// What the fake DAC was asked and was given.
#[derive(Default)]
struct Log {
    /// Every Master (left, right) in centi-dB it was set to.
    masters: Vec<(i32, i32)>,
    /// The PCM it received.
    written: Vec<u8>,
}

struct Dac {
    log: Arc<Mutex<Log>>,
    level: Option<u8>,
    trim_db: Option<f32>,
    /// Whether the output has a volume of its own: false answers `Ok(false)`, as the speaker and
    /// Bluetooth outputs do.
    takes: bool,
    /// The first `n` settings are taken, the next one fails.
    refuse_after: Option<usize>,
}
impl Dac {
    fn headphones(log: &Arc<Mutex<Log>>) -> Self {
        Self {
            log: log.clone(),
            level: Some(1),
            trim_db: Some(-6.0),
            takes: true,
            refuse_after: None,
        }
    }
}
impl AudioSink for Dac {
    fn parameters(&self) -> Parameters {
        Parameters {
            rate: 44_100,
            period: 512,
            buffer: 4096,
            format: PcmFormat::S16LE,
            channels: 2,
            hardware_mixer_gain_db: self.trim_db,
            device: "test".into(),
            fallback: false,
            fallback_reason: String::new(),
            hp_level_live: self.level,
            level_raised: false,
        }
    }
    fn write(&mut self, pcm: &[u8]) -> Result<usize, String> {
        self.log.lock().unwrap().written.extend_from_slice(pcm);
        Ok(pcm.len() / 4)
    }
    fn discard(&mut self) -> Result<(), String> {
        Ok(())
    }
    fn delay(&self) -> u64 {
        0
    }
    fn set_hardware_gain(&mut self, left_cdb: i32, right_cdb: i32) -> Result<bool, String> {
        if !self.takes {
            return Ok(false);
        }
        let mut log = self.log.lock().unwrap();
        if self.refuse_after.is_some_and(|n| log.masters.len() >= n) {
            return Err("injected mixer failure".into());
        }
        log.masters.push((left_cdb, right_cdb));
        Ok(true)
    }
}

/// An observer on a directory of its own (the tests run side by side).
fn observer() -> Observer {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "reborn-dac-volume-sink-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    Observer::new(&root).unwrap()
}

fn engaged(
    dac: Dac,
    volume: u8,
    balance: i8,
) -> (Box<dyn AudioSink>, Arc<AtomicU8>, Arc<AtomicI8>) {
    let volume = Arc::new(AtomicU8::new(volume));
    let balance = Arc::new(AtomicI8::new(balance));
    let sink = engage(Box::new(dac), &volume, &balance, &observer(), 7);
    (sink, volume, balance)
}

#[test]
fn a_headphone_sink_gets_the_volume_in_the_dac_and_follows_it() {
    let log = Arc::new(Mutex::new(Log::default()));
    let (mut sink, volume, balance) = engaged(Dac::headphones(&log), 50, 0);
    assert!(sink.applies_volume());
    let first = hardware_gain(-600, 50, 0);
    assert_eq!(
        log.lock().unwrap().masters,
        [(first.left_cdb, first.right_cdb)]
    );
    // The PCM goes through as it is, and a steady volume writes nothing more to the mixer.
    let pcm: Vec<u8> = (0..64u8).collect();
    assert_eq!(sink.write(&pcm).unwrap(), 16);
    assert_eq!(sink.write(&pcm).unwrap(), 16);
    assert_eq!(log.lock().unwrap().masters.len(), 1);
    assert_eq!(
        log.lock().unwrap().written,
        [pcm.clone(), pcm.clone()].concat()
    );
    // A new volume or balance reaches the mixer with the next write, once.
    volume.store(70, Ordering::Relaxed);
    sink.write(&pcm).unwrap();
    sink.write(&pcm).unwrap();
    balance.store(-3, Ordering::Relaxed);
    sink.write(&pcm).unwrap();
    let then = hardware_gain(-600, 70, 0);
    let panned = hardware_gain(-600, 70, -3);
    assert_eq!(
        log.lock().unwrap().masters,
        [
            (first.left_cdb, first.right_cdb),
            (then.left_cdb, then.right_cdb),
            (panned.left_cdb, panned.right_cdb)
        ]
    );
    assert!(sink.applies_volume());
}

#[test]
fn outputs_without_a_master_or_a_level_keep_the_software_volume() {
    // A kernel without the headphone level control: no level reported.
    let log = Arc::new(Mutex::new(Log::default()));
    let mut legacy = Dac::headphones(&log);
    legacy.level = None;
    let (sink, _, _) = engaged(legacy, 50, 0);
    assert!(!sink.applies_volume());
    assert!(log.lock().unwrap().masters.is_empty());
    // The speaker or Bluetooth: the output answers that it has no volume of its own.
    let mut speaker = Dac::headphones(&log);
    speaker.takes = false;
    let (mut sink, _, _) = engaged(speaker, 50, 0);
    assert!(!sink.applies_volume());
    assert_eq!(sink.write(&[0; 8]).unwrap(), 2, "its PCM is still written");
    assert!(log.lock().unwrap().masters.is_empty());
}

#[test]
fn a_refused_setting_gives_the_level_back_and_writes_nothing_at_the_wrong_gain() {
    let log = Arc::new(Mutex::new(Log::default()));
    let mut dac = Dac::headphones(&log);
    dac.refuse_after = Some(1); // the opening setting is taken, the next one is not
    let (mut sink, volume, _) = engaged(dac, 50, 0);
    assert!(sink.applies_volume());
    volume.store(30, Ordering::Relaxed);
    // This block was rendered for the DAC's volume: none of it is written.
    assert_eq!(sink.write(&[1; 8]).unwrap(), 0);
    assert!(log.lock().unwrap().written.is_empty());
    assert!(!sink.applies_volume());
    // The caller's software gain takes over from the next block on.
    assert_eq!(sink.write(&[2; 8]).unwrap(), 2);
    assert_eq!(log.lock().unwrap().written, [2; 8]);
    // The opening setting stands in the log: the failed give-back had nothing to add.
    assert_eq!(log.lock().unwrap().masters.len(), 1);
}

#[test]
fn a_refused_first_setting_leaves_the_plain_sink() {
    let log = Arc::new(Mutex::new(Log::default()));
    let mut dac = Dac::headphones(&log);
    dac.refuse_after = Some(0);
    let (sink, _, _) = engaged(dac, 50, 0);
    assert!(!sink.applies_volume());
    assert!(log.lock().unwrap().masters.is_empty());
}
