use memmap2::MmapMut;
use std::ffi::CString;
use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::time::{Duration, Instant};
use wayland_client::protocol::{wl_buffer, wl_output, wl_registry, wl_shm, wl_shm_pool};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1 as frame, zwlr_screencopy_manager_v1 as manager,
};

#[allow(unused_imports, dead_code, clippy::all)]
mod toplevel {
    use wayland_client;
    use wayland_client::protocol::*;
    use wayland_protocols_wlr::foreign_toplevel::v1::client::*;
    pub mod __interfaces {
        use wayland_client::protocol::__interfaces::*;
        use wayland_protocols_wlr::foreign_toplevel::v1::client::__interfaces::*;
        wayland_scanner::generate_interfaces!("./protocols/hyprland-toplevel-export-v1.xml");
    }
    use self::__interfaces::*;
    wayland_scanner::generate_client_code!("./protocols/hyprland-toplevel-export-v1.xml");
}
use toplevel::{
    hyprland_toplevel_export_frame_v1 as window_frame,
    hyprland_toplevel_export_manager_v1 as window_manager,
};

/// Capture one compositor output or one isolated Hyprland window, independent of focus changes.
pub enum CaptureTarget<'a> {
    Output(Option<&'a str>),
    Window(u32),
}

pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub timestamp_ns: u64,
    pub stride: u32,
    format: wl_shm::Format,
    inverted: bool,
    pixels: MmapMut,
}

impl Picture {
    /// Packed RGBA8; conversion runs on the encoder thread when supplied through write_with.
    pub fn rgba(self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width as usize * self.height as usize * 4);
        let bgra = matches!(
            self.format,
            wl_shm::Format::Argb8888 | wl_shm::Format::Xrgb8888
        );
        for row in 0..self.height as usize {
            let y = if self.inverted {
                self.height as usize - 1 - row
            } else {
                row
            };
            let begin = y * self.stride as usize;
            for p in self.pixels[begin..begin + self.width as usize * 4].chunks_exact(4) {
                if bgra {
                    out.extend_from_slice(&[p[2], p[1], p[0], 255]);
                } else {
                    out.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
        }
        out
    }
}

#[derive(Default)]
struct State {
    outputs: Vec<(wl_output::WlOutput, String)>,
    manager: Option<manager::ZwlrScreencopyManagerV1>,
    window_manager: Option<window_manager::HyprlandToplevelExportManagerV1>,
    shm: Option<wl_shm::WlShm>,
    buffer: Option<wl_buffer::WlBuffer>,
    pixels: Option<MmapMut>,
    format: Option<wl_shm::Format>,
    dimensions: (u32, u32, u32),
    inverted: bool,
    ready: Option<u64>,
    failure: Option<String>,
}

pub struct Screen {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
    output: Option<wl_output::WlOutput>,
    window: Option<u32>,
    pub name: String,
}

impl Screen {
    pub fn connect(output_name: Option<&str>) -> io::Result<Self> {
        Self::connect_target(CaptureTarget::Output(output_name))
    }

    pub fn connect_target(target: CaptureTarget<'_>) -> io::Result<Self> {
        let connection = Connection::connect_to_env().map_err(io::Error::other)?;
        let mut queue = connection.new_event_queue();
        connection.display().get_registry(&queue.handle(), ());
        let mut state = State::default();
        queue.roundtrip(&mut state).map_err(io::Error::other)?;
        queue.roundtrip(&mut state).map_err(io::Error::other)?;
        if let CaptureTarget::Window(handle) = target {
            if state.window_manager.is_none() || state.shm.is_none() {
                return Err(io::Error::other(
                    "window capture requires Hyprland toplevel export and wl_shm",
                ));
            }
            return Ok(Self {
                connection,
                queue,
                state,
                output: None,
                window: Some(handle),
                name: format!("window:{handle:x}"),
            });
        }
        let CaptureTarget::Output(output_name) = target else {
            unreachable!()
        };
        if state.manager.is_none() || state.shm.is_none() {
            return Err(io::Error::other(
                "compositor needs wlr-screencopy v3 and wl_shm",
            ));
        }
        let selected = state
            .outputs
            .iter()
            .find(|(_, name)| output_name.is_some_and(|wanted| wanted == name));
        let (output, name) = if output_name.is_some() {
            selected
        } else if state.outputs.len() == 1 {
            state.outputs.first()
        } else {
            None
        }
        .cloned()
        .ok_or_else(|| {
            io::Error::other(format!(
                "select --output; available: {}",
                state
                    .outputs
                    .iter()
                    .map(|(_, n)| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
        Ok(Self {
            connection,
            queue,
            state,
            output: Some(output),
            window: None,
            name,
        })
    }

    pub fn capture(&mut self) -> io::Result<Picture> {
        self.state.ready = None;
        self.state.failure = None;
        self.state.inverted = false;
        self.state.format = None;
        self.state.pixels = None;
        let output_capture = self.output.as_ref().map(|output| {
            self.state
                .manager
                .as_ref()
                .unwrap()
                .capture_output(1, output, &self.queue.handle(), ())
        });
        let window_capture = self.window.map(|handle| {
            self.state
                .window_manager
                .as_ref()
                .unwrap()
                .capture_toplevel(1, handle, &self.queue.handle(), ())
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.state.ready.is_none() && self.state.failure.is_none() {
            self.queue
                .dispatch_pending(&mut self.state)
                .map_err(io::Error::other)?;
            if self.state.ready.is_some() || self.state.failure.is_some() {
                break;
            }
            self.connection.flush().map_err(io::Error::other)?;
            let Some(guard) = self.queue.prepare_read() else {
                continue;
            };
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.state.failure = Some("desktop capture timed out".into());
                break;
            }
            let mut poll = libc::pollfd {
                fd: self.connection.as_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let result = unsafe {
                libc::poll(
                    &mut poll,
                    1,
                    remaining.as_millis().min(i32::MAX as u128) as i32,
                )
            };
            if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                self.state.failure = Some(error.to_string());
                break;
            }
            if result > 0 {
                guard.read().map_err(io::Error::other)?;
            }
        }
        if let Some(capture) = output_capture {
            capture.destroy();
        }
        if let Some(capture) = window_capture {
            capture.destroy();
        }
        if let Some(buffer) = self.state.buffer.take() {
            buffer.destroy();
        }
        if let Some(error) = self.state.failure.take() {
            return Err(io::Error::other(error));
        }
        let (width, height, stride) = self.state.dimensions;
        Ok(Picture {
            width,
            height,
            stride,
            timestamp_ns: self.state.ready.unwrap(),
            format: self
                .state
                .format
                .ok_or_else(|| io::Error::other("no supported shm format"))?,
            pixels: self
                .state
                .pixels
                .take()
                .ok_or_else(|| io::Error::other("no captured pixels"))?,
            inverted: self.state.inverted,
        })
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_output" => state
                    .outputs
                    .push((registry.bind(name, version.min(4), qh, ()), String::new())),
                "wl_shm" => state.shm = Some(registry.bind(name, 1, qh, ())),
                "zwlr_screencopy_manager_v1" if version >= 3 => {
                    state.manager = Some(registry.bind(name, 3, qh, ()))
                }
                "hyprland_toplevel_export_manager_v1" => {
                    state.window_manager = Some(registry.bind(name, version.min(2), qh, ()))
                }
                _ => (),
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            if let Some((_, held)) = state.outputs.iter_mut().find(|(o, _)| o == output) {
                *held = name;
            }
        }
    }
}

impl Dispatch<frame::ZwlrScreencopyFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        object: &frame::ZwlrScreencopyFrameV1,
        event: frame::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        process_frame(state, event, qh, |buffer| object.copy(buffer));
    }
}

fn process_frame(
    state: &mut State,
    event: frame::Event,
    qh: &QueueHandle<State>,
    copy: impl FnOnce(&wl_buffer::WlBuffer),
) {
    let result = (|| -> io::Result<()> {
        match event {
            frame::Event::Buffer {
                format: WEnum::Value(format),
                width,
                height,
                stride,
            } => {
                if !matches!(
                    format,
                    wl_shm::Format::Argb8888
                        | wl_shm::Format::Xrgb8888
                        | wl_shm::Format::Abgr8888
                        | wl_shm::Format::Xbgr8888
                ) {
                    return Err(io::Error::other(format!(
                        "unsupported desktop pixel format {format:?}"
                    )));
                }
                let size = (stride as usize)
                    .checked_mul(height as usize)
                    .filter(|n| *n > 0 && *n <= i32::MAX as usize)
                    .ok_or_else(|| io::Error::other("invalid desktop buffer size"))?;
                if width == 0
                    || stride
                        < width
                            .checked_mul(4)
                            .ok_or_else(|| io::Error::other("invalid width"))?
                {
                    return Err(io::Error::other("invalid desktop stride"));
                }
                let name = CString::new("frametape").unwrap();
                let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
                if fd < 0 {
                    return Err(io::Error::last_os_error());
                }
                let file = unsafe { File::from_raw_fd(fd) };
                file.set_len(size as u64)?;
                let pixels = unsafe { MmapMut::map_mut(&file)? };
                let pool =
                    state
                        .shm
                        .as_ref()
                        .unwrap()
                        .create_pool(file.as_fd(), size as i32, qh, ());
                state.buffer = Some(pool.create_buffer(
                    0,
                    width as i32,
                    height as i32,
                    stride as i32,
                    format,
                    qh,
                    (),
                ));
                pool.destroy();
                state.pixels = Some(pixels);
                state.format = Some(format);
                state.dimensions = (width, height, stride);
            }
            frame::Event::BufferDone => {
                let buffer = state
                    .buffer
                    .as_ref()
                    .ok_or_else(|| io::Error::other("compositor supplied no shm buffer"))?;
                copy(buffer);
            }
            frame::Event::Ready {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => {
                let seconds = ((tv_sec_hi as u64) << 32) | tv_sec_lo as u64;
                state.ready = Some(
                    seconds
                        .checked_mul(1_000_000_000)
                        .and_then(|t| t.checked_add(tv_nsec as u64))
                        .ok_or_else(|| io::Error::other("capture time overflow"))?,
                );
            }
            frame::Event::Flags {
                flags: WEnum::Value(flags),
            } => state.inverted = flags.contains(frame::Flags::YInvert),
            frame::Event::Failed => {
                return Err(io::Error::other("compositor refused desktop capture"));
            }
            _ => (),
        }
        Ok(())
    })();
    if let Err(error) = result {
        state.failure = Some(error.to_string());
    }
}

impl Dispatch<window_frame::HyprlandToplevelExportFrameV1, ()> for State {
    fn event(
        state: &mut Self,
        object: &window_frame::HyprlandToplevelExportFrameV1,
        event: window_frame::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let event = match event {
            window_frame::Event::Buffer {
                format,
                width,
                height,
                stride,
            } => frame::Event::Buffer {
                format,
                width,
                height,
                stride,
            },
            window_frame::Event::BufferDone => frame::Event::BufferDone,
            window_frame::Event::Ready {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            } => frame::Event::Ready {
                tv_sec_hi,
                tv_sec_lo,
                tv_nsec,
            },
            window_frame::Event::Flags {
                flags: WEnum::Value(flags),
            } => frame::Event::Flags {
                flags: WEnum::Value(frame::Flags::from_bits_truncate(flags.bits())),
            },
            window_frame::Event::Failed => frame::Event::Failed,
            _ => return,
        };
        process_frame(state, event, qh, |buffer| object.copy(buffer, 1));
    }
}

delegate_noop!(State: ignore wl_shm::WlShm);
delegate_noop!(State: ignore wl_shm_pool::WlShmPool);
delegate_noop!(State: ignore wl_buffer::WlBuffer);
delegate_noop!(State: ignore manager::ZwlrScreencopyManagerV1);

delegate_noop!(State: ignore window_manager::HyprlandToplevelExportManagerV1);
