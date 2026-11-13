//! Local media producer: retain compressed sources, prepare a seekable video once.
use frametape::{
    TapeReader,
    playback::{PlaybackPlan, picture_count},
};
use serde_json::{Value, json};
use std::{
    collections::hash_map::DefaultHasher,
    fs,
    hash::{Hash, Hasher},
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
pub struct Media {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
}

pub fn open(path: &Path, cache: &Path) -> io::Result<Option<Media>> {
    let mut tape = TapeReader::open(path)?;
    let candidates = tape
        .manifest
        .attachments
        .iter()
        .filter(|a| a.media_type.starts_with("video/"))
        .collect::<Vec<_>>();
    let attachment = candidates
        .iter()
        .find(|a| a.media_type == "video/mp4")
        .or(candidates.first())
        .cloned()
        .cloned();
    let Some(attachment) = attachment else {
        return Ok(None);
    };
    let extension = Path::new(&attachment.path)
        .extension()
        .and_then(|x| x.to_str())
        .filter(|x| ["mp4", "mkv", "webm", "h264"].contains(x))
        .unwrap_or("video");
    let (crc, bytes) = tape.member_fingerprint(&attachment.path)?;
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    attachment.path.hash(&mut hash);
    crc.hash(&mut hash);
    bytes.hash(&mut hash);
    let folder = cache.join(format!("{:016x}", hash.finish()));
    fs::create_dir_all(&folder)?;
    let source = folder.join(format!("original.{extension}"));
    if fs::metadata(&source).map_or(true, |m| m.len() != bytes) {
        let temporary = temporary(&folder, extension)?;
        let extracted = (|| {
            tape.extract(&attachment.path, &temporary)?;
            fs::rename(&temporary, &source)
        })();
        if extracted.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        extracted?;
    }
    let result = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
        ])
        .arg(&source)
        .stdin(Stdio::null())
        .output()?;
    if !result.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&result.stderr).into_owned(),
        ));
    }
    let info: serde_json::Value =
        serde_json::from_slice(&result.stdout).map_err(io::Error::other)?;
    let stream = &info["streams"][0];
    let width = stream["width"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| io::Error::other("missing video width"))?;
    let height = stream["height"]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| io::Error::other("missing video height"))?;
    if width == 0 || height == 0 {
        return Err(io::Error::other("invalid video dimensions/rate"));
    }
    Ok(Some(Media {
        path: source,
        width,
        height,
    }))
}

fn temporary(folder: &Path, extension: &str) -> io::Result<PathBuf> {
    let mut random = [0; 8];
    getrandom::fill(&mut random).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(folder.join(format!(
        "prepare-{:016x}.{extension}",
        u64::from_le_bytes(random)
    )))
}

fn probe(path: &Path, count: usize, decoded: bool) -> io::Result<Value> {
    let entries = if decoded {
        "stream=codec_name,has_b_frames,time_base,width,height,start_time,duration:format=format_name,duration:frame=best_effort_timestamp_time,duration_time,pkt_duration_time:frame_side_data="
    } else {
        "stream=codec_name,has_b_frames,time_base,width,height,start_time,duration:format=format_name,duration:packet=pts_time,duration_time:packet_side_data="
    };
    let mut command = Command::new("ffprobe");
    command.args(["-v", "fatal", "-select_streams", "v:0"]);
    if !decoded {
        command.args(["-read_intervals", &format!("%+#{}", count + 1)]);
    }
    command
        .args(["-show_entries", entries, "-of", "json"])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped());
    let mut child = command.spawn()?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .unwrap()
        .take(128 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() > 128 * 1024 * 1024 {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io::Error::other("Video index exceeds review memory limit"));
    }
    if !child.wait()?.success() {
        return Err(io::Error::other("Cannot index recording video"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}

fn run(command: &mut Command) -> io::Result<()> {
    let result = command.stdin(Stdio::null()).output()?;
    if result.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&result.stderr).into_owned(),
        ))
    }
}

pub struct Playback {
    pub path: PathBuf,
    pub mime: String,
    pub descriptor: Value,
}

pub fn playback(path: &Path, cache: &Path, compatible: bool) -> io::Result<Option<Playback>> {
    let Some(media) = open(path, cache)? else {
        return Ok(None);
    };
    let mut tape = TapeReader::open(path)?;
    let frames = tape.frames()?;
    let count = picture_count(&frames)?;
    let mut hash = DefaultHasher::new();
    "video-player-v2".hash(&mut hash);
    serde_json::to_vec(&frames)?.hash(&mut hash);
    let folder = media
        .path
        .parent()
        .unwrap()
        .join(format!("playback-{:016x}", hash.finish()));
    fs::create_dir_all(&folder)?;
    let index = folder.join("index.json");
    let plan: PlaybackPlan = if index.exists() {
        serde_json::from_reader(fs::File::open(&index)?).map_err(io::Error::other)?
    } else {
        let mut p = probe(&media.path, count, false)?;
        if !matches!(
            p["streams"][0]["codec_name"].as_str(),
            Some("h264" | "hevc")
        ) {
            p = probe(&media.path, count, true)?;
        }
        let plan = PlaybackPlan::new(&frames, &p)?;
        frametape::atomic_json(&index, &plan)?;
        plan
    };
    let mode = if compatible || !plan.copy && !plan.original {
        "compatible"
    } else {
        "clocked"
    };
    let result = folder.join(format!("{mode}.mp4"));
    let verified = folder.join(format!("{mode}.json"));
    let source = if plan.original && !compatible {
        media.path.clone()
    } else {
        result.clone()
    };
    if !verified.exists() || !source.exists() {
        if source == result {
            let temporary = temporary(&folder, "mp4")?;
            let bsf = folder.join(format!("{mode}.bsf"));
            fs::write(
                &bsf,
                if mode == "compatible" {
                    &plan.encode_bsf
                } else {
                    &plan.bsf
                },
            )?;
            let mut command = Command::new("ffmpeg");
            command.args(["-hide_banner", "-loglevel", "error", "-n", "-threads", "2"]);
            if mode == "compatible" {
                command.args(["-r", &(count as f64 / plan.duration).to_string()]);
            }
            command
                .arg("-i")
                .arg(&media.path)
                .args(["-map", "0:v:0", "-an"]);
            if mode == "compatible" {
                let filter_path = folder.join("source-clock.filter");
                fs::write(&filter_path, &plan.filter)?;
                command.arg("-/filter:v").arg(filter_path);
                command.args([
                    "-c:v",
                    "libx264",
                    "-pix_fmt",
                    "yuv420p",
                    "-preset",
                    "veryfast",
                    "-crf",
                    "16",
                    "-bf",
                    "0",
                    "-fps_mode",
                    "passthrough",
                    "-enc_time_base",
                    "1:1000000",
                    "-threads",
                    "2",
                ]);
            } else {
                command.args(["-c:v", "copy"]);
            }
            command
                .arg("-/bsf:v")
                .arg(&bsf)
                .args([
                    "-video_track_timescale",
                    "1000000",
                    "-movflags",
                    "+faststart",
                ])
                .arg(&temporary);
            let prepared = (|| {
                run(&mut command)?;
                let times = plan.verify(&probe(&temporary, count, false)?)?;
                fs::rename(&temporary, &result)?;
                frametape::atomic_json(&verified, &times)
            })();
            if prepared.is_err() {
                let _ = fs::remove_file(&temporary);
            }
            prepared?;
        } else {
            let p = probe(&source, count, !plan.copy)?;
            let mut times: Vec<f64> = if plan.copy {
                p["packets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|p| p["pts_time"].as_str()?.parse().ok())
                    .collect()
            } else {
                p["frames"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter_map(|p| p["best_effort_timestamp_time"].as_str()?.parse().ok())
                    .collect()
            };
            times.sort_by(f64::total_cmp);
            frametape::atomic_json(&verified, &times)?;
        }
    }
    let times: Vec<f64> =
        serde_json::from_reader(fs::File::open(verified)?).map_err(io::Error::other)?;
    let mime = if source == media.path {
        plan.original_mime.clone()
    } else {
        "video/mp4".into()
    };
    Ok(Some(Playback {
        path: source,
        mime,
        descriptor: json!({"times":times,"source_offset_ns":plan.source_offset_ns,"duration":plan.duration,"compatible":mode=="compatible"}),
    }))
}
