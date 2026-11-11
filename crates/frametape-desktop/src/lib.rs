//! Linux capture producers, independent of the recorder CLI and any game engine.
#![cfg(target_os = "linux")]
mod input;
mod screen;
pub use input::{InputRecorder, InputSummary, input_devices};
pub use screen::{Picture, Screen};

pub fn monotonic_ns() -> std::io::Result<u64> {
    let mut time: libc::timespec = unsafe { std::mem::zeroed() };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return Err(std::io::Error::last_os_error());
    }
    (time.tv_sec as u64)
        .checked_mul(1_000_000_000)
        .and_then(|t| t.checked_add(time.tv_nsec as u64))
        .ok_or_else(|| std::io::Error::other("monotonic clock overflow"))
}
