use frametape_video::{EncoderConfig, Quality, Recorder, Sent};
use std::process::Command;

#[test]
fn lossless_rgb_through_ffmpeg_and_odd_frame_padding() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.agents/scratch/video-test")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join("lossless.mp4");
    let config = EncoderConfig {
        quality: Quality::Lossless,
        encoder: Some("libx264rgb".into()),
        ..Default::default()
    };
    let mut recorder = Recorder::start_with(&path, 5, 3, 24.0, &config).unwrap();
    assert!(!recorder.info().hardware);
    assert_eq!(recorder.info().chroma, "RGB");
    let rgba: Vec<u8> = (0..60).map(|value| (value * 37) as u8).collect();
    assert_eq!(recorder.write(rgba.clone()).unwrap(), Sent::Queued);
    assert!(recorder.write(vec![0; 3]).is_err());
    // A host can supply lazy conversion without owning the recorder's writer thread.
    let second = rgba.clone();
    assert_eq!(
        recorder.write_with(Box::new(move || second)).unwrap(),
        Sent::Queued
    );
    recorder.finish().unwrap();

    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(&path)
        .args([
            "-vf",
            "crop=5:3:0:0",
            "-pix_fmt",
            "rgb24",
            "-f",
            "rawvideo",
            "pipe:1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rgb: Vec<u8> = rgba
        .chunks_exact(4)
        .flat_map(|pixel| pixel[..3].iter().copied())
        .collect();
    assert_eq!(output.stdout, [rgb.as_slice(), rgb.as_slice()].concat());
    println!("lossless RGB proof: {}", path.display());
}
