use frametape::{Channel, Frame, TapeWriter};
use serde_json::json;
use std::{path::Path, process::Command};

fn run(command: &mut Command) -> Vec<u8> {
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}
fn hashes(path: &Path) -> Vec<String> {
    String::from_utf8(run(Command::new("ffmpeg")
        .args(["-v", "error", "-threads", "2", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:v:0",
            "-fps_mode",
            "passthrough",
            "-f",
            "framemd5",
            "-",
        ])))
    .unwrap()
    .lines()
    .filter(|s| !s.starts_with('#'))
    .map(|s| s.rsplit(',').next().unwrap().trim().to_owned())
    .collect()
}

fn check(raw: bool) {
    let name = if raw { "raw" } else { "reordered" };
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.agents/scratch/playback-test")
        .join(format!("{}-{name}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let video = root.join(if raw { "source.h264" } else { "source.mp4" });
    run(Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-n",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x180:rate=12:duration=1",
            "-c:v",
            "libx264",
            "-bf",
            if raw { "0" } else { "2" },
            "-g",
            "12",
            "-threads",
            "1",
        ])
        .arg(&video));
    let tape = root.join("capture.ftape");
    let mut writer = TapeWriter::create(root.join("journal"), json!({"video_fps":12})).unwrap();
    writer
        .register(Channel {
            id: "video".into(),
            kind: "video".into(),
            schema: "video/mp4".into(),
        })
        .unwrap();
    let times = [
        0u64, 10, 80, 140, 150, 210, 230, 280, 320, 400, 450, 500, 650,
    ];
    for i in 0..12 {
        writer
            .frame(&Frame {
                index: i as u64,
                timestamp_ns: times[i] * 1_000_000,
                duration_ns: (times[i + 1] - times[i]) * 1_000_000,
                video_frame: Some(i as u64),
            })
            .unwrap();
    }
    writer
        .attach(
            "video",
            video.file_name().unwrap().to_str().unwrap(),
            &video,
            if raw { "video/h264" } else { "video/mp4" },
        )
        .unwrap();
    writer.finish(&tape).unwrap();
    let before = std::fs::read(&tape).unwrap();
    let prepared = frametape_review::media::playback(&tape, &root.join("cache"), false)
        .unwrap()
        .unwrap();
    let actual = prepared.descriptor["times"].as_array().unwrap();
    assert_eq!(actual.len(), 12);
    let first = actual[0].as_f64().unwrap();
    for (i, time) in actual.iter().enumerate() {
        assert!((time.as_f64().unwrap() - first - times[i] as f64 / 1000.0).abs() < 0.001);
    }
    assert_eq!(hashes(&video), hashes(&prepared.path));
    assert_eq!(before, std::fs::read(&tape).unwrap());
    let compatible = frametape_review::media::playback(&tape, &root.join("cache"), true)
        .unwrap()
        .unwrap();
    assert_eq!(compatible.descriptor["compatible"], true);
    let compatible_times = compatible.descriptor["times"].as_array().unwrap();
    assert_eq!(compatible_times.len(), 12);
    for (i, time) in compatible_times.iter().enumerate() {
        assert!((time.as_f64().unwrap() - times[i] as f64 / 1000.0).abs() < 0.001);
    }
    let info: serde_json::Value = serde_json::from_slice(&run(Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=width,height,duration",
            "-of",
            "json",
        ])
        .arg(&compatible.path)))
    .unwrap();
    assert_eq!(info["streams"][0]["width"], 320);
    assert_eq!(info["streams"][0]["height"], 180);
    let duration = info["streams"][0]["duration"]
        .as_str()
        .unwrap()
        .parse::<f64>()
        .unwrap();
    assert!((duration - 0.650).abs() < 0.001);
    let loaded = frametape_review::media::playback(&tape, &root.join("cache"), false)
        .unwrap()
        .unwrap();
    assert_eq!(prepared.descriptor, loaded.descriptor);
}
#[test]
fn reordered_packets_preserve_pictures_and_source_clock() {
    check(false)
}
#[test]
fn elementary_h264_becomes_seekable_without_reencoding() {
    check(true)
}
