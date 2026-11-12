use crate::export;
use frametape::{Annotations, TapeReader};
use serde_json::{Value, json};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
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

#[derive(Clone)]
struct Library {
    root: PathBuf,
    cache: PathBuf,
}

/// One review instance. Owns its listener and worker; dropping it drains the current request.
pub struct AppServer {
    pub url: String,
    library: Arc<RwLock<Library>>,
    origin: String,
    prefix: String,
    stop: Arc<AtomicBool>,
    server: Arc<Server>,
    worker: Option<JoinHandle<()>>,
}

fn library(path: &Path) -> io::Result<(Library, Option<String>)> {
    let path = fs::canonicalize(path)?;
    let selected = path.is_file().then(|| id(&path));
    let root = if path.is_dir() {
        path
    } else {
        path.parent().unwrap().to_owned()
    };
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .ok_or_else(|| io::Error::other("cache directory unavailable"))?;
    let cache = cache_home.join("frametape").join(id(&root));
    fs::create_dir_all(&cache)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            cache_home.join("frametape"),
            fs::Permissions::from_mode(0o700),
        )?;
    }
    Ok((Library { root, cache }, selected))
}

impl AppServer {
    pub fn start(path: PathBuf, port: u16) -> io::Result<Self> {
        let (initial, selected) = library(&path)?;
        let server = Arc::new(Server::http(("127.0.0.1", port)).map_err(io::Error::other)?);
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
        let shared = Arc::new(RwLock::new(initial));
        let stop = Arc::new(AtomicBool::new(false));
        let (listener, state, stopping, allowed_origin, route_prefix) = (
            server.clone(),
            shared.clone(),
            stop.clone(),
            origin.clone(),
            prefix.clone(),
        );
        let worker = std::thread::spawn(move || {
            for request in listener.incoming_requests() {
                if stopping.load(Ordering::Relaxed) {
                    break;
                }
                let accepted = (|| -> io::Result<()> {
                    let host = request
                        .headers()
                        .iter()
                        .find(|h| h.field.equiv("Host"))
                        .map(|h| h.value.as_str());
                    if host != Some(address.to_string().as_str()) {
                        return reply(request, 403, "text/plain", "invalid host");
                    }
                    if request.method() == &Method::Post
                        && !request
                            .headers()
                            .iter()
                            .any(|h| h.field.equiv("Origin") && h.value.as_str() == allowed_origin)
                    {
                        return reply(request, 403, "text/plain", "invalid origin");
                    }
                    let route = request
                        .url()
                        .split('?')
                        .next()
                        .unwrap_or("")
                        .strip_prefix(&route_prefix)
                        .map(str::to_owned);
                    let Some(route) = route else {
                        return reply(request, 404, "text/plain", "unknown session");
                    };
                    let library = state
                        .read()
                        .map_err(|_| io::Error::other("library state poisoned"))?
                        .clone();
                    handle(request, &route, &library.root, &library.cache)
                })();
                if let Err(error) = accepted {
                    eprintln!("app request: {error}");
                }
            }
        });
        Ok(Self {
            url,
            library: shared,
            origin,
            prefix,
            stop,
            server,
            worker: Some(worker),
        })
    }
    pub fn open(&self, path: &Path) -> io::Result<String> {
        let (next, selected) = library(path)?;
        *self
            .library
            .write()
            .map_err(|_| io::Error::other("library state poisoned"))? = next;
        Ok(format!(
            "{}{}{}",
            self.origin,
            self.prefix,
            selected.map_or(String::new(), |id| format!("?take={id}"))
        ))
    }
}
impl Drop for AppServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.server.unblock();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

pub fn serve(path: PathBuf, port: u16, no_open: bool) -> io::Result<()> {
    let mut app = AppServer::start(path, port)?;
    println!("{}", app.url);
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
            .arg(&app.url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }
    app.worker
        .take()
        .unwrap()
        .join()
        .map_err(|_| io::Error::other("review server worker panicked"))
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
    let asset = match route {
        "entry.js" => Some(include_str!("../../../app/entry.js")),
        "catalog.js" => Some(include_str!("../../../app/catalog.js")),
        "host.js" => Some(include_str!("../../../app/host.js")),
        _ => None,
    };
    if let Some(asset) = asset {
        return reply(request, 200, "text/javascript", asset);
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
    if parts[0] == "download" {
        return request.respond(
            Response::from_file(File::open(&path)?)
                .with_header(header("Content-Type", "application/x-frametape"))
                .with_header(header(
                    "Content-Disposition",
                    "attachment; filename=recording.ftape",
                )),
        );
    }
    if parts[0] == "copy" && request.method() == &Method::Post {
        let result = (|| -> io::Result<PathBuf> {
            let mut data = Vec::new();
            request
                .as_reader()
                .take(8 * 1024 * 1024 + 1)
                .read_to_end(&mut data)?;
            if data.len() > 8 * 1024 * 1024 {
                return Err(io::Error::other("annotation document exceeds8MiB"));
            }
            let data: Value = serde_json::from_slice(&data).map_err(io::Error::other)?;
            let annotations: Annotations =
                serde_json::from_value(data["annotations"].clone()).map_err(io::Error::other)?;
            let mut source = File::open(&path)?;
            if data["revision"].as_str()
                != Some(export::cache_key_metadata(&path, &source.metadata()?)?.as_str())
            {
                return Err(io::Error::other(
                    "Recording changed; reopen before exporting this draft",
                ));
            }
            let mut random = [0u8; 16];
            getrandom::fill(&mut random).map_err(|error| io::Error::other(error.to_string()))?;
            let name = random
                .iter()
                .map(|n| format!("{n:02x}"))
                .collect::<String>();
            let destination = cache.join(format!("copy-{name}.ftape"));
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&destination)?;
            io::copy(&mut source, &mut output)?;
            output.sync_all()?;
            drop(output);
            if let Err(error) = frametape::save_annotations(&destination, &annotations) {
                let _ = fs::remove_file(&destination);
                return Err(error);
            }
            Ok(destination)
        })();
        return match result {
            Ok(copy) => {
                let file = File::open(&copy)?;
                let response = request.respond(
                    Response::from_file(file)
                        .with_header(header("Content-Type", "application/x-frametape")),
                );
                let _ = fs::remove_file(copy);
                response
            }
            Err(error) => reply(request, 422, "text/plain", error.to_string()),
        };
    }
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
                let template = include_str!("../../../app/annotator.html");
                let (before, rest) = template.split_once("__DATA__").unwrap();
                let (middle, after) = rest.split_once("__MARKS__").unwrap();
                let html = format!(
                    "{before}{}{middle}{}{after}",
                    script_json(&data),
                    script_json(&serde_json::to_value(marks).map_err(io::Error::other)?)
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
