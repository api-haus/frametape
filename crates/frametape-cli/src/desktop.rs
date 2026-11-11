use clap::Args;
use frametape::{Channel, Event, Frame, TapeWriter};
use frametape_desktop::{InputRecorder, Screen, monotonic_ns};
use frametape_video::{EncoderConfig, Quality, Recorder, Sent};
use serde_json::json;
use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Args)]
pub struct Options {
    pub path: PathBuf,
    #[arg(long)]
    pub output: Option<String>,
    #[arg(long,default_value_t=60, value_parser=clap::value_parser!(u32).range(1..=240))]
    pub fps: u32,
    #[arg(long)]
    pub seconds: Option<f64>,
    #[arg(long)]
    pub encoder: Option<String>,
    /// Preserve exact RGB pixels (may require software when hardware cannot guarantee RGB equality).
    #[arg(long)]
    pub lossless: bool,
    #[arg(long)]
    pub vaapi_device: Option<PathBuf>,
    #[arg(long)]
    pub no_input: bool,
    /// Select evdev nodes; by default follow all devices, including hotplug.
    #[arg(long)]
    pub input: Vec<PathBuf>,
}

pub fn record(options: Options) -> io::Result<()> {
    if options.path.exists() {
        return Err(io::Error::other("output exists"));
    }
    if options.seconds.is_some_and(|s| !s.is_finite() || s <= 0.0) {
        return Err(io::Error::other("seconds must be finite and positive"));
    }
    if let Some(parent) = options.path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut journal = options.path.as_os_str().to_owned();
    journal.push(".recording");
    let journal = PathBuf::from(journal);
    let mut screen = Screen::connect(options.output.as_deref())?;
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)).map_err(io::Error::other)?;
    let config = EncoderConfig {
        quality: if options.lossless {
            Quality::Lossless
        } else {
            Quality::Readable
        },
        encoder: options.encoder,
        vaapi_device: options.vaapi_device,
        ..Default::default()
    };
    let mut writer = TapeWriter::create(
        &journal,
        json!({"host":"desktop","output":screen.name,"clock":"CLOCK_MONOTONIC","video_fps":options.fps,"capture":"wlr-screencopy-shm"}),
    )?;
    writer.register(Channel {
        id: "video".into(),
        kind: "video".into(),
        schema: "video/mp4".into(),
    })?;
    writer.register(Channel {
        id: "capture".into(),
        kind: "custom".into(),
        schema: "frametape:capture:v1".into(),
    })?;
    let first = screen.capture()?;
    let video_path = journal.join("capture.mp4");
    let mut encoder = Recorder::start_with(
        &video_path,
        first.width,
        first.height,
        f64::from(options.fps),
        &config,
    )?;
    let epoch = monotonic_ns()?;
    let input_path = journal.join("input.events.jsonl");
    let input = if options.no_input {
        None
    } else {
        writer.register(Channel {
            id: "input.evdev".into(),
            kind: "input".into(),
            schema: "linux:evdev:input_event:v1".into(),
        })?;
        Some(InputRecorder::start(options.input, &input_path, epoch)?)
    };
    let info = encoder.info();
    writer.event(&Event {frame:None,timestamp_ns:0,channel:"capture".into(),data:json!({"epoch_ns":epoch,"encoder":info.name,"hardware":info.hardware,"pixel_format":info.pixel_format,"chroma":info.chroma,"quality":format!("{:?}",info.quality),"width":first.width,"height":first.height})})?;
    eprintln!(
        "recording {} using {} ({}); Ctrl+C stops",
        screen.name, info.name, info.chroma
    );
    let period = Duration::from_secs_f64(1.0 / f64::from(options.fps));
    let began = Instant::now();
    let mut due = Instant::now();
    drop(first);
    let mut pending: Option<Frame> = None;
    let mut next_index = 0u64;
    while !stop.load(Ordering::Relaxed)
        && options
            .seconds
            .is_none_or(|s| began.elapsed().as_secs_f64() < s)
    {
        let requested = monotonic_ns()?;
        let image = screen.capture()?;
        let received = monotonic_ns()?;
        // A foreign presentation epoch cannot be correlated with evdev; refuse guessed alignment.
        if image.timestamp_ns < requested.saturating_sub(250_000_000)
            || image.timestamp_ns > received.saturating_add(1_000_000)
        {
            return Err(io::Error::other(
                "compositor presentation clock differs from CLOCK_MONOTONIC",
            ));
        }
        if image.timestamp_ns < epoch {
            return Err(io::Error::other("capture predates recording epoch"));
        }
        let timestamp_ns = image.timestamp_ns - epoch;
        if let Some(mut previous) = pending.take() {
            previous.duration_ns = timestamp_ns
                .checked_sub(previous.timestamp_ns)
                .ok_or_else(|| io::Error::other("capture clock moved backward"))?;
            writer.frame(&previous)?;
        }
        let video_frame = if (image.width, image.height) == encoder.size() {
            let number = encoder.written();
            (encoder.write_with(Box::new(move || image.rgba()))? == Sent::Queued).then_some(number)
        } else {
            writer.event(&Event {frame:Some(next_index),timestamp_ns,channel:"capture".into(),data:json!({"error":"output resized; frame omitted","width":image.width,"height":image.height})})?;
            None
        };
        pending = Some(Frame {
            index: next_index,
            timestamp_ns,
            duration_ns: 0,
            video_frame,
        });
        next_index += 1;
        if next_index % options.fps as u64 == 0 {
            writer.checkpoint()?;
        }
        due += period;
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        } else {
            due = Instant::now();
        }
    }
    if let Some(mut frame) = pending {
        frame.duration_ns = (monotonic_ns()? - epoch).saturating_sub(frame.timestamp_ns);
        writer.frame(&frame)?;
    }
    if let Some(input) = input {
        let summary = input.stop()?;
        for line in BufReader::new(File::open(&input_path)?).lines() {
            let event: Event = serde_json::from_str(&line?).map_err(io::Error::other)?;
            writer.event(&event)?;
        }
        std::fs::remove_file(&input_path)?;
        writer.event(&Event {frame:None,timestamp_ns:monotonic_ns()?-epoch,channel:"capture".into(),data:json!({"input_events":summary.events,"lost_input_packets":summary.lost_packets,"devices":summary.devices})})?;
    }
    encoder.finish()?;
    writer.attach("video", "capture.mp4", &video_path, "video/mp4")?;
    std::fs::remove_file(&video_path)?;
    let path = writer.finish(&options.path)?;
    println!("{}", std::path::absolute(path)?.display());
    Ok(())
}
