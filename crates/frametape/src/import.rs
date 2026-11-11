//! Legacy adapters preserve source bytes and project-specific replay semantics.
use crate::{Channel, Event, Frame, Result, TapeWriter, atomic_json, invalid};
use serde_json::{Value, json};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

fn channel(writer: &mut TapeWriter, id: &str, kind: &str, schema: &str) -> Result<()> {
    writer.register(Channel {
        id: id.into(),
        kind: kind.into(),
        schema: schema.into(),
    })
}

fn journal(output: &Path) -> std::path::PathBuf {
    let mut name = output.as_os_str().to_owned();
    name.push(".recording");
    name.into()
}

fn ns(seconds: f64) -> Result<u64> {
    if !seconds.is_finite() || seconds < 0.0 || seconds * 1e9 > u64::MAX as f64 {
        return Err(invalid("invalid frame duration"));
    }
    Ok((seconds * 1e9).round() as u64)
}

fn sidecars(writer: &mut TapeWriter, take: &Path, frames: &[Frame]) -> Result<()> {
    for (extension, id, kind, mime) in [
        ("mp4", "video", "video", "video/mp4"),
        ("h264", "video.annexb", "video", "video/h264"),
        ("video.json", "video.mapping", "video", "application/json"),
        ("aprof", "profile.aprof", "profile", "application/zip"),
        (
            "b3rec",
            "physics.box3d",
            "physics",
            "application/vnd.box3d.recording",
        ),
    ] {
        let path = take.with_extension(extension);
        if path.is_file() {
            channel(writer, id, kind, mime)?;
            writer.attach(id, &format!("source.{extension}"), path, mime)?;
        }
    }
    let mut annotations = crate::Annotations::load(take)?;
    if !annotations.containers.is_empty() {
        annotations.validate().map_err(invalid)?;
        if take.extension().is_some_and(|ext| ext == "jsonl") {
            map_annotations(&mut annotations, frames)?;
            channel(
                writer,
                "annotations.legacy",
                "custom",
                "frametape:annotations:v2:video-index",
            )?;
            writer.attach(
                "annotations.legacy",
                "source.annotations.json",
                take.with_extension("annotations.json"),
                "application/json",
            )?;
        }
        if annotations
            .containers
            .iter()
            .any(|c| c.end_frame >= writer.manifest().frame_count)
        {
            return Err(invalid("annotations exceed take frame count"));
        }
        atomic_json(&writer.directory().join("annotations.json"), &annotations)?;
    }
    Ok(())
}

fn map_annotations(annotations: &mut crate::Annotations, frames: &[Frame]) -> Result<()> {
    for container in &mut annotations.containers {
        let resolve = |video| {
            frames
                .iter()
                .find(|f| f.video_frame == Some(video))
                .map(|f| f.index)
                .ok_or_else(|| invalid("legacy annotation references missing video frame"))
        };
        container.start_frame = resolve(container.start_frame)?;
        container.end_frame = resolve(container.end_frame)?;
    }
    Ok(())
}

/// Synchronizes engine-authored annotations with the delivered bundle.
pub fn update_annotations(take: &Path) -> Result<()> {
    let output = take.with_extension("ftape");
    if !output.is_file() {
        raycat(take, &output)?;
        return Ok(());
    }
    let mut annotations = crate::Annotations::load(take)?;
    let frames = crate::TapeReader::open(&output)?.frames()?;
    map_annotations(&mut annotations, &frames)?;
    crate::save_annotations(&output, &annotations)
}

/// Import Raycat JSONL and completed sibling artifacts. Never decodes or replaces engine replay.
pub fn raycat(take: &Path, output: &Path) -> Result<std::path::PathBuf> {
    if output.exists() {
        return Err(invalid("output exists"));
    }
    let capture = if take.with_extension("bundle.json").is_file() {
        serde_json::from_reader::<_, Value>(File::open(take.with_extension("bundle.json"))?)
            .map_err(invalid)?
    } else {
        Value::Null
    };
    let mut writer = TapeWriter::create(
        journal(output),
        json!({"host":"raycat", "clock":"simulation", "video_fps":60,"capture":capture}),
    )?;
    channel(
        &mut writer,
        "state.raycat",
        "state",
        "raycat:entity-jsonl:v1",
    )?;
    let video = take.with_extension("mp4").is_file();
    let mapping = if take.with_extension("video.json").is_file() {
        Some(
            serde_json::from_reader::<_, Value>(File::open(take.with_extension("video.json"))?)
                .map_err(invalid)?,
        )
    } else {
        None
    };
    let mut clock = 0u64;
    let mut frame = None;
    let mut frames = Vec::new();
    for line in BufReader::new(File::open(take)?).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(&line).map_err(invalid)?;
        if let Some(delta) = value.get("dt") {
            let duration_ns = ns(delta.as_f64().ok_or_else(|| invalid("non-numeric dt"))?)?;
            let index = writer.manifest().frame_count;
            let video_frame = if video {
                if let Some(map) = &mapping {
                    let pairs = map["mapping"]
                        .as_array()
                        .ok_or_else(|| invalid("invalid video mapping"))?;
                    let first = pairs.first().and_then(|p| p[0].as_u64()).unwrap_or(
                        map["frames"]
                            .as_u64()
                            .ok_or_else(|| invalid("invalid video frame count"))?,
                    );
                    if index < first {
                        Some(index)
                    } else {
                        pairs
                            .iter()
                            .find(|p| p[1].as_u64() == Some(index))
                            .and_then(|p| p[0].as_u64())
                    }
                } else {
                    Some(index)
                }
            } else {
                None
            };
            let captured = Frame {
                index,
                timestamp_ns: clock,
                duration_ns,
                video_frame,
            };
            writer.frame(&captured)?;
            frames.push(captured);
            frame = Some(index);
            clock = clock
                .checked_add(duration_ns)
                .ok_or_else(|| invalid("take clock overflow"))?;
        } else {
            let frame = value.get("f").and_then(Value::as_u64).or(frame);
            let timestamp_ns = frame
                .and_then(|f| frames.get(f as usize))
                .map_or(0, |f| f.timestamp_ns);
            writer.event(&Event {
                frame,
                timestamp_ns,
                channel: "state.raycat".into(),
                data: value,
            })?;
        }
    }
    writer.attach("state.raycat", "source.jsonl", take, "application/x-ndjson")?;
    sidecars(&mut writer, take, &frames)?;
    writer.finish(output)
}

/// Import BallRPG's producer-exported frame JSON without exposing its private bit codec.
pub fn ballrpg_export(take: &Path, data: &Path, output: &Path) -> Result<std::path::PathBuf> {
    if output.exists() {
        return Err(invalid("output exists"));
    }
    let source: Value = serde_json::from_reader(File::open(data)?).map_err(invalid)?;
    let frames = source["frames"]
        .as_array()
        .ok_or_else(|| invalid("missing exported frames"))?;
    let mut writer = TapeWriter::create(
        journal(output),
        json!({"host":"ballrpg", "clock":"simulation", "video_fps":60, "units":source.get("units")}),
    )?;
    channel(&mut writer, "state.ballrpg", "state", "ballrpg:export:v1")?;
    let video = take.with_extension("mp4").is_file() || take.with_extension("h264").is_file();
    let mut clock = 0u64;
    let mut timeline = Vec::new();
    for (i, data) in frames.iter().enumerate() {
        let duration_ns = data["dt"]
            .as_u64()
            .and_then(|dt| dt.checked_mul(1000))
            .ok_or_else(|| invalid("invalid dt_us"))?;
        let index = i as u64;
        let frame = Frame {
            index,
            timestamp_ns: clock,
            duration_ns,
            video_frame: video.then_some(index),
        };
        writer.frame(&frame)?;
        timeline.push(frame);
        writer.event(&Event {
            frame: Some(index),
            timestamp_ns: clock,
            channel: "state.ballrpg".into(),
            data: data.clone(),
        })?;
        clock = clock
            .checked_add(duration_ns)
            .ok_or_else(|| invalid("take clock overflow"))?;
    }
    writer.attach(
        "state.ballrpg",
        "source.brtake",
        take,
        "application/vnd.ballrpg.take",
    )?;
    writer.attach(
        "state.ballrpg",
        "source.data.json",
        data,
        "application/json",
    )?;
    sidecars(&mut writer, take, &timeline)?;
    writer.finish(output)
}

/// Recover without modifying original evidence. Pending desktop media remains explicitly unverified.
pub fn recover(directory: &Path, output: &Path) -> Result<std::path::PathBuf> {
    if output.exists() {
        return Err(invalid("output exists"));
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(directory.join(".lock"))?;
    lock.try_lock()
        .map_err(|e| invalid(format!("journal is in use: {e}")))?;
    let manifest: crate::Manifest =
        serde_json::from_reader(File::open(directory.join("manifest.json"))?).map_err(invalid)?;
    if manifest.format != "frametape" || manifest.version != 1 {
        return Err(invalid("unsupported journal"));
    }
    if !manifest.metadata.is_object() {
        return Err(invalid("invalid journal metadata"));
    }
    let (mut frames, frame_tail) = recover_lines::<Frame>(&directory.join("frames.jsonl"))?;
    let (mut events, event_tail) = recover_lines::<Event>(&directory.join("events.jsonl"))?;
    let desktop = manifest.metadata["host"] == "desktop";
    let pending_input = directory.join("input.events.jsonl");
    let pending_video = directory.join("capture.mp4");
    let mut input_tail = 0;
    if desktop && pending_input.is_file() {
        let (input, tail) = recover_lines::<Event>(&pending_input)?;
        input_tail = tail;
        if input.iter().any(|e| e.channel != "input.evdev") {
            return Err(invalid("unexpected pending input channel"));
        }
        events.retain(|e| e.channel != "input.evdev");
        events.extend(input);
    }
    let unverified_video = desktop && pending_video.is_file();
    if unverified_video {
        for frame in &mut frames {
            frame.video_frame = None;
        }
    }
    let mut metadata = manifest.metadata;
    metadata["recovery"] = serde_json::json!({"complete":false,"truncated_frame_bytes":frame_tail,"truncated_event_bytes":event_tail,"truncated_input_bytes":input_tail,"pending_video_unverified":unverified_video});
    let mut name = output.as_os_str().to_owned();
    name.push(".recovery-recording");
    let mut writer = TapeWriter::create(std::path::PathBuf::from(name), metadata)?;
    for channel in manifest.channels {
        writer.register(channel)?;
    }
    for frame in &frames {
        writer.frame(frame)?;
    }
    for event in &events {
        if event.frame.is_some_and(|i| i >= frames.len() as u64) {
            return Err(invalid("journal event references missing frame"));
        }
        writer.event(event)?;
    }
    let root = std::fs::canonicalize(directory)?;
    for attachment in manifest.attachments {
        crate::safe_name(&attachment.path)?;
        let name = attachment
            .path
            .strip_prefix("attachments/")
            .ok_or_else(|| invalid("attachment outside attachments directory"))?;
        let source = std::fs::canonicalize(directory.join(&attachment.path))?;
        if !source.starts_with(&root)
            || !source.is_file()
            || std::fs::metadata(&source)?.len() != attachment.bytes
        {
            return Err(invalid("invalid recovered attachment"));
        }
        writer.attach(&attachment.channel, name, source, &attachment.media_type)?;
    }
    if desktop && pending_input.is_file() {
        writer.attach(
            "input.evdev",
            "input.interrupted.jsonl",
            &pending_input,
            "application/x-ndjson",
        )?;
    }
    if unverified_video {
        channel(
            &mut writer,
            "capture.recovery",
            "custom",
            "frametape:recovery:v1",
        )?;
        writer.attach(
            "capture.recovery",
            "video.unverified.mp4",
            &pending_video,
            "application/octet-stream",
        )?;
        writer.attach(
            "capture.recovery",
            "frames.accepted.jsonl",
            directory.join("frames.jsonl"),
            "application/x-ndjson",
        )?;
    }
    if directory.join("annotations.json").is_file() {
        let bytes = std::fs::read(directory.join("annotations.json"))?;
        let annotations = crate::Annotations::parse(std::str::from_utf8(&bytes).map_err(invalid)?)
            .map_err(invalid)?;
        annotations.validate().map_err(invalid)?;
        if annotations
            .containers
            .iter()
            .any(|c| c.end_frame >= frames.len() as u64)
        {
            return Err(invalid("recovered annotation range exceeds frames"));
        }
        atomic_json(&writer.directory().join("annotations.json"), &annotations)?;
    }
    writer.finish(output)
}

fn recover_lines<T: serde::de::DeserializeOwned>(path: &Path) -> Result<(Vec<T>, usize)> {
    let bytes = std::fs::read(path)?;
    let mut lines = bytes.split_inclusive(|b| *b == b'\n').peekable();
    let mut values = Vec::new();
    let mut tail = 0;
    while let Some(line) = lines.next() {
        match serde_json::from_slice(line) {
            Ok(value) => values.push(value),
            Err(_) if lines.peek().is_none() && !line.ends_with(b"\n") => tail = line.len(),
            Err(error) => return Err(invalid(error)),
        }
    }
    Ok((values, tail))
}
