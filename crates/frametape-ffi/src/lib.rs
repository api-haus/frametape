//! C handles are independently owned; callers serialize access to each handle.
use frametape::{Channel, Event, Frame, TapeWriter};
use frametape_video::{Recorder, Sent};
use std::cell::RefCell;
use std::ffi::{CStr, CString, c_char, c_int};
use std::io;
use std::path::PathBuf;

thread_local! { static ERROR: RefCell<CString> = RefCell::new(CString::default()); }

fn call<T: Default>(action: impl FnOnce() -> io::Result<T>) -> T {
    ERROR.with(|e| *e.borrow_mut() = CString::default());
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(action)) {
        Ok(Ok(value)) => value,
        outcome => {
            let text = match outcome {
                Ok(Err(e)) => e.to_string(),
                _ => "frametape internal panic".into(),
            };
            ERROR.with(|e| *e.borrow_mut() = CString::new(text.replace('\0', "�")).unwrap());
            T::default()
        }
    }
}

fn status(action: impl FnOnce() -> io::Result<()>) -> c_int {
    if call(|| {
        action()?;
        Ok(true)
    }) {
        0
    } else {
        -1
    }
}

unsafe fn text<'a>(pointer: *const c_char) -> io::Result<&'a str> {
    if pointer.is_null() {
        return Err(io::Error::other("null string"));
    }
    unsafe { CStr::from_ptr(pointer) }
        .to_str()
        .map_err(io::Error::other)
}

unsafe fn handle<'a, T>(pointer: *mut T) -> io::Result<&'a mut T> {
    unsafe { pointer.as_mut() }.ok_or_else(|| io::Error::other("null handle"))
}

pub struct Session {
    writer: TapeWriter,
    output: PathBuf,
}
pub struct Video {
    recorder: Recorder,
    width: u32,
    height: u32,
    format: String,
    next: u64,
}

#[unsafe(no_mangle)]
pub extern "C" fn ft_last_error() -> *const c_char {
    ERROR.with(|e| e.borrow().as_ptr())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_session_open(
    path: *const c_char,
    metadata: *const c_char,
) -> *mut Session {
    call(|| {
        let output = PathBuf::from(unsafe { text(path)? });
        if output.exists() {
            return Err(io::Error::other("output exists"));
        }
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut journal = output.as_os_str().to_owned();
        journal.push(".recording");
        let metadata =
            serde_json::from_str(unsafe { text(metadata)? }).map_err(io::Error::other)?;
        let writer = TapeWriter::create(PathBuf::from(journal), metadata)?;
        Ok(Box::into_raw(Box::new(Session { writer, output })))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_session_frame(
    session: *mut Session,
    index: u64,
    timestamp_ns: u64,
    duration_ns: u64,
) -> c_int {
    status(|| {
        unsafe { handle(session)? }.writer.frame(&Frame {
            index,
            timestamp_ns,
            duration_ns,
            video_frame: None,
        })
    })
}

fn register(writer: &mut TapeWriter, id: &str) -> io::Result<()> {
    if !writer.manifest().channels.iter().any(|c| c.id == id) {
        writer.register(Channel {
            id: id.into(),
            kind: id.split('.').next().unwrap_or("custom").into(),
            schema: "application/json".into(),
        })?;
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_session_event(
    session: *mut Session,
    frame: u64,
    timestamp_ns: u64,
    channel: *const c_char,
    json: *const c_char,
) -> c_int {
    status(|| {
        let writer = &mut unsafe { handle(session)? }.writer;
        let channel = unsafe { text(channel)? };
        let data = serde_json::from_str(unsafe { text(json)? }).map_err(io::Error::other)?;
        register(writer, channel)?;
        writer.event(&Event {
            frame: (frame != u64::MAX).then_some(frame),
            timestamp_ns,
            channel: channel.into(),
            data,
        })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_session_attach(
    session: *mut Session,
    channel: *const c_char,
    path: *const c_char,
    media_type: *const c_char,
) -> c_int {
    status(|| {
        let writer = &mut unsafe { handle(session)? }.writer;
        let channel = unsafe { text(channel)? };
        let source = PathBuf::from(unsafe { text(path)? });
        let filename = source
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| io::Error::other("invalid attachment filename"))?;
        let name = format!("{}-{filename}", writer.manifest().attachments.len());
        register(writer, channel)?;
        writer.attach(channel, &name, source, unsafe { text(media_type)? })
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_session_close(session: *mut Session) -> c_int {
    status(|| {
        if session.is_null() {
            return Err(io::Error::other("null handle"));
        }
        let Session { writer, output } = *unsafe { Box::from_raw(session) };
        writer.finish(output)?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_session_abort(session: *mut Session) {
    call(|| {
        if !session.is_null() {
            drop(unsafe { Box::from_raw(session) });
        }
        Ok(())
    });
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_video_open(
    path: *const c_char,
    width: u32,
    height: u32,
    fps: u32,
    format: *const c_char,
) -> *mut Video {
    call(|| {
        let format = unsafe { text(format)? }.to_owned();
        if !["rgba", "bgra", "rgb0", "bgr0", "x2bgr10le"].contains(&format.as_str()) {
            return Err(io::Error::other("unsupported pixel format"));
        }
        let recorder = Recorder::start(unsafe { text(path)? }, width, height, f64::from(fps))?;
        Ok(Box::into_raw(Box::new(Video {
            recorder,
            width,
            height,
            format,
            next: 0,
        })))
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_video_frame(
    video: *mut Video,
    index: u64,
    pixels: *const u8,
    length: usize,
) -> c_int {
    call(|| {
        let video = unsafe { handle(video)? };
        let expected = (video.width as usize)
            .checked_mul(video.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| io::Error::other("image size overflow"))?;
        if pixels.is_null() || length != expected || index < video.next {
            return Err(io::Error::other(
                "invalid pixel data or nonincreasing frame index",
            ));
        }
        let mut data = unsafe { std::slice::from_raw_parts(pixels, length) }.to_vec();
        let format = video.format.clone();
        let sent = video.recorder.write_with(Box::new(move || {
            if format != "rgba" {
                for pixel in data.chunks_exact_mut(4) {
                    if format == "x2bgr10le" {
                        let packed = u32::from_le_bytes(pixel.try_into().unwrap());
                        pixel.copy_from_slice(&[
                            ((packed & 1023) >> 2) as u8,
                            (((packed >> 10) & 1023) >> 2) as u8,
                            (((packed >> 20) & 1023) >> 2) as u8,
                            255,
                        ]);
                    } else {
                        if format.starts_with("bgr") {
                            pixel.swap(0, 2);
                        }
                        if format.ends_with('0') {
                            pixel[3] = 255;
                        }
                    }
                }
            }
            data
        }))?;
        if sent == Sent::Queued {
            video.next = index + 1;
        }
        Ok(Some(if sent == Sent::Queued { 1 } else { 0 }))
    })
    .unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_video_close(video: *mut Video) -> c_int {
    status(|| {
        if video.is_null() {
            return Err(io::Error::other("null handle"));
        }
        unsafe { Box::from_raw(video) }.recorder.finish()?;
        Ok(())
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn ft_video_abort(video: *mut Video) {
    call(|| {
        if !video.is_null() {
            drop(unsafe { Box::from_raw(video) });
        }
        Ok(())
    });
}
