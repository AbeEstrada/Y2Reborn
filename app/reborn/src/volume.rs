//! User gain at the live sink boundary, after the decode/EQ/limiter graph.
//! Keeping queued PCM independent of user volume avoids reopening the decoder
//! or ALSA and lets volume changes affect already buffered/crossfaded samples.
use reborn_core::PcmFormat;

pub(crate) struct VolumeRamp {
    gain: f64,
    target: f64,
    step: f64,
    remaining: usize,
    ramp_frames: usize,
}
impl VolumeRamp {
    pub(crate) fn new(volume: u8, rate: u32) -> Self {
        let gain = Self::gain(volume);
        Self {
            gain,
            target: gain,
            step: 0.0,
            remaining: 0,
            ramp_frames: (rate as usize / 200).max(1), // five milliseconds
        }
    }
    /// Start from silence and reach `volume` over `ms`: the first stream after
    /// the headphone level went up (hearing safety; a later volume change
    /// takes over with the usual short ramp).
    pub(crate) fn fade_in(volume: u8, rate: u32, ms: u32) -> Self {
        let mut ramp = Self::new(volume, rate);
        ramp.gain = 0.0;
        ramp.remaining = ((u64::from(rate) * u64::from(ms) / 1000) as usize).max(1);
        ramp.step = ramp.target / ramp.remaining as f64;
        ramp
    }
    fn gain(volume: u8) -> f64 {
        // Preserve the existing FFmpeg DSP's squared amplitude curve.
        (f64::from(volume.min(100)) / 100.0).powi(2)
    }
    pub(crate) fn target(&mut self, volume: u8) {
        let target = Self::gain(volume);
        if target != self.target {
            self.target = target;
            self.remaining = self.ramp_frames;
            self.step = (target - self.gain) / self.remaining as f64;
        }
    }
    /// Render speculatively; the sink may accept only part (or none) of it.
    /// Call advance only for frames actually accepted by the sink.
    pub(crate) fn render(&self, data: &[u8], format: PcmFormat) -> Vec<u8> {
        if self.remaining == 0 && self.gain == 1.0 {
            return data.to_vec();
        }
        let mut result = Vec::with_capacity(data.len());
        for (index, frame) in data.chunks_exact(format.bytes_per_frame()).enumerate() {
            let gain = self.at(index + 1);
            for sample in frame.chunks_exact(format.bytes_per_sample()) {
                match format {
                    PcmFormat::S16LE => {
                        let value = i16::from_le_bytes(sample.try_into().unwrap());
                        let value = (f64::from(value) * gain)
                            .round()
                            .clamp(f64::from(i16::MIN), f64::from(i16::MAX))
                            as i16;
                        result.extend_from_slice(&value.to_le_bytes());
                    }
                    PcmFormat::S24LE | PcmFormat::S32LE => {
                        let value = i32::from_le_bytes(sample.try_into().unwrap());
                        let (value, min, max) = if format == PcmFormat::S24LE {
                            ((value << 8) >> 8, -8_388_608, 8_388_607)
                        } else {
                            (value, i32::MIN, i32::MAX)
                        };
                        let value = (f64::from(value) * gain)
                            .round()
                            .clamp(f64::from(min), f64::from(max))
                            as i32;
                        result.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
        }
        result
    }
    fn at(&self, frames: usize) -> f64 {
        if frames >= self.remaining {
            self.target
        } else {
            self.gain + self.step * frames as f64
        }
    }
    pub(crate) fn advance(&mut self, frames: usize) {
        self.gain = self.at(frames);
        self.remaining = self.remaining.saturating_sub(frames);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fade_in_starts_silent_rises_steadily_and_yields_to_a_volume_change() {
        let mut ramp = VolumeRamp::fade_in(100, 1000, 2000); // 2000 frames
        let first = values(&ramp.render(&stereo16(10_000, 4), PcmFormat::S16LE));
        assert_eq!(first, [5, 5, 10, 10, 15, 15, 20, 20]);
        ramp.advance(1000);
        assert_eq!(
            values(&ramp.render(&stereo16(10_000, 1), PcmFormat::S16LE)),
            [5005, 5005]
        );
        ramp.advance(1000);
        assert_eq!(
            values(&ramp.render(&stereo16(10_000, 1), PcmFormat::S16LE)),
            [10_000, 10_000]
        );
        // Turning the volume down mid-fade uses the short ramp from where it is.
        let mut ramp = VolumeRamp::fade_in(100, 48_000, 2000);
        ramp.advance(48_000);
        ramp.target(50);
        ramp.advance(240);
        assert_eq!(
            values(&ramp.render(&stereo16(10_000, 1), PcmFormat::S16LE)),
            [2500, 2500]
        );
    }
    fn stereo16(sample: i16, frames: usize) -> Vec<u8> {
        sample.to_le_bytes().repeat(frames * 2)
    }
    fn values(data: &[u8]) -> Vec<i16> {
        data.as_chunks::<2>()
            .0
            .iter()
            .map(|s| i16::from_le_bytes(*s))
            .collect()
    }
    #[test]
    fn live_change_ramps_both_channels_and_reaches_exact_mute() {
        let mut ramp = VolumeRamp::new(100, 48_000);
        ramp.target(0);
        let data = values(&ramp.render(&stereo16(24_000, 480), PcmFormat::S16LE));
        for (i, pair) in data.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(pair[0], pair[1]);
            assert_eq!(pair[0], (24_000 - 100 * (i as i32 + 1)).max(0) as i16);
        }
        ramp.advance(480);
        assert!(ramp
            .render(&stereo16(-24_000, 10), PcmFormat::S16LE)
            .iter()
            .all(|v| *v == 0));
        ramp.target(50);
        ramp.advance(240);
        assert_eq!(
            values(&ramp.render(&stereo16(-24_000, 1), PcmFormat::S16LE)),
            [-6000, -6000]
        );
    }
    #[test]
    fn zero_and_partial_writes_do_not_consume_or_repeat_the_ramp() {
        let mut ramp = VolumeRamp::new(100, 48_000);
        ramp.target(0);
        let input = stereo16(24_000, 480);
        let all = ramp.render(&input, PcmFormat::S16LE);
        ramp.advance(0);
        assert_eq!(ramp.render(&input, PcmFormat::S16LE), all);
        ramp.advance(73);
        assert_eq!(
            ramp.render(&input[73 * 4..], PcmFormat::S16LE),
            all[73 * 4..]
        );
        // A new target starts from the last accepted gain, not the old target.
        ramp.target(100);
        let resumed = values(&ramp.render(&input, PcmFormat::S16LE));
        assert!((16_700..17_000).contains(&resumed[0]));
        assert_eq!(resumed[478], 24_000);
    }
    #[test]
    fn unity_preserves_low_bits_and_gain_preserves_signed_sample_packing() {
        for format in [PcmFormat::S24LE, PcmFormat::S32LE] {
            let input = [0x00012345i32, -0x12345]
                .into_iter()
                .flat_map(i32::to_le_bytes)
                .collect::<Vec<_>>();
            assert_eq!(VolumeRamp::new(100, 44_100).render(&input, format), input);
            let scaled = VolumeRamp::new(50, 44_100).render(&input, format);
            let samples = scaled
                .as_chunks::<4>()
                .0
                .iter()
                .map(|s| i32::from_le_bytes(*s))
                .collect::<Vec<_>>();
            assert_eq!(samples, [18_641, -18_641]);
        }
        // S24 padding may be zero for a negative value; sign comes from bit23.
        let input = [0x00800000i32, 0x007fffff]
            .into_iter()
            .flat_map(i32::to_le_bytes)
            .collect::<Vec<_>>();
        let scaled = VolumeRamp::new(50, 44_100).render(&input, PcmFormat::S24LE);
        assert_eq!(
            i32::from_le_bytes(scaled[..4].try_into().unwrap()),
            -2_097_152
        );
        assert_eq!(
            i32::from_le_bytes(scaled[4..].try_into().unwrap()),
            2_097_152
        );
    }
}
