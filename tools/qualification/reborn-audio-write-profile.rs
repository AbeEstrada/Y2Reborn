//! Compile against native/audio.c with the production ARM SDK. This uses the
//! application's exact gain code and 512-frame write window with native wired
//! PCM, excluding decode/UI work. Caller owns the device lease and restoration.
extern crate self as reborn_core;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PcmFormat {
    S16LE,
    S24LE,
    S32LE,
}
impl PcmFormat {
    pub fn bytes_per_sample(self) -> usize {
        if self == Self::S16LE {
            2
        } else {
            4
        }
    }
    pub fn bytes_per_frame(self) -> usize {
        self.bytes_per_sample() * 2
    }
}
#[path = "../../app/reborn/src/volume.rs"]
mod volume;

#[repr(C)]
struct Params {
    rate: u32,
    period: u32,
    buffer: u32,
    format: u32,
    channels: u32,
    fallback: i32,
    hardware_mixer_gain_cdb: i32,
    device: [i8; 128],
    reason: [i8; 256],
}
unsafe extern "C" {
    fn rb_sink_open(
        name: *const std::ffi::c_char,
        rate: u32,
        format: u32,
        wired: i32,
        out: *mut *mut std::ffi::c_void,
        params: *mut Params,
    ) -> i32;
    fn rb_sink_write(sink: *mut std::ffi::c_void, data: *const u8, frames: i32) -> i32;
    fn rb_sink_wait(sink: *mut std::ffi::c_void, ms: i32) -> i32;
    fn rb_sink_close(sink: *mut std::ffi::c_void);
    fn clock_gettime(clock: i32, value: *mut [i32; 2]) -> i32;
}
fn cpu_seconds() -> f64 {
    let mut time = [0; 2];
    assert_eq!(unsafe { clock_gettime(3, &mut time) }, 0);
    f64::from(time[0]) + f64::from(time[1]) / 1e9
}
struct Sink(*mut std::ffi::c_void);
impl Drop for Sink {
    fn drop(&mut self) {
        unsafe { rb_sink_close(self.0) };
    }
}
fn main() {
    let mode = std::env::args().nth(1).unwrap();
    assert!(matches!(mode.as_str(), "unpaced" | "partial-wait"));
    let mut params: Params = unsafe { std::mem::zeroed() };
    let mut sink = std::ptr::null_mut();
    let name = c"hw:CARD=Y2Audio,DEV=0";
    assert_eq!(
        unsafe { rb_sink_open(name.as_ptr(), 48000, 1, 1, &mut sink, &mut params) },
        0
    );
    let sink = Sink(sink);
    let input = (0..512)
        .flat_map(|i| {
            let value = ((i * 93) % 40000 - 20000) as i16;
            value.to_le_bytes().repeat(2)
        })
        .collect::<Vec<_>>();
    let mut gain = volume::VolumeRamp::new(50, 48000);
    let (mut calls, mut frames, mut full, mut partial, mut again, mut zero, mut waits, mut small) =
        (0, 0, 0, 0, 0, 0, 0, 0);
    let mut minimum = 512;
    let started = std::time::Instant::now();
    let cpu = cpu_seconds();
    while started.elapsed().as_secs_f64() < 1.0 {
        gain.target(50);
        let scaled = gain.render(&input, PcmFormat::S16LE);
        let n = unsafe { rb_sink_write(sink.0, scaled.as_ptr(), 512) };
        calls += 1;
        assert!(n >= 0 || n == -11, "native write {n}");
        if n > 0 {
            frames += n;
            minimum = minimum.min(n);
            gain.advance(n as usize);
            if n == 512 {
                full += 1;
            } else {
                partial += 1;
            }
            if n < 64 {
                small += 1;
            }
        } else if n == 0 {
            zero += 1;
        } else {
            again += 1;
        }
        if n == -11 || (mode == "partial-wait" && n >= 0 && n < 512) {
            unsafe { rb_sink_wait(sink.0, 20) };
            waits += 1;
        }
    }
    let cpu = (cpu_seconds() - cpu) * 1000.0;
    let seconds = started.elapsed().as_secs_f64();
    drop(sink);
    println!("{{\"mode\":\"{mode}\",\"rate\":{},\"period\":{},\"buffer\":{},\"requested_frames\":512,\"seconds\":{seconds},\"thread_cpu_ms\":{cpu},\"calls\":{calls},\"accepted_frames\":{frames},\"full\":{full},\"partial\":{partial},\"zero\":{zero},\"again\":{again},\"waits\":{waits},\"writes_below_64_frames\":{small},\"minimum_positive_frames\":{minimum}}}",params.rate,params.period,params.buffer);
}
