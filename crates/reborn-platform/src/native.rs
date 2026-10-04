use std::{
    ffi::{c_char, c_void, CStr, CString},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
static STOP: AtomicBool = AtomicBool::new(false);
extern "C" fn signal_handler(_: i32) {
    STOP.store(true, Ordering::Relaxed)
}
pub fn install_signals() {
    // SAFETY: signal handler performs only a lock-free atomic store and has static lifetime.
    unsafe {
        libc::signal(libc::SIGTERM, signal_handler as libc::sighandler_t);
        libc::signal(libc::SIGINT, signal_handler as libc::sighandler_t);
    }
}
pub fn stop_requested() -> bool {
    STOP.load(Ordering::Relaxed)
}
pub fn monotonic_seconds() -> Option<f64> {
    let mut value = std::mem::MaybeUninit::<libc::timespec>::uninit();
    // SAFETY: clock_gettime writes a valid timespec on success; only then read.
    unsafe {
        if libc::clock_gettime(libc::CLOCK_MONOTONIC, value.as_mut_ptr()) != 0 {
            return None;
        }
        let value = value.assume_init();
        Some(value.tv_sec as f64 + value.tv_nsec as f64 / 1e9)
    }
}
/// Ask evdev to stamp records with CLOCK_MONOTONIC so wheel cadence is
/// immune to wall-clock steps. Failure keeps the default clock; the input
/// layer then rejects negative intervals instead of trusting them.
pub fn evdev_monotonic_clock(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    // _IOW('E', 0xa0, int)
    const EVIOCSCLOCKID: libc::c_ulong = 0x4004_45a0;
    let clock: libc::c_int = libc::CLOCK_MONOTONIC;
    // SAFETY: valid open evdev descriptor and a pointer to a live c_int.
    unsafe { libc::ioctl(file.as_raw_fd(), EVIOCSCLOCKID as _, &clock) == 0 }
}
/// Wait on the existing evdev handles without a polling thread or allocations.
/// Readiness includes HUP/ERR so the normal input recovery path can cancel held
/// controls and rediscover a lost device. Signals interrupt shutdown promptly.
pub(crate) fn wait_readable<'a>(
    files: impl Iterator<Item = std::os::fd::BorrowedFd<'a>>,
    timeout: std::time::Duration,
) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;
    let mut fds = [libc::pollfd {
        fd: -1,
        events: libc::POLLIN,
        revents: 0,
    }; 16];
    let mut count = 0;
    for file in files {
        if count == fds.len() {
            return Err(std::io::Error::other("too many input handles"));
        }
        fds[count].fd = file.as_raw_fd();
        count += 1;
    }
    let ms = timeout
        .as_millis()
        .saturating_add(u128::from(
            !timeout.subsec_nanos().is_multiple_of(1_000_000),
        ))
        .min(i32::MAX as u128) as i32;
    // SAFETY: initialized pollfd array covers count; descriptors remain borrowed
    // throughout this synchronous call. No ownership or event bytes are consumed.
    let result = unsafe { libc::poll(fds.as_mut_ptr(), count as libc::nfds_t, ms) };
    if result >= 0 {
        return Ok(result > 0);
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::Interrupted {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod input_wait_tests {
    use super::wait_readable;
    use std::{
        io::{Read, Write},
        os::{fd::AsFd, unix::net::UnixStream},
        time::Duration,
    };
    #[test]
    fn wait_preserves_event_bytes_and_wakes_for_data_and_device_loss() {
        let (mut reader, mut writer) = UnixStream::pair().unwrap();
        assert!(!wait_readable(std::iter::once(reader.as_fd()), Duration::from_millis(2)).unwrap());
        let task = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            writer.write_all(b"event").unwrap();
            writer
        });
        assert!(wait_readable(std::iter::once(reader.as_fd()), Duration::from_secs(1)).unwrap());
        let writer = task.join().unwrap();
        let mut bytes = [0; 5];
        reader.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"event");
        drop(writer);
        assert!(wait_readable(std::iter::once(reader.as_fd()), Duration::from_secs(1)).unwrap());
    }
}
/// Bytes and total bytes of the filesystem holding `path`.
pub fn space(path: &Path) -> Result<(u64, u64), String> {
    use std::os::unix::ffi::OsStrExt;
    let s = CString::new(path.as_os_str().as_bytes()).map_err(|_| "NUL path")?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: valid NUL-terminated path and writable statvfs; read only on success.
    unsafe {
        if libc::statvfs(s.as_ptr(), stat.as_mut_ptr()) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let v = stat.assume_init();
        #[allow(clippy::unnecessary_cast)] // statvfs field widths differ on ARM32 and host.
        Ok((
            (v.f_bavail as u64) * (v.f_frsize as u64),
            (v.f_blocks as u64) * (v.f_frsize as u64),
        ))
    }
}
pub fn free_bytes(path: &Path) -> Result<u64, String> {
    use std::os::unix::ffi::OsStrExt;
    let s = CString::new(path.as_os_str().as_bytes()).map_err(|_| "NUL path")?;
    let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: valid NUL-terminated path and writable statvfs; result is read only on success.
    unsafe {
        if libc::statvfs(s.as_ptr(), stat.as_mut_ptr()) != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let v = stat.assume_init();
        #[allow(clippy::unnecessary_cast)] // statvfs field widths differ on ARM32 and host.
        let bytes = (v.f_bavail as u64) * (v.f_frsize as u64);
        Ok(bytes)
    }
}
#[link(name = "blkid")]
unsafe extern "C" {
    fn blkid_new_probe_from_filename(p: *const c_char) -> *mut c_void;
    fn blkid_do_safeprobe(p: *mut c_void) -> i32;
    fn blkid_probe_lookup_value(
        p: *mut c_void,
        name: *const c_char,
        data: *mut *const c_char,
        len: *mut usize,
    ) -> i32;
    fn blkid_free_probe(p: *mut c_void);
}
pub fn filesystem_uuid(device: &str) -> Option<String> {
    if !device.starts_with("/dev/") {
        return None;
    }
    let name = CString::new(device).ok()?;
    // SAFETY: probe has one local owner; lookup bytes are copied before probe destruction; no device data is modified.
    unsafe {
        let p = blkid_new_probe_from_filename(name.as_ptr());
        if p.is_null() {
            return None;
        }
        let mut value = std::ptr::null();
        let mut len = 0;
        let ok = blkid_do_safeprobe(p) == 0
            && blkid_probe_lookup_value(p, c"UUID".as_ptr(), &mut value, &mut len) == 0;
        let result = if ok && !value.is_null() {
            Some(CStr::from_ptr(value).to_string_lossy().into_owned())
        } else {
            None
        };
        blkid_free_probe(p);
        result
    }
}
