use crate::export;
use frametape::{Annotations, TapeReader};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

fn id(path: &Path) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

fn recordings(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_file() && entry.path().extension().is_some_and(|e| e == "ftape") {
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

fn revision(path: &Path) -> io::Result<String> {
    export::cache_key(path)
}
fn script_json(value: &Value) -> String {
    value
        .to_string()
        .replace('<', "\\u003c")
        .replace('&', "\\u0026")
}
fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name, value).unwrap()
}

fn reply(request: Request, status: u16, mime: &str, body: impl Into<Vec<u8>>) -> io::Result<()> {
    request.respond(Response::from_data(body).with_status_code(StatusCode(status))
        .with_header(header("Content-Type",mime)).with_header(header("Cache-Control","no-store"))
        .with_header(header("X-Content-Type-Options","nosniff"))
        .with_header(header("Referrer-Policy","no-referrer"))
        .with_header(header("Content-Security-Policy","default-src 'self' data: blob:; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; frame-ancestors 'self'")))
}

fn json_reply(request: Request, value: &Value) -> io::Result<()> {
    reply(request, 200, "application/json", value.to_string())
}

fn file_reply(request: Request, path: &Path, mime: &str) -> io::Result<()> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let range = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .map(|h| h.value.as_str().to_owned());
    let (start, end, status) = if let Some(range) = range {
        let parsed = range
            .strip_prefix("bytes=")
            .and_then(|r| r.split_once('-'))
            .and_then(|(s, e)| {
                let start = s.parse::<u64>().ok()?;
                let end = if e.is_empty() {
                    length.checked_sub(1)?
                } else {
                    e.parse().ok()?
                };
                (start <= end && end < length).then_some((start, end))
            });
        let Some((start, end)) = parsed else {
            return reply(request, 416, "text/plain", "invalid byte range");
        };
        (start, end, 206)
    } else {
        (0, length.saturating_sub(1), 200)
    };
    file.seek(SeekFrom::Start(start))?;
    let count = if length == 0 { 0 } else { end - start + 1 };
    let mut headers = vec![
        header("Content-Type", mime),
        header("Accept-Ranges", "bytes"),
        header("X-Content-Type-Options", "nosniff"),
    ];
    if status == 206 {
        headers.push(header(
            "Content-Range",
            &format!("bytes {start}-{end}/{length}"),
        ));
    }
    request.respond(Response::new(
        StatusCode(status),
        headers,
        file.take(count),
        Some(usize::try_from(count).map_err(io::Error::other)?),
        None,
    ))
}

pub fn serve(path: PathBuf, port: u16, no_open: bool) -> io::Result<()> {
    let path = fs::canonicalize(path)?;
    let selected = path.is_file().then(|| id(&path));
    let root = if path.is_dir() {
        path
    } else {
        path.parent().unwrap().to_owned()
    };
    let cache = root.join(".frametape-cache");
    fs::create_dir_all(&cache)?;
    let server = Server::http(("127.0.0.1", port)).map_err(io::Error::other)?;
    let address = server
        .server_addr()
        .to_ip()
        .ok_or_else(|| io::Error::other("missing bound address"))?;
    let origin = format!("http://{address}");
    let mut random = [0u8; 24];
    getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
    let token = random
        .iter()
        .map(|n| format!("{n:02x}"))
        .collect::<String>();
    let prefix = format!("/{token}/");
    let url = format!(
        "{origin}{prefix}{}",
        selected.map_or(String::new(), |id| format!("?take={id}"))
    );
    println!("{url}");
    if !no_open {
        #[cfg(target_os = "macos")]
        let mut opener = Command::new("open");
        #[cfg(windows)]
        let mut opener = {
            let mut command = Command::new("cmd");
            command.args(["/C", "start", ""]);
            command
        };
        #[cfg(all(not(windows), not(target_os = "macos")))]
        let mut opener = Command::new("xdg-open");
        opener
            .arg(&url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }
    for request in server.incoming_requests() {
        let host = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Host"))
            .map(|h| h.value.as_str());
        if host != Some(address.to_string().as_str()) {
            reply(request, 403, "text/plain", "invalid host")?;
            continue;
        }
        if request.method() == &Method::Post
            && !request
                .headers()
                .iter()
                .any(|h| h.field.equiv("Origin") && h.value.as_str() == origin)
        {
            reply(request, 403, "text/plain", "invalid origin")?;
            continue;
        }
        let route = request
            .url()
            .split('?')
            .next()
            .unwrap_or("")
            .strip_prefix(&prefix)
            .map(str::to_owned);
        let Some(route) = route else {
            reply(request, 404, "text/plain", "unknown session")?;
            continue;
        };
        if let Err(error) = handle(request, &route, &root, &cache) {
            eprintln!("app request: {error}");
        }
    }
    Ok(())
}

fn handle(mut request: Request, route: &str, root: &Path, cache: &Path) -> io::Result<()> {
    if request.method() != &Method::Get && request.method() != &Method::Post {
        return reply(request, 405, "text/plain", "method not allowed");
    }
    if route.is_empty() {
        return reply(
            request,
            200,
            "text/html",
            include_str!("../../../app/catalog.html"),
        );
    }
    if route == "catalog" {
        let mut rows = Vec::new();
        for path in recordings(root)? {
            let mut entry = json!({"id":id(&path),"name":path.file_name().unwrap().to_string_lossy(),"bytes":0,"modified_ms":0});
            let inspected = (|| -> io::Result<()> {
                let stat = fs::metadata(&path)?;
                entry["bytes"] = json!(stat.len());
                entry["modified_ms"] = json!(
                    stat.modified()?
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_err(io::Error::other)?
                        .as_millis()
                );
                let mut tape = TapeReader::open(&path)?;
                let annotations = tape.annotations()?;
                entry["manifest"] =
                    serde_json::to_value(&tape.manifest).map_err(io::Error::other)?;
                entry["labels"] = json!(
                    annotations
                        .containers
                        .iter()
                        .flat_map(|c| c.labels())
                        .collect::<Vec<_>>()
                );
                Ok(())
            })();
            if let Err(error) = inspected {
                entry["error"] = json!(error.to_string());
            }
            rows.push(entry);
        }
        return json_reply(request, &json!(rows));
    }
    let parts = route.split('/').collect::<Vec<_>>();
    if parts.len() < 2 {
        return reply(request, 404, "text/plain", "unknown route");
    }
    let Some(path) = recordings(root)?.into_iter().find(|p| id(p) == parts[1]) else {
        return reply(
            request,
            404,
            "text/plain",
            "recording not found; refresh catalog",
        );
    };
    let result = (|| -> io::Result<(u16, String, Vec<u8>)> {
        let mut tape = TapeReader::open(&path)?;
        match parts[0] {
            "take" => {
                let frames = tape.frames()?;
                Ok((200,"application/json".into(),serde_json::to_vec(&json!({"manifest":tape.manifest,"frames":frames,"annotations":tape.annotations()?,"revision":revision(&path)?})).map_err(io::Error::other)?))
            }
            "review" => {
                let frames = tape.frames()?;
                let marks = tape.annotations()?;
                let data = json!({"stem":path.file_stem().unwrap().to_string_lossy(),"id":parts[1],"frames":frames.len(),"timeline":frames,"revision":revision(&path)?,"fps":tape.manifest.metadata["video_fps"].as_f64().unwrap_or(60.0)});
                let html = include_str!("../../../app/annotator.html")
                    .replace("__DATA__", &script_json(&data))
                    .replace(
                        "__MARKS__",
                        &script_json(&serde_json::to_value(marks).map_err(io::Error::other)?),
                    );
                Ok((200, "text/html".into(), html.into_bytes()))
            }
            "events" => {
                let frame = parts.get(2).and_then(|n| n.parse::<u64>().ok());
                let frames = tape.frames()?;
                let events = tape
                    .events()?
                    .into_iter()
                    .filter(|e| {
                        frame.is_none_or(|frame| {
                            e.frame == Some(frame)
                                || (e.frame.is_none()
                                    && frametape::frame_at(&frames, e.timestamp_ns)
                                        .is_some_and(|f| f.index == frame))
                        })
                    })
                    .collect::<Vec<_>>();
                Ok((
                    200,
                    "application/json".into(),
                    serde_json::to_vec(&events).map_err(io::Error::other)?,
                ))
            }
            "annotations" if request.method() == &Method::Post => {
                let mut data = Vec::new();
                request
                    .as_reader()
                    .take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut data)?;
                if data.len() > 8 * 1024 * 1024 {
                    return Err(io::Error::other("annotation document exceeds 8 MiB"));
                }
                let data: Value = serde_json::from_slice(&data).map_err(io::Error::other)?;
                let annotations: Annotations = serde_json::from_value(data["annotations"].clone())
                    .map_err(io::Error::other)?;
                let mut stale = false;
                let saved = frametape::edit_annotations(&path, |_| {
                    if data["revision"].as_str() != Some(revision(&path)?.as_str()) {
                        stale = true;
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "annotation revision changed",
                        ));
                    }
                    Ok(annotations)
                });
                if stale {
                    return Ok((
                        409,
                        "text/plain".into(),
                        b"Recording changed in another editor. Reload before saving.".to_vec(),
                    ));
                }
                saved?;
                Ok((
                    200,
                    "application/json".into(),
                    serde_json::to_vec(&json!({"revision":revision(&path)?}))
                        .map_err(io::Error::other)?,
                ))
            }
            _ => Ok((404, "text/plain".into(), b"unknown route".to_vec())),
        }
    })();
    // File endpoints stay streamed and retain browser byte-range support.
    if parts[0] == "frame" || parts[0] == "preview" || parts[0] == "video" {
        let index = parts
            .get(2)
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let rendered = if parts[0] == "video" {
            preview(&path, cache)
        } else {
            export::image(&path, index, cache)
        };
        return match rendered {
            Ok(Some(file)) => file_reply(
                request,
                &file,
                if parts[0] == "video" {
                    "video/mp4"
                } else {
                    "image/png"
                },
            ),
            Ok(None) => reply(
                request,
                404,
                "text/plain",
                "no captured image for this frame",
            ),
            Err(error) => reply(request, 422, "text/plain", error.to_string()),
        };
    }
    match result {
        Ok((status, mime, body)) => reply(request, status, &mime, body),
        Err(error) => reply(request, 422, "text/plain", error.to_string()),
    }
}

fn preview(path: &Path, cache: &Path) -> io::Result<Option<PathBuf>> {
    let Some(media) = export::media(path, cache)? else {
        return Ok(None);
    };
    let out = media.path.parent().unwrap().join("preview.mp4");
    if !out.exists() {
        let mut command = Command::new("ffmpeg");
        command.args(["-hide_banner", "-loglevel", "error", "-n", "-threads", "2"]);
        if media.path.extension().is_some_and(|e| e == "h264") {
            command.args(["-r", &media.fps.to_string()]);
        }
        let result = command
            .arg("-i")
            .arg(&media.path)
            .args([
                "-vf",
                "scale=min(1280\\,iw):-2,format=yuv420p",
                "-c:v",
                "libx264",
                "-preset",
                "veryfast",
                "-crf",
                "18",
                "-threads",
                "2",
                "-movflags",
                "+faststart",
            ])
            .arg(&out)
            .stdin(Stdio::null())
            .output()?;
        if !result.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&result.stderr).into_owned(),
            ));
        }
    }
    Ok(Some(out))
}
