//! Engine-independent recording bundles. Producers own channels; this crate owns storage.
pub mod annotations;
pub mod import;
pub use annotations::*;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

pub type Result<T> = io::Result<T>;
const JSON_LIMIT: u64 = 64 * 1024 * 1024;

fn invalid(error: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, error.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    /// e.g. input, profile, physics, video, state, custom.
    pub kind: String,
    /// Versioned schema URI or media type; payload interpretation belongs to its producer.
    pub schema: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub channel: String,
    pub path: String,
    pub media_type: String,
    pub bytes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub id: String,
    pub created_unix_ns: u64,
    pub metadata: Value,
    pub channels: Vec<Channel>,
    pub attachments: Vec<Attachment>,
    pub frame_count: u64,
    pub event_count: u64,
    pub duration_ns: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Frame {
    pub index: u64,
    /// Nanoseconds since the producer's declared monotonic recording epoch.
    pub timestamp_ns: u64,
    pub duration_ns: u64,
    /// Encoded picture index. None means no captured image; never infer equality.
    pub video_frame: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Event {
    /// None for independently sampled producers; resolve by timestamp against frame intervals.
    pub frame: Option<u64>,
    pub timestamp_ns: u64,
    pub channel: String,
    pub data: Value,
}

/// A recoverable directory journal, sealed into one ZIP on finish. Drop retains the journal.
pub struct TapeWriter {
    lock: File,
    directory: PathBuf,
    manifest: Manifest,
    frames: BufWriter<File>,
    events: BufWriter<File>,
    last_timestamp: Option<u64>,
}

impl TapeWriter {
    pub fn create(directory: impl AsRef<Path>, metadata: Value) -> Result<Self> {
        if !metadata.is_object() {
            return Err(invalid("metadata must be a JSON object"));
        }
        let directory = directory.as_ref().to_owned();
        fs::create_dir(&directory)?;
        let lock = File::create(directory.join(".lock"))?;
        lock.try_lock().map_err(invalid)?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(invalid)?;
        let created_unix_ns = u64::try_from(now.as_nanos()).map_err(invalid)?;
        let writer = Self {
            lock,
            frames: BufWriter::new(File::create(directory.join("frames.jsonl"))?),
            events: BufWriter::new(File::create(directory.join("events.jsonl"))?),
            manifest: Manifest {
                format: "frametape".into(),
                version: 1,
                id: format!("{created_unix_ns:x}-{:x}", std::process::id()),
                created_unix_ns,
                metadata,
                channels: vec![],
                attachments: vec![],
                frame_count: 0,
                event_count: 0,
                duration_ns: 0,
            },
            directory,
            last_timestamp: None,
        };
        writer.save_manifest()?;
        Ok(writer)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn register(&mut self, channel: Channel) -> Result<()> {
        if channel.id.is_empty() || channel.schema.is_empty() || channel.kind.is_empty() {
            return Err(invalid("channel id, kind and schema must be nonempty"));
        }
        if let Some(held) = self
            .manifest
            .channels
            .iter()
            .find(|held| held.id == channel.id)
        {
            return if held == &channel {
                Ok(())
            } else {
                Err(invalid("channel already registered with different schema"))
            };
        }
        self.manifest.channels.push(channel);
        self.save_manifest()
    }

    pub fn frame(&mut self, frame: &Frame) -> Result<()> {
        if frame.index != self.manifest.frame_count
            || self.last_timestamp.is_some_and(|t| frame.timestamp_ns < t)
        {
            return Err(invalid(
                "frame indices must be contiguous and timestamps nondecreasing",
            ));
        }
        let end = frame
            .timestamp_ns
            .checked_add(frame.duration_ns)
            .ok_or_else(|| invalid("frame time overflow"))?;
        json_line(&mut self.frames, frame)?;
        self.last_timestamp = Some(frame.timestamp_ns);
        self.manifest.frame_count += 1;
        self.manifest.duration_ns = self.manifest.duration_ns.max(end);
        Ok(())
    }

    pub fn event(&mut self, event: &Event) -> Result<()> {
        if !self.manifest.channels.iter().any(|c| c.id == event.channel) {
            return Err(invalid(format!("unregistered channel {}", event.channel)));
        }
        json_line(&mut self.events, event)?;
        self.manifest.event_count += 1;
        Ok(())
    }

    /// Copies an original payload, preserving producer-owned encodings.
    pub fn attach(
        &mut self,
        channel: &str,
        name: &str,
        source: impl AsRef<Path>,
        media_type: &str,
    ) -> Result<()> {
        if !self.manifest.channels.iter().any(|c| c.id == channel) {
            return Err(invalid(format!("unregistered channel {channel}")));
        }
        let path = format!("attachments/{name}");
        safe_name(&path)?;
        if self.manifest.attachments.iter().any(|a| a.path == path) {
            return Err(invalid("duplicate attachment"));
        }
        let target = self.directory.join(&path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)?;
        let bytes = io::copy(&mut File::open(source)?, &mut out)?;
        out.sync_all()?;
        self.manifest.attachments.push(Attachment {
            channel: channel.into(),
            path,
            media_type: media_type.into(),
            bytes,
        });
        self.save_manifest()
    }

    pub fn checkpoint(&mut self) -> Result<()> {
        self.frames.flush()?;
        self.events.flush()?;
        self.frames.get_ref().sync_data()?;
        self.events.get_ref().sync_data()?;
        self.save_manifest()
    }

    fn save_manifest(&self) -> Result<()> {
        atomic_json(&self.directory.join("manifest.json"), &self.manifest)
    }

    pub fn finish(mut self, output: impl AsRef<Path>) -> Result<PathBuf> {
        self.checkpoint()?;
        for line in BufReader::new(File::open(self.directory.join("events.jsonl"))?).lines() {
            let event: Event = serde_json::from_str(&line?).map_err(invalid)?;
            if event.frame.is_some_and(|f| f >= self.manifest.frame_count) {
                return Err(invalid("event references missing source frame"));
            }
        }
        let mut members = vec![
            "manifest.json".to_owned(),
            "frames.jsonl".into(),
            "events.jsonl".into(),
        ];
        if self.directory.join("annotations.json").exists() {
            members.push("annotations.json".into());
        }
        members.extend(self.manifest.attachments.iter().map(|a| a.path.clone()));
        seal(&self.directory, &members, output.as_ref())?;
        let directory = self.directory.clone();
        let Self {
            lock,
            frames,
            events,
            ..
        } = self;
        drop(frames);
        drop(events);
        let mut folders = std::collections::BTreeSet::new();
        for name in members {
            let path = directory.join(name);
            fs::remove_file(&path)?;
            let mut parent = path.parent();
            while let Some(path) = parent.filter(|p| *p != directory.as_path()) {
                folders.insert(path.to_owned());
                parent = path.parent();
            }
        }
        for folder in folders.iter().rev() {
            fs::remove_dir(folder)?;
        }
        drop(lock);
        fs::remove_file(directory.join(".lock"))?;
        // Unknown files belong to the producer; never discard them during journal cleanup.
        match fs::remove_dir(&directory) {
            Ok(()) => (),
            Err(e) if e.kind() == io::ErrorKind::DirectoryNotEmpty => (),
            Err(e) => return Err(e),
        }
        Ok(output.as_ref().to_owned())
    }
}

fn json_line(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, value).map_err(invalid)?;
    writer.write_all(b"\n")
}

pub fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temp = path.with_extension("json.writing");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        serde_json::to_writer_pretty(&mut file, value).map_err(invalid)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn safe_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.contains(['\\', ':', '\0'])
        || name.starts_with('/')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
        || Path::new(name)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(invalid(format!("unsafe archive member {name:?}")));
    }
    Ok(())
}

fn options(name: &str, size: u64) -> SimpleFileOptions {
    let method = if name.ends_with(".json") || name.ends_with(".jsonl") {
        CompressionMethod::Deflated
    } else {
        CompressionMethod::Stored
    };
    SimpleFileOptions::default()
        .compression_method(method)
        .large_file(size > u32::MAX as u64)
}

fn seal(directory: &Path, members: &[String], output: &Path) -> Result<()> {
    let temp = output.with_extension("ftape.writing");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        let mut zip = ZipWriter::new(file);
        for name in members {
            safe_name(name)?;
            let path = directory.join(name);
            if !fs::symlink_metadata(&path)?.is_file() {
                return Err(invalid("attachment is not a regular file"));
            }
            let mut input = File::open(path)?;
            zip.start_file(name, options(name, input.metadata()?.len()))
                .map_err(invalid)?;
            io::copy(&mut input, &mut zip)?;
        }
        zip.finish().map_err(invalid)?.sync_all()?;
        // Hard-link publication is atomic and refuses to replace an existing take.
        fs::hard_link(&temp, output)?;
        fs::remove_file(&temp)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

pub struct TapeReader {
    archive: ZipArchive<File>,
    pub manifest: Manifest,
}

impl TapeReader {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let mut archive = ZipArchive::new(File::open(path)?).map_err(invalid)?;
        let mut names = HashSet::new();
        for i in 0..archive.len() {
            let member = archive.by_index(i).map_err(invalid)?;
            safe_name(member.name())?;
            if member.is_symlink() || !names.insert(member.name().to_owned()) {
                return Err(invalid("symlink or duplicate archive member"));
            }
        }
        let manifest = {
            let member = archive.by_name("manifest.json").map_err(invalid)?;
            if member.size() > JSON_LIMIT {
                return Err(invalid("manifest exceeds size limit"));
            }
            serde_json::from_reader::<_, Manifest>(member.take(JSON_LIMIT + 1)).map_err(invalid)?
        };
        if manifest.format != "frametape" || manifest.version != 1 {
            return Err(invalid("unsupported recording format/version"));
        }
        if !manifest.metadata.is_object() || manifest.id.is_empty() {
            return Err(invalid("invalid recording metadata/id"));
        }
        let mut channels = HashSet::new();
        for channel in &manifest.channels {
            if channel.id.is_empty()
                || channel.kind.is_empty()
                || channel.schema.is_empty()
                || !channels.insert(&channel.id)
            {
                return Err(invalid("invalid/duplicate channel"));
            }
        }
        for attachment in &manifest.attachments {
            if !channels.contains(&attachment.channel) {
                return Err(invalid("attachment names missing channel"));
            }
            let member = archive.by_name(&attachment.path).map_err(invalid)?;
            if member.size() != attachment.bytes {
                return Err(invalid("attachment size mismatch"));
            }
        }
        for name in ["frames.jsonl", "events.jsonl"] {
            archive.by_name(name).map_err(invalid)?;
        }
        Ok(Self { archive, manifest })
    }

    pub fn members(&self) -> Vec<String> {
        self.archive.file_names().map(str::to_owned).collect()
    }

    pub fn member_fingerprint(&mut self, name: &str) -> Result<(u32, u64)> {
        let member = self.archive.by_name(name).map_err(invalid)?;
        Ok((member.crc32(), member.size()))
    }

    pub fn read(&mut self, name: &str, limit: u64) -> Result<Vec<u8>> {
        let mut member = self.archive.by_name(name).map_err(invalid)?;
        if member.size() > limit {
            return Err(invalid(format!("{name} exceeds {limit} byte limit")));
        }
        let mut data = Vec::new();
        member.by_ref().take(limit + 1).read_to_end(&mut data)?;
        if data.len() as u64 > limit {
            return Err(invalid("expanded member exceeds limit"));
        }
        Ok(data)
    }

    pub fn extract(&mut self, name: &str, output: &Path) -> Result<()> {
        safe_name(name)?;
        let mut member = self.archive.by_name(name).map_err(invalid)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        let bytes = io::copy(&mut member, &mut file)?;
        if bytes != member.size() {
            return Err(invalid("truncated attachment"));
        }
        file.sync_all()
    }

    pub fn frames(&mut self) -> Result<Vec<Frame>> {
        let frames: Vec<Frame> = self.lines("frames.jsonl")?;
        if frames.len() as u64 != self.manifest.frame_count {
            return Err(invalid("frame count differs from manifest"));
        }
        let mut last = 0;
        for (i, frame) in frames.iter().enumerate() {
            if frame.index != i as u64
                || (i > 0 && frame.timestamp_ns < last)
                || frame
                    .timestamp_ns
                    .checked_add(frame.duration_ns)
                    .is_none_or(|end| end > self.manifest.duration_ns)
            {
                return Err(invalid("invalid frame timeline"));
            }
            last = frame.timestamp_ns;
        }
        Ok(frames)
    }
    pub fn events(&mut self) -> Result<Vec<Event>> {
        let events: Vec<Event> = self.lines("events.jsonl")?;
        if events.len() as u64 != self.manifest.event_count {
            return Err(invalid("event count differs from manifest"));
        }
        for event in &events {
            if event.frame.is_some_and(|f| f >= self.manifest.frame_count)
                || !self.manifest.channels.iter().any(|c| c.id == event.channel)
            {
                return Err(invalid("event references missing channel/frame"));
            }
        }
        Ok(events)
    }

    pub fn lines<T: serde::de::DeserializeOwned>(&mut self, name: &str) -> Result<Vec<T>> {
        let bytes = self.read(name, 256 * 1024 * 1024)?;
        BufReader::new(bytes.as_slice())
            .lines()
            .map(|line| serde_json::from_str(&line?).map_err(invalid))
            .collect()
    }

    pub fn annotations(&mut self) -> Result<Annotations> {
        if !self.archive.file_names().any(|n| n == "annotations.json") {
            return Ok(Annotations::default());
        }
        Annotations::parse(
            std::str::from_utf8(&self.read("annotations.json", JSON_LIMIT)?).map_err(invalid)?,
        )
        .map_err(invalid)
    }
}

/// Replaces annotations inside the single bundle; original remains intact on write failure.
pub fn save_annotations(path: &Path, annotations: &Annotations) -> Result<()> {
    let mut source = TapeReader::open(path)?;
    annotations.validate().map_err(invalid)?;
    let frames = source.manifest.frame_count;
    if annotations.version != 2
        || annotations
            .containers
            .iter()
            .any(|c| c.start_frame > c.end_frame || c.end_frame >= frames)
    {
        return Err(invalid("annotation frame range/version invalid"));
    }
    let temp = path.with_extension("ftape.annotating");
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let result = (|| {
        let mut target = ZipWriter::new(file);
        for i in 0..source.archive.len() {
            let member = source.archive.by_index(i).map_err(invalid)?;
            if member.name() != "annotations.json" {
                target.raw_copy_file(member).map_err(invalid)?;
            }
        }
        target
            .start_file("annotations.json", options("annotations.json", 0))
            .map_err(invalid)?;
        serde_json::to_writer(&mut target, annotations).map_err(invalid)?;
        target.finish().map_err(invalid)?.sync_all()?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

/// Finds the sampled image interval containing an independent channel event.
pub fn frame_at(frames: &[Frame], timestamp_ns: u64) -> Option<&Frame> {
    let index = frames
        .partition_point(|f| f.timestamp_ns <= timestamp_ns)
        .checked_sub(1)?;
    let frame = &frames[index];
    (timestamp_ns < frame.timestamp_ns.saturating_add(frame.duration_ns)).then_some(frame)
}
