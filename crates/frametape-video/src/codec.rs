use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Quality {
    /// Full chroma when hardware supports it; high quality 4:2:0 otherwise.
    #[default]
    Readable,
    /// Exact RGB samples, excluding alpha. Uses RGB software encoding.
    Lossless,
}

#[derive(Clone, Debug)]
pub struct EncoderConfig {
    pub quality: Quality,
    /// Explicit encoder selection fails rather than silently choosing another.
    pub encoder: Option<String>,
    pub ffmpeg: PathBuf,
    pub vaapi_device: Option<PathBuf>,
}

impl Default for EncoderConfig {
    fn default() -> Self {
        Self {
            quality: Quality::Readable,
            encoder: std::env::var("FRAMETAPE_VIDEO_ENCODER").ok(),
            ffmpeg: "ffmpeg".into(),
            vaapi_device: std::env::var_os("FRAMETAPE_VAAPI_DEVICE").map(PathBuf::from),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncoderInfo {
    pub name: &'static str,
    /// Encoded sample format, not the RGBA pipe's input format.
    pub pixel_format: &'static str,
    pub chroma: &'static str,
    pub hardware: bool,
    pub quality: Quality,
    pub vaapi_device: Option<PathBuf>,
}

impl EncoderInfo {
    pub(crate) fn input_args(&self, command: &mut Command) {
        if let Some(device) = &self.vaapi_device {
            command.arg("-vaapi_device").arg(device);
        }
    }

    pub(crate) fn output_args(&self, command: &mut Command, filter: &str) {
        if self.vaapi_device.is_some() {
            command.args(["-vf", &format!("{filter},format=nv12,hwupload")]);
        } else {
            command.args(["-vf", filter]);
        }
        command.args(["-an", "-c:v", self.name]);
        if self.name.ends_with("_nvenc") {
            command.args([
                "-preset", "p5", "-tune", "hq", "-rc", "constqp", "-qp", "18",
            ]);
            if self.name == "hevc_nvenc" && self.chroma == "4:4:4" {
                command.args(["-profile:v", "rext"]);
            } else if self.name == "h264_nvenc" && self.chroma == "4:4:4" {
                command.args(["-profile:v", "high444p"]);
            }
        } else if self.name.ends_with("_amf") {
            command.args([
                "-quality", "quality", "-rc", "cqp", "-qp_i", "18", "-qp_p", "18",
            ]);
        } else if self.vaapi_device.is_some() {
            command.args(["-rc_mode", "CQP", "-qp", "18"]);
        } else {
            command.args([
                "-preset",
                "veryfast",
                "-crf",
                if self.quality == Quality::Lossless {
                    "0"
                } else {
                    "16"
                },
            ]);
        }
        if self.vaapi_device.is_none() {
            command.args([
                "-pix_fmt",
                if self.name == "libx264rgb" {
                    "rgb24"
                } else {
                    self.pixel_format
                },
            ]);
        }
        if self.name.starts_with("hevc") {
            command.args(["-tag:v", "hvc1"]);
        }
    }
}

const CANDIDATES: &[(&str, &str, &str)] = &[
    ("hevc_nvenc", "yuv444p", "4:4:4"),
    ("h264_nvenc", "yuv444p", "4:4:4"),
    ("av1_nvenc", "yuv420p", "4:2:0"),
    ("hevc_amf", "yuv420p", "4:2:0"),
    ("av1_amf", "yuv420p", "4:2:0"),
    ("h264_amf", "yuv420p", "4:2:0"),
    ("hevc_vaapi", "yuv420p", "4:2:0"),
    ("av1_vaapi", "yuv420p", "4:2:0"),
    ("h264_vaapi", "yuv420p", "4:2:0"),
    ("libx264rgb", "gbrp", "RGB"),
    ("libx264", "yuv444p", "4:4:4"),
];

/// Opens each candidate and encodes a frame; listing an FFmpeg encoder is insufficient.
pub fn negotiate(config: &EncoderConfig) -> io::Result<EncoderInfo> {
    if config.quality == Quality::Lossless
        && config
            .encoder
            .as_deref()
            .is_some_and(|name| name != "libx264rgb")
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "RGB lossless requires libx264rgb; hardware YUV conversion is not RGB lossless",
        ));
    }
    if let Some(name) = &config.encoder
        && !CANDIDATES.iter().any(|candidate| candidate.0 == name)
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsupported encoder: {name}"),
        ));
    }
    let mut failures = Vec::new();
    for &(name, format, chroma) in CANDIDATES {
        if config
            .encoder
            .as_deref()
            .is_some_and(|chosen| chosen != name)
            || config.quality == Quality::Lossless && name != "libx264rgb"
        {
            continue;
        }
        let devices = if name.ends_with("_vaapi") {
            vaapi_devices(config)?
        } else {
            vec![None]
        };
        for device in devices {
            let info = EncoderInfo {
                name,
                pixel_format: format,
                chroma,
                hardware: !name.starts_with("lib"),
                quality: config.quality,
                vaapi_device: device,
            };
            let mut command = Command::new(&config.ffmpeg);
            command.args(["-hide_banner", "-loglevel", "error", "-filter_threads", "1"]);
            info.input_args(&mut command);
            // NVENC rejects 64×64 despite supporting desktop-sized frames.
            command.args([
                "-f",
                "lavfi",
                "-i",
                "color=s=640x360:d=0.04,format=rgba",
                "-frames:v",
                "1",
            ]);
            info.output_args(&mut command, "null");
            let output = command
                .args(["-threads", "1", "-f", "null", "-"])
                .stdin(Stdio::null())
                .output()?;
            if output.status.success() {
                return Ok(info);
            }
            failures.push(format!(
                "{name}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Err(io::Error::other(format!(
        "no usable video encoder: {}",
        failures.join("; ")
    )))
}

fn vaapi_devices(config: &EncoderConfig) -> io::Result<Vec<Option<PathBuf>>> {
    if let Some(device) = &config.vaapi_device {
        return Ok(vec![Some(device.clone())]);
    }
    let entries = match std::fs::read_dir("/dev/dri") {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut devices = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("renderD"))
        .map(|entry| Some(entry.path()))
        .collect::<Vec<_>>();
    devices.sort();
    Ok(devices)
}

pub(crate) fn default_encoder() -> io::Result<EncoderInfo> {
    static CHOSEN: OnceLock<Result<EncoderInfo, String>> = OnceLock::new();
    CHOSEN
        .get_or_init(|| negotiate(&EncoderConfig::default()).map_err(|error| error.to_string()))
        .clone()
        .map_err(io::Error::other)
}
