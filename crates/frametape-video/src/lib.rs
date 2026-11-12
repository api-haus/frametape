//! Frame-indexed RGBA recording through a bounded queue and a long-lived FFmpeg process.

mod codec;

pub use codec::{EncoderConfig, EncoderInfo, Quality, negotiate};

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;

pub type Frame = Box<dyn FnOnce() -> Vec<u8> + Send>;
const QUEUE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sent {
    Queued,
    Busy,
}

pub struct Recorder {
    frames: Option<SyncSender<Frame>>,
    writer: Option<thread::JoinHandle<io::Result<()>>>,
    path: PathBuf,
    info: EncoderInfo,
    width: u32,
    height: u32,
    written: u64,
}

impl Recorder {
    pub fn start(path: impl AsRef<Path>, width: u32, height: u32, fps: f64) -> io::Result<Self> {
        validate_video(width, height, fps)?;
        Self::open(
            path.as_ref(),
            width,
            height,
            fps,
            &EncoderConfig::default(),
            codec::default_encoder()?,
        )
    }

    pub fn start_with(
        path: impl AsRef<Path>,
        width: u32,
        height: u32,
        fps: f64,
        config: &EncoderConfig,
    ) -> io::Result<Self> {
        validate_video(width, height, fps)?;
        Self::open(
            path.as_ref(),
            width,
            height,
            fps,
            config,
            negotiate(config)?,
        )
    }

    fn open(
        path: &Path,
        width: u32,
        height: u32,
        fps: f64,
        config: &EncoderConfig,
        info: EncoderInfo,
    ) -> io::Result<Self> {
        let mut command = Command::new(&config.ffmpeg);
        command.args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-n",
            "-filter_threads",
            "1",
        ]);
        info.input_args(&mut command);
        command.args([
            "-f",
            "rawvideo",
            "-pixel_format",
            "rgba",
            "-video_size",
            &format!("{width}x{height}"),
            "-framerate",
            &fps.to_string(),
            "-i",
            "pipe:0",
        ]);
        info.output_args(&mut command, "pad=ceil(iw/2)*2:ceil(ih/2)*2");
        command.args(["-movflags", "+faststart"]).arg(path);
        let child = ChildGuard(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()?,
        );
        let bytes = frame_bytes(width, height)?;
        let (tx, rx) = sync_channel(QUEUE);
        let writer = thread::Builder::new()
            .name("frametape encode".into())
            .spawn(move || encode(child, rx, bytes))?;
        Ok(Self {
            frames: Some(tx),
            writer: Some(writer),
            path: path.to_owned(),
            info,
            width,
            height,
            written: 0,
        })
    }

    /// Prewarm default negotiation on a background thread before recording starts.
    /// An empty name means negotiation failed; `start` returns the detailed error.
    pub fn settle() -> &'static str {
        codec::default_encoder().map_or("", |info| info.name)
    }

    pub fn encoder(&self) -> &'static str {
        self.info.name
    }
    pub fn info(&self) -> &EncoderInfo {
        &self.info
    }
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
    /// Frames accepted by the queue. `finish` reports any later encoding failure.
    pub fn written(&self) -> u64 {
        self.written
    }

    pub fn write(&mut self, rgba: Vec<u8>) -> io::Result<Sent> {
        if rgba.len() != frame_bytes(self.width, self.height)? {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "frame is not width × height × 4 RGBA bytes",
            ));
        }
        self.write_with(Box::new(move || rgba))
    }

    /// Conversion runs on the encoder thread. A full queue returns `Busy`, never waits.
    pub fn write_with(&mut self, frame: Frame) -> io::Result<Sent> {
        match self
            .frames
            .as_ref()
            .expect("active recorder")
            .try_send(frame)
        {
            Ok(()) => {
                self.written += 1;
                Ok(Sent::Queued)
            }
            Err(TrySendError::Full(_)) => Ok(Sent::Busy),
            Err(TrySendError::Disconnected(_)) => Err(io::Error::other("the encoder stopped")),
        }
    }

    /// Waits for bounded queue capacity. Use outside a host's render thread when every
    /// captured picture must be retained; conversion still runs on the encoder thread.
    pub fn write_with_backpressure(&mut self, frame: Frame) -> io::Result<()> {
        self.frames
            .as_ref()
            .expect("active recorder")
            .send(frame)
            .map_err(|_| io::Error::other("the encoder stopped"))?;
        self.written += 1;
        Ok(())
    }

    /// Flushes every accepted frame and returns only after FFmpeg wrote its trailer.
    pub fn finish(mut self) -> io::Result<PathBuf> {
        self.frames.take();
        self.writer
            .take()
            .expect("active writer")
            .join()
            .map_err(|_| io::Error::other("the encoder thread panicked"))??;
        Ok(self.path.clone())
    }
}

impl Drop for Recorder {
    fn drop(&mut self) {
        // Closing the channel asks the owning thread to flush and reap its child.
        self.frames.take();
    }
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn frame_bytes(width: u32, height: u32) -> io::Result<usize> {
    if width == 0 || height == 0 || width > i32::MAX as u32 || height > i32::MAX as u32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame dimensions must fit positive signed integers",
        ));
    }
    (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "frame dimensions overflow"))
}

fn validate_video(width: u32, height: u32, fps: f64) -> io::Result<()> {
    frame_bytes(width, height)?;
    if !fps.is_finite() || fps <= 0.0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame rate must be finite and positive",
        ));
    }
    Ok(())
}

fn encode(mut child: ChildGuard, frames: Receiver<Frame>, bytes: usize) -> io::Result<()> {
    let mut stdin = child.0.stdin.take().expect("piped input");
    let mut stderr = child.0.stderr.take().expect("piped errors");
    // Drain concurrently: FFmpeg can fill stderr while this thread writes pixels.
    let errors = thread::Builder::new()
        .name("frametape ffmpeg errors".into())
        .spawn(move || {
            let mut tail = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stderr.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                tail.extend_from_slice(&buffer[..count]);
                if tail.len() > 65536 {
                    tail.drain(..tail.len() - 65536);
                }
            }
            Ok::<_, io::Error>(tail)
        })?;
    let mut failure = None;
    for frame in frames {
        let rgba = frame();
        if rgba.len() != bytes {
            failure = Some(io::Error::new(
                io::ErrorKind::InvalidInput,
                "converted frame has the wrong byte count",
            ));
            break;
        }
        if let Err(error) = stdin.write_all(&rgba) {
            failure = Some(error);
            break;
        }
    }
    drop(stdin);
    let status = child.0.wait()?;
    let said = errors
        .join()
        .map_err(|_| io::Error::other("FFmpeg error reader panicked"))??;
    if !status.success() || failure.is_some() {
        return Err(io::Error::other(format!(
            "ffmpeg exited {status}: {}; {}",
            String::from_utf8_lossy(&said).trim(),
            failure.map_or(String::new(), |error| error.to_string())
        )));
    }
    Ok(())
}

/// Fits resized segments to one canvas, then concatenates them in frame order.
pub fn concat(parts: &[(PathBuf, (u32, u32))], path: &Path, size: (u32, u32)) -> io::Result<()> {
    frame_bytes(size.0, size.1)?;
    if parts.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no video segments",
        ));
    }
    let (width, height) = (size.0.div_ceil(2) * 2, size.1.div_ceil(2) * 2);
    let info = codec::default_encoder()?;
    let config = EncoderConfig::default();
    let mut list = String::new();
    let mut fitted = Vec::new();
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let directory = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!(
            ".frametape-concat-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
    std::fs::create_dir(&directory)?;
    let listing = directory.join("parts.txt");
    let result = (|| {
        for (index, (part, part_size)) in parts.iter().enumerate() {
            let file = if *part_size == size {
                part.clone()
            } else {
                let fit = directory.join(format!("fit{index}.mp4"));
                fitted.push(fit.clone());
                let mut command = Command::new(&config.ffmpeg);
                command.args([
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-n",
                    "-filter_threads",
                    "1",
                ]);
                info.input_args(&mut command);
                command.arg("-i").arg(part);
                info.output_args(&mut command, &format!("scale={width}:{height}:force_original_aspect_ratio=decrease,pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1"));
                command.arg(&fit);
                run(command, "fit")?;
                fit
            };
            let file = std::path::absolute(&file)?;
            let name = file
                .to_str()
                .filter(|name| !name.contains(['\n', '\r']))
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "concat paths must be UTF-8 without newlines",
                    )
                })?;
            list += &format!("file '{}'\n", name.replace('\'', "'\\''"));
        }
        std::fs::write(&listing, &list)?;
        let mut command = Command::new(&config.ffmpeg);
        command
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-n",
                "-f",
                "concat",
                "-safe",
                "0",
                "-i",
            ])
            .arg(&listing)
            .args(["-c", "copy", "-movflags", "+faststart"])
            .arg(path);
        run(command, "concat")
    })();
    for file in fitted.iter().chain([&listing]) {
        let _ = std::fs::remove_file(file);
    }
    let _ = std::fs::remove_dir(directory);
    result
}

fn run(mut command: Command, step: &str) -> io::Result<()> {
    let output = command.stdin(Stdio::null()).output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(io::Error::other(format!(
        "ffmpeg {step} exited {}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}
