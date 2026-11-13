//! Packet timing for an ordinary video decoder, aligned with recorded source frames.
use crate::Frame;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io;

pub const MAX_PICTURES: usize = 1_000_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaybackPlan {
    pub times: Vec<f64>,
    pub source_offset_ns: u64,
    pub duration: f64,
    pub bsf: String,
    pub filter: String,
    pub original_mime: String,
    pub copy: bool,
    pub original: bool,
    pub encode_bsf: String,
    pub tolerance: f64,
}

fn invalid(message: &str) -> io::Error {
    io::Error::other(message)
}

pub fn picture_count(frames: &[Frame]) -> io::Result<usize> {
    let count = frames
        .iter()
        .filter_map(|f| f.video_frame)
        .max()
        .map_or(0, |n| n.saturating_add(1));
    if count == 0 || count > MAX_PICTURES as u64 {
        return Err(invalid(
            "Video picture index is absent or exceeds the review limit",
        ));
    }
    Ok(count as usize)
}

fn lookup(values: &[i64], start: usize) -> String {
    if values.len() == 1 {
        return values[0].to_string();
    }
    let middle = values.len() / 2;
    format!(
        "if(lt(N,{}),{},{})",
        start + middle,
        lookup(&values[..middle], start),
        lookup(&values[middle..], start + middle)
    )
}

fn time_base(probe: &Value) -> io::Result<f64> {
    let value = probe["streams"][0]["time_base"]
        .as_str()
        .ok_or_else(|| invalid("Video has no timebase"))?;
    let (a, b) = value
        .split_once('/')
        .ok_or_else(|| invalid("Invalid video timebase"))?;
    let tb = a
        .parse::<f64>()
        .map_err(|_| invalid("Invalid video timebase"))?
        / b.parse::<f64>()
            .map_err(|_| invalid("Invalid video timebase"))?;
    if !tb.is_finite() || tb <= 0.0 || tb > 1.0 {
        return Err(invalid("Invalid video timebase"));
    }
    Ok(tb)
}

fn coverage_end(probe: &Value) -> Option<f64> {
    let rows = probe["packets"]
        .as_array()
        .or_else(|| probe["frames"].as_array())?;
    let number = |v: &Value| v.as_str()?.parse::<f64>().ok().filter(|n| n.is_finite());
    let mut latest: Option<(f64, &Value)> = None;
    for row in rows {
        let pts = number(&row["pts_time"]).or_else(|| number(&row["best_effort_timestamp_time"]));
        if let Some(pts) = pts {
            if latest.is_none_or(|(old, _)| pts > old) {
                latest = Some((pts, row));
            }
        }
    }
    let (pts, row) = latest?;
    if let Some(duration) = number(&row["duration_time"])
        .or_else(|| number(&row["pkt_duration_time"]))
        .filter(|n| *n > 0.0)
    {
        return Some(pts + duration);
    }
    number(&probe["format"]["duration"]).or_else(|| {
        number(&probe["streams"][0]["duration"])
            .map(|duration| number(&probe["streams"][0]["start_time"]).unwrap_or(0.0) + duration)
    })
}

impl PlaybackPlan {
    /// H.264/HEVC access units are remuxed without decoding. Other codecs index decoded
    /// presentations first, so hidden packets cannot be mistaken for displayed pictures.
    pub fn new(frames: &[Frame], probe: &Value) -> io::Result<Self> {
        let count = picture_count(frames)?;
        let mut source = vec![None; count];
        for f in frames {
            if let Some(i) = f.video_frame {
                if source[i as usize].is_none() {
                    source[i as usize] = Some(f.timestamp_ns);
                }
            }
        }
        let source: Vec<u64> = source
            .into_iter()
            .map(|t| {
                t.ok_or_else(|| {
                    invalid("Video contains a picture without a confirmed source mapping")
                })
            })
            .collect::<io::Result<_>>()?;
        let origin = source[0];
        if source.windows(2).any(|p| p[0] >= p[1]) {
            return Err(invalid(
                "Video pictures must have distinct, increasing source timestamps",
            ));
        }
        let last = frames.last().ok_or_else(|| invalid("No source frames"))?;
        let end = last
            .timestamp_ns
            .checked_add(last.duration_ns)
            .ok_or_else(|| invalid("Source clock overflow"))?;
        if end <= *source.last().unwrap() {
            return Err(invalid("Final video picture has no duration"));
        }
        let times: Vec<f64> = source.iter().map(|t| (*t - origin) as f64 / 1e9).collect();
        let duration = (end - origin) as f64 / 1e9;
        let codec = probe["streams"][0]["codec_name"]
            .as_str()
            .ok_or_else(|| invalid("Video codec is missing"))?;
        let copy = matches!(codec, "h264" | "hevc");
        let tb = if copy { time_base(probe)? } else { 1e-6 };
        let ticks = |seconds: f64| -> io::Result<i64> {
            let n = (seconds / tb).round();
            if !n.is_finite() || n < 0.0 || n > i64::MAX as f64 {
                Err(invalid("Video timestamp exceeds mux range"))
            } else {
                Ok(n as i64)
            }
        };
        let pts: Vec<i64> = times.iter().map(|t| ticks(*t)).collect::<io::Result<_>>()?;
        let end = ticks(duration)?;
        let mut durations: Vec<i64> = pts.windows(2).map(|p| p[1] - p[0]).collect();
        durations.push(end - pts[count - 1]);
        if durations.iter().any(|d| *d <= 0) {
            return Err(invalid("Source pictures collapse in the video timebase"));
        }
        let filter = format!(
            "settb=1/1000000,setpts='{}'",
            lookup(
                &times
                    .iter()
                    .map(|t| (t * 1e6).round() as i64)
                    .collect::<Vec<_>>(),
                0
            )
        );
        let original_times: Vec<f64> = if copy {
            probe["packets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| p["pts_time"].as_str()?.parse::<f64>().ok())
                .collect()
        } else {
            let decoded = probe["frames"]
                .as_array()
                .ok_or_else(|| invalid("Video presentation index is missing"))?;
            if decoded.len() != count {
                return Err(invalid("Decoded picture count differs from source mapping"));
            }
            decoded
                .iter()
                .map(|p| {
                    p["best_effort_timestamp_time"]
                        .as_str()
                        .and_then(|s| s.parse::<f64>().ok())
                        .ok_or_else(|| invalid("Decoded picture has no timestamp"))
                })
                .collect::<io::Result<_>>()?
        };
        let mut original_times = original_times;
        original_times.sort_by(f64::total_cmp);
        let format = probe["format"]["format_name"].as_str().unwrap_or("");
        let original = (!copy || format.contains("mov"))
            && original_times.len() == count
            && original_times.iter().all(|t| t.is_finite())
            && original_times.windows(2).all(|p| p[0] < p[1])
            && probe["format"]["duration"]
                .as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .is_some_and(|container_end| {
                    coverage_end(probe).is_some_and(|video_end| {
                        (container_end - video_end).abs() <= time_base(probe).unwrap_or(1e-6) * 2.0
                    })
                })
            && coverage_end(probe).is_some_and(|end| {
                (end - original_times[0] - duration).abs() <= time_base(probe).unwrap_or(1e-6) * 2.0
            })
            && original_times.iter().zip(&times).all(|(a, b)| {
                (a - original_times[0] - b).abs() <= time_base(probe).unwrap_or(1e-6) * 2.0
            });
        let duration_us: Vec<i64> = times
            .windows(2)
            .map(|p| ((p[1] - p[0]) * 1e6).round() as i64)
            .chain(std::iter::once(
                ((duration - times[count - 1]) * 1e6).round() as i64,
            ))
            .collect();
        if duration_us.iter().any(|d| *d <= 0) {
            return Err(invalid("Source pictures collapse in the playback timebase"));
        }
        let encode_bsf = format!("setts=duration='{}'", lookup(&duration_us, 0));
        let mut bsf = String::new();
        if copy {
            let packets = probe["packets"]
                .as_array()
                .ok_or_else(|| invalid("Video packet index is missing"))?;
            if packets.len() != count {
                return Err(invalid(
                    "Encoded picture count differs from the source mapping",
                ));
            }
            let packet_pts: Vec<Option<f64>> = packets
                .iter()
                .map(|p| p["pts_time"].as_str().and_then(|s| s.parse::<f64>().ok()))
                .collect();
            let mut order: Vec<usize> = (0..count).collect();
            if packet_pts.iter().all(Option::is_some) {
                if packet_pts.iter().flatten().any(|p| !p.is_finite()) {
                    return Err(invalid("Invalid video packet timestamp"));
                }
                order.sort_by(|a, b| packet_pts[*a].unwrap().total_cmp(&packet_pts[*b].unwrap()));
                if order
                    .windows(2)
                    .any(|p| packet_pts[p[0]] == packet_pts[p[1]])
                {
                    return Err(invalid("Duplicate video presentation timestamps"));
                }
            } else if packet_pts.iter().any(Option::is_some)
                || probe["streams"][0]["has_b_frames"].as_u64() != Some(0)
            {
                return Err(invalid("Cannot determine reordered video picture identity"));
            }
            let mut rank = vec![0; count];
            for (picture, packet) in order.into_iter().enumerate() {
                rank[packet] = picture;
            }
            let packet_pts: Vec<i64> = rank.iter().map(|i| pts[*i]).collect();
            let packet_durations: Vec<i64> = rank.iter().map(|i| durations[*i]).collect();
            let mut dts = Vec::with_capacity(count);
            let mut elapsed = 0i64;
            for d in &packet_durations {
                dts.push(elapsed);
                elapsed = elapsed
                    .checked_add(*d)
                    .ok_or_else(|| invalid("Decode clock overflow"))?;
            }
            let shift = packet_pts
                .iter()
                .zip(&dts)
                .map(|(p, d)| p - d)
                .min()
                .unwrap();
            for d in &mut dts {
                *d += shift;
            }
            bsf = format!(
                "setts=pts='{}':dts='{}':duration='{}'",
                lookup(&packet_pts, 0),
                lookup(&dts, 0),
                lookup(&packet_durations, 0)
            );
        }
        Ok(Self {
            times,
            source_offset_ns: origin,
            duration,
            bsf,
            filter,
            original_mime: if format.contains("webm") || format.contains("matroska") {
                "video/webm"
            } else if format.contains("ogg") {
                "video/ogg"
            } else if format.contains("mov") {
                "video/mp4"
            } else {
                "application/octet-stream"
            }
            .into(),
            copy,
            original,
            encode_bsf,
            tolerance: (tb * 2.0).max(2e-6),
        })
    }

    /// Validate the published mux, not merely FFmpeg's exit status. Returned timestamps
    /// are the actual media-element seek positions, including container quantization.
    pub fn verify(&self, probe: &Value) -> io::Result<Vec<f64>> {
        let packets = probe["packets"]
            .as_array()
            .ok_or_else(|| invalid("Prepared video has no packet index"))?;
        let mut times: Vec<f64> = packets
            .iter()
            .map(|p| {
                p["pts_time"]
                    .as_str()
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| invalid("Prepared video has no presentation timestamp"))
            })
            .collect::<io::Result<_>>()?;
        times.sort_by(f64::total_cmp);
        if times.len() != self.times.len() || times.windows(2).any(|p| p[0] >= p[1]) {
            return Err(invalid("Prepared video lost or duplicated pictures"));
        }
        let offset = times[0];
        if times
            .iter()
            .zip(&self.times)
            .any(|(actual, expected)| (actual - offset - expected).abs() > self.tolerance)
        {
            return Err(invalid(
                "Prepared video changed the source presentation clock",
            ));
        }
        if !coverage_end(probe)
            .is_some_and(|end| (end - offset - self.duration).abs() <= self.tolerance)
        {
            return Err(invalid("Prepared video changed the final picture duration"));
        }
        Ok(times)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn frames() -> Vec<Frame> {
        vec![
            Frame {
                index: 0,
                timestamp_ns: 0,
                duration_ns: 100_000_000,
                video_frame: Some(0),
            },
            Frame {
                index: 1,
                timestamp_ns: 100_000_000,
                duration_ns: 900_000_000,
                video_frame: Some(1),
            },
        ]
    }
    fn probe() -> Value {
        json!({"streams":[{"codec_name":"h264","time_base":"1/10000","has_b_frames":0}],"format":{"format_name":"mov,mp4"},"packets":[{"pts_time":"0.0","duration_time":"0.1"},{"pts_time":"0.1","duration_time":"0.1"}]})
    }
    #[test]
    fn shorter_last_picture_is_not_reused_or_published() {
        let plan = PlaybackPlan::new(&frames(), &probe()).unwrap();
        assert!(!plan.original);
        assert!(plan.verify(&probe()).is_err());
    }
    #[test]
    fn mux_timebase_cannot_collapse_pictures() {
        let mut f = frames();
        f[1].timestamp_ns = 1;
        assert!(PlaybackPlan::new(&f, &probe()).is_err());
    }
}
