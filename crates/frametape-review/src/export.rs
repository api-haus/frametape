use base64::{Engine, engine::general_purpose::STANDARD};
use frametape::{Mark, TapeReader};
use serde_json::json;
use std::collections::{BTreeSet, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn cache_key(path: &Path) -> io::Result<String> {
    cache_key_metadata(path, &fs::metadata(path)?)
}

pub fn cache_key_metadata(path: &Path, stat: &fs::Metadata) -> io::Result<String> {
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    stat.len().hash(&mut hash);
    stat.modified()?.hash(&mut hash);
    Ok(format!("{:016x}", hash.finish()))
}

pub fn image(path: &Path, frame: u64, cache: &Path) -> io::Result<Option<PathBuf>> {
    let mut tape = TapeReader::open(path)?;
    let frames = tape.frames()?;
    let frame = frames
        .get(frame as usize)
        .ok_or_else(|| io::Error::other("frame outside recording"))?;
    let Some(index) = frame.video_frame else {
        return Ok(None);
    };
    let Some(media) = crate::media::open(path, cache)? else {
        return Ok(None);
    };
    let output = media
        .path
        .parent()
        .unwrap()
        .join(format!("frame-{index}.png"));
    if !output.exists() {
        let result = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-n", "-threads", "2"])
            .arg("-i")
            .arg(&media.path)
            .args([
                "-vf",
                &format!("select=eq(n\\,{index})"),
                "-fps_mode",
                "passthrough",
                "-frames:v",
                "1",
                "-threads",
                "1",
            ])
            .arg(&output)
            .stdin(Stdio::null())
            .output()?;
        if !result.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&result.stderr).into_owned(),
            ));
        }
        if !output.is_file() {
            return Err(io::Error::other("video ends before mapped picture"));
        }
    }
    Ok(Some(output))
}

fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn burn(
    raw: &Path,
    output: &Path,
    width: u32,
    height: u32,
    marks: impl Iterator<Item = Mark>,
) -> io::Result<()> {
    let mut svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{width}\" height=\"{height}\"><image width=\"{width}\" height=\"{height}\" xlink:href=\"data:image/png;base64,{}\"/>",
        STANDARD.encode(fs::read(raw)?)
    );
    let w = width as f64;
    let h = height as f64;
    let short = w.min(h);
    for mark in marks {
        let [r, g, b] = mark.colour();
        let ink = format!("rgb({r},{g},{b})");
        let stroke = f64::from(mark.width()) * short;
        let style = format!(
            "fill=\"none\" stroke=\"{ink}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"",
            stroke.max(1.0)
        );
        match mark {
            Mark::Rectangle { a, b, .. } => {
                svg += &format!(
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" {style}/>",
                    f64::from(a[0].min(b[0])) * w,
                    f64::from(a[1].min(b[1])) * h,
                    f64::from((b[0] - a[0]).abs()) * w,
                    f64::from((b[1] - a[1]).abs()) * h
                )
            }
            Mark::Ellipse { a, b, .. } => {
                svg += &format!(
                    "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" {style}/>",
                    f64::from((a[0] + b[0]) / 2.0) * w,
                    f64::from((a[1] + b[1]) / 2.0) * h,
                    f64::from((b[0] - a[0]).abs()) * w / 2.0,
                    f64::from((b[1] - a[1]).abs()) * h / 2.0
                )
            }
            Mark::Stroke { points, .. } => {
                let points = points
                    .iter()
                    .map(|p| format!("{},{}", f64::from(p[0]) * w, f64::from(p[1]) * h))
                    .collect::<Vec<_>>()
                    .join(" ");
                svg += &format!("<polyline points=\"{points}\" {style}/>");
            }
            Mark::Arrow { from, to, .. } => {
                let (ax, ay, bx, by) = (
                    f64::from(from[0]) * w,
                    f64::from(from[1]) * h,
                    f64::from(to[0]) * w,
                    f64::from(to[1]) * h,
                );
                let angle = (by - ay).atan2(bx - ax);
                let length = short * 0.03;
                svg += &format!(
                    "<path d=\"M{ax},{ay} L{bx},{by} M{},{} L{bx},{by} L{},{}\" {style}/>",
                    bx - length * (angle - 0.5).cos(),
                    by - length * (angle - 0.5).sin(),
                    bx - length * (angle + 0.5).cos(),
                    by - length * (angle + 0.5).sin()
                );
            }
            Mark::Text { at, text, size, .. } => {
                svg += &format!(
                    "<text x=\"{}\" y=\"{}\" font-family=\"sans-serif\" font-weight=\"600\" font-size=\"{}\" dominant-baseline=\"middle\" fill=\"{ink}\" stroke=\"black\" stroke-width=\"1\" paint-order=\"stroke\">{}</text>",
                    f64::from(at[0]) * w,
                    f64::from(at[1]) * h,
                    f64::from(size) * short,
                    escaped(&text)
                )
            }
        }
    }
    svg += "</svg>";
    let mut options = resvg::usvg::Options::default();
    options.fontdb_mut().load_system_fonts();
    let tree = resvg::usvg::Tree::from_str(&svg, &options).map_err(io::Error::other)?;
    let mut pixels = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| io::Error::other("image size invalid"))?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::identity(),
        &mut pixels.as_mut(),
    );
    pixels.save_png(output).map_err(io::Error::other)
}

pub fn export(
    path: &Path,
    output: &Path,
    requested: &[u64],
    every: Option<u64>,
    list: bool,
) -> io::Result<()> {
    if every == Some(0) {
        return Err(io::Error::other("--every must exceed zero"));
    }
    let mut tape = TapeReader::open(path)?;
    let frames = tape.frames()?;
    let events = tape.events()?;
    let annotations = tape.annotations()?;
    let mut wanted = BTreeSet::new();
    if !requested.is_empty() {
        wanted.extend(requested.iter().copied());
    } else if let Some(every) = every {
        wanted.extend(
            (0..frames.len() as u64).step_by(usize::try_from(every).map_err(io::Error::other)?),
        );
    } else {
        for container in &annotations.containers {
            wanted.extend([
                container.start_frame,
                (container.start_frame + container.end_frame) / 2,
                container.end_frame,
            ]);
        }
    }
    if wanted.iter().any(|f| *f >= frames.len() as u64) {
        return Err(io::Error::other("requested frame exceeds recording"));
    }
    fs::create_dir(output)?;
    let cache = output.join("source");
    let media = if list {
        None
    } else {
        crate::media::open(path, &cache)?
    };
    let mut rows = Vec::new();
    for index in wanted {
        let frame = &frames[index as usize];
        let marks = annotations.marks(index).cloned().collect::<Vec<_>>();
        let mut png = None;
        if !list {
            if let (Some(raw), Some(media)) = (image(path, index, &cache)?, &media) {
                let file = output.join(format!("frame-{index:06}.png"));
                if marks.is_empty() {
                    fs::copy(raw, &file)?;
                } else {
                    burn(&raw, &file, media.width, media.height, marks.into_iter())?;
                }
                png = Some(file.file_name().unwrap().to_string_lossy().into_owned());
            }
        }
        let data = events
            .iter()
            .filter(|event| {
                event.frame == Some(index)
                    || (event.frame.is_none()
                        && frametape::frame_at(&frames, event.timestamp_ns)
                            .is_some_and(|f| f.index == index))
            })
            .collect::<Vec<_>>();
        rows.push(json!({"frame":frame,"image":png,"events":data,"annotations":annotations.covering(index).collect::<Vec<_>>()}));
    }
    frametape::atomic_json(
        &output.join("manifest.json"),
        &json!({"recording":tape.manifest,"source":std::path::absolute(path)?,"annotations":annotations,"frames":rows}),
    )?;
    println!(
        "{}",
        std::path::absolute(output.join("manifest.json"))?.display()
    );
    Ok(())
}
