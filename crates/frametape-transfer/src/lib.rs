mod outbox;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

pub const HOST: &str = "tape.api.haus";
pub const CHUNK: usize = 1024 * 1024;
pub const MAX_MEMBER: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
pub struct Member {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Progress {
    Connecting,
    Streaming { bytes: u64 },
    Paused { error: String },
    Finalizing,
    Confirmed { url: String },
    Failed { error: String },
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub fn member_name(encoded: &str) -> Option<String> {
    if encoded.len() > 1024 {
        return None;
    }
    let name = String::from_utf8(hex::decode(encoded).ok()?).ok()?;
    valid_member(&name).then_some(name)
}

pub fn valid_member(name: &str) -> bool {
    name.len() <= 512
        && (matches!(
            name,
            "manifest.json" | "frames.jsonl" | "events.jsonl" | "annotations.json"
        ) || name.starts_with("attachments/"))
        && name
            .split('/')
            .all(|part| !matches!(part, "" | "." | "..") && !part.contains(['\\', ':', '\0']))
}

pub fn link_id(url: &str) -> io::Result<&str> {
    url.strip_prefix("https://tape.api.haus/?take=")
        .filter(|id| valid_id(id))
        .ok_or_else(|| io::Error::other("expected a tape.api.haus recording link"))
}

pub fn link(id: &str) -> String {
    format!("https://{HOST}/?take={id}")
}

pub fn digest_bytes(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

pub fn digest_file(file: &mut File) -> io::Result<String> {
    let mut hash = Sha256::new();
    io::copy(file, &mut hash)?;
    Ok(hex::encode(hash.finalize()))
}

pub fn unique_recording_id() -> io::Result<String> {
    let mut id = [0; 16];
    getrandom::fill(&mut id).map_err(|error| io::Error::other(error.to_string()))?;
    Ok(hex::encode(id))
}

pub fn id_for(recording_id: &str) -> String {
    hex::encode(&Sha256::digest(recording_id.as_bytes())[..16])
}

pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("frametape/portal.sock"))
}

#[derive(Clone)]
enum Route {
    Local(PathBuf),
    Lan { ip: Ipv4Addr, token: String },
}

#[derive(Clone)]
pub struct Client(Route);

impl Client {
    pub fn configured() -> io::Result<Option<Self>> {
        let ip = std::env::var("FRAMETAPE_PORTAL_LAN_IP").ok();
        if let Some(ip) = ip {
            let ip: Ipv4Addr = ip.parse().map_err(io::Error::other)?;
            if !ip.is_private() {
                return Err(io::Error::other("portal address must be private LAN IPv4"));
            }
            let token = std::env::var("FRAMETAPE_PORTAL_TOKEN").map_err(io::Error::other)?;
            if !valid_token(&token) {
                return Err(io::Error::other("invalid portal token"));
            }
            return Ok(Some(Self(Route::Lan { ip, token })));
        }
        Ok(socket_path()
            .filter(|path| path.exists())
            .map(|path| Self(Route::Local(path))))
    }

    pub fn owner() -> io::Result<Self> {
        socket_path()
            .filter(|path| path.exists())
            .map(|path| Self(Route::Local(path)))
            .ok_or_else(|| io::Error::other("local Frametape owner socket is unavailable"))
    }

    pub fn require() -> io::Result<Self> {
        Self::configured()?.ok_or_else(|| io::Error::other("Frametape portal is unavailable"))
    }

    fn request(
        &self,
        method: &str,
        path: &str,
        bytes: &[u8],
        output: Option<&Path>,
    ) -> io::Result<Vec<u8>> {
        let mut cmd = Command::new("curl");
        cmd.args([
            "-q",
            "--noproxy",
            "*",
            "--fail",
            "--silent",
            "--show-error",
            "--connect-timeout",
            "2",
            "--max-time",
            if output.is_some() {
                "600"
            } else if method == "POST" {
                "120"
            } else {
                "8"
            },
            "--request",
            method,
        ]);
        match &self.0 {
            Route::Local(socket) => {
                cmd.arg("--unix-socket")
                    .arg(socket)
                    .arg(format!("http://localhost/{path}"));
            }
            Route::Lan { ip, token } => {
                cmd.env("FRAMETAPE_PORTAL_TOKEN", token)
                    .args([
                        "--resolve",
                        &format!("{HOST}:443:{ip}"),
                        "--proto",
                        "=https",
                        "--variable",
                        "%FRAMETAPE_PORTAL_TOKEN",
                        "--expand-header",
                        "Authorization: Bearer {{FRAMETAPE_PORTAL_TOKEN}}",
                    ])
                    .arg(format!("https://{HOST}/{path}"));
            }
        }
        if method != "GET" {
            cmd.args([
                "--header",
                "Content-Type: application/octet-stream",
                "--data-binary",
                "@-",
            ]);
        }
        if let Some(output) = output {
            cmd.arg("--output").arg(output);
        }
        let mut child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let written = child.stdin.take().unwrap().write_all(bytes);
        let reply = child.wait_with_output()?;
        if !reply.status.success() {
            return Err(io::Error::other(
                String::from_utf8_lossy(&reply.stderr).trim(),
            ));
        }
        written?;
        Ok(reply.stdout)
    }

    pub fn inspect(&self, id: &str, frame: Option<u64>) -> io::Result<Vec<u8>> {
        if !valid_id(id) {
            return Err(io::Error::other("invalid portal recording ID"));
        }
        let path = frame.map_or_else(
            || format!("summary/{id}"),
            |frame| format!("events/{id}/{frame}"),
        );
        self.request("GET", &path, &[], None)
    }

    pub fn fetch(&self, id: &str, output: &Path) -> io::Result<()> {
        if !valid_id(id) {
            return Err(io::Error::other("invalid portal recording ID"));
        }
        let temporary = output.with_extension("ftape.fetching");
        let mut options = File::options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let initial = options.open(&temporary)?;
        drop(initial);
        let result = self
            .request("GET", &format!("download/{id}"), &[], Some(&temporary))
            .and_then(|_| std::fs::hard_link(&temporary, output));
        let removed = std::fs::remove_file(temporary);
        result.and(removed)
    }
}

pub fn valid_token(value: &str) -> bool {
    value.len() >= 32
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}

#[derive(Clone)]
pub struct Status(Arc<Mutex<Progress>>);

impl Status {
    pub fn get(&self) -> Progress {
        self.0.lock().unwrap().clone()
    }

    pub fn wait(&self) -> Progress {
        loop {
            let current = self.get();
            if matches!(
                current,
                Progress::Confirmed { .. } | Progress::Failed { .. }
            ) {
                return current;
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    fn set(&self, value: Progress) {
        *self.0.lock().unwrap() = value;
    }
}

enum Update {
    Source(String, PathBuf),
    Finish(File, Option<outbox::Pending>),
}

pub struct Transfer {
    send: Option<mpsc::Sender<Update>>,
    status: Status,
    id: String,
}

impl Transfer {
    pub fn start(recording_id: &str) -> Option<Self> {
        let client = Client::configured().transpose()?;
        let status = Status(Arc::new(Mutex::new(Progress::Connecting)));
        let report = status.clone();
        let (send, receive) = mpsc::channel();
        let id = id_for(recording_id);
        let worker_id = id.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("frametape upload".into())
            .spawn(move || {
                let result = client
                    .and_then(|client| Upload::new(client, worker_id, report.clone()).run(receive));
                if let Err(error) = result {
                    report.set(Progress::Failed {
                        error: error.to_string(),
                    });
                }
            })
        {
            status.set(Progress::Failed {
                error: error.to_string(),
            });
        }
        Some(Self {
            send: Some(send),
            status,
            id,
        })
    }

    pub fn status(&self) -> Status {
        self.status.clone()
    }

    pub fn source(&self, name: &str, path: &Path) {
        if valid_member(name)
            && let Some(send) = &self.send
        {
            let _ = send.send(Update::Source(name.into(), path.into()));
        }
    }

    pub fn finish(mut self, archive: &Path) {
        match File::open(archive) {
            Ok(file) => {
                if let Some(send) = self.send.take() {
                    self.status.set(Progress::Finalizing);
                    let pending = match outbox::Pending::register(archive, &self.id) {
                        Ok(pending) => Some(pending),
                        Err(error) => {
                            eprintln!(
                                "Frametape could not queue the local archive for retry: {error}"
                            );
                            None
                        }
                    };
                    if send.send(Update::Finish(file, pending)).is_err() {
                        self.status.set(Progress::Failed {
                            error: "upload worker is unavailable".into(),
                        });
                    }
                }
            }
            Err(error) => self.status.set(Progress::Failed {
                error: error.to_string(),
            }),
        }
    }
}

struct Upload {
    client: Client,
    id: String,
    status: Status,
    sent: HashMap<(String, u64), (usize, [u8; 32])>,
    bytes: u64,
}

impl Upload {
    fn new(client: Client, id: String, status: Status) -> Self {
        Self {
            client,
            id,
            status,
            sent: HashMap::new(),
            bytes: 0,
        }
    }

    fn run(mut self, receive: mpsc::Receiver<Update>) -> io::Result<()> {
        let mut sources = HashMap::<String, (PathBuf, u64)>::new();
        loop {
            match receive.recv_timeout(Duration::from_secs(1)) {
                Ok(Update::Source(name, path)) => {
                    sources.insert(name, (path, 0));
                }
                Ok(Update::Finish(file, pending)) => {
                    self.status.set(Progress::Finalizing);
                    self.archive(file)?;
                    if let Some(pending) = pending
                        && let Err(error) = pending.complete()
                    {
                        eprintln!("Frametape outbox cleanup: {error}");
                    }
                    self.status.set(Progress::Confirmed {
                        url: link(&self.id),
                    });
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    for (name, (path, offset)) in &mut sources {
                        if let Err(error) = self.live(name, path, offset) {
                            self.status.set(Progress::Paused {
                                error: error.to_string(),
                            });
                            break;
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::other("recording ended without a sealed bundle"));
                }
            }
        }
    }

    fn live(&mut self, name: &str, path: &Path, offset: &mut u64) -> io::Result<()> {
        use std::io::{Seek, SeekFrom};
        let Ok(mut file) = File::open(path) else {
            return Ok(());
        };
        let end = file.metadata()?.len();
        file.seek(SeekFrom::Start(*offset))?;
        let mut buffer = Vec::new();
        for _ in 0..16 {
            let count = end.saturating_sub(*offset).min(CHUNK as u64);
            if count == 0 {
                break;
            }
            buffer.clear();
            Read::by_ref(&mut file)
                .take(count)
                .read_to_end(&mut buffer)?;
            if buffer.len() as u64 != count {
                break;
            }
            self.chunk(name, *offset, &buffer)?;
            self.status.set(Progress::Streaming { bytes: self.bytes });
            if count < CHUNK as u64 {
                break;
            }
            *offset += count;
        }
        Ok(())
    }

    fn chunk(&mut self, name: &str, offset: u64, bytes: &[u8]) -> io::Result<()> {
        let key = (name.into(), offset);
        let digest = Sha256::digest(bytes).into();
        let from = match self.sent.get(&key) {
            Some(&(_, previous)) if digest == previous => return Ok(()),
            Some(&(length, previous))
                if length < bytes.len()
                    && Sha256::digest(&bytes[..length]).as_slice() == previous =>
            {
                length
            }
            _ => 0,
        };
        self.client.request(
            "PUT",
            &format!(
                "ingest/{}/{}/{}",
                self.id,
                hex::encode(name),
                offset + from as u64
            ),
            &bytes[from..],
            None,
        )?;
        self.sent.insert(key, (bytes.len(), digest));
        self.bytes += (bytes.len() - from) as u64;
        Ok(())
    }

    fn archive(&mut self, archive: File) -> io::Result<()> {
        let mut zip = zip::ZipArchive::new(archive)?;
        let mut members = Vec::new();
        let mut buffer = Vec::with_capacity(CHUNK);
        for index in 0..zip.len() {
            let mut file = zip.by_index(index)?;
            let name = file.name().to_owned();
            if !valid_member(&name) || file.size() > MAX_MEMBER {
                return Err(io::Error::other("invalid portal member"));
            }
            let mut offset = 0;
            let mut hash = Sha256::new();
            loop {
                buffer.clear();
                file.by_ref().take(CHUNK as u64).read_to_end(&mut buffer)?;
                if buffer.is_empty() {
                    break;
                }
                hash.update(&buffer);
                self.chunk(&name, offset, &buffer)?;
                offset += buffer.len() as u64;
            }
            members.push(Member {
                name,
                bytes: offset,
                sha256: hex::encode(hash.finalize()),
            });
        }
        self.client.request(
            "POST",
            &format!("ingest/{}/commit", self.id),
            &serde_json::to_vec(&members)?,
            None,
        )?;
        Ok(())
    }
}

pub fn upload_file(path: &Path, recording_id: &str) -> io::Result<String> {
    let id = id_for(recording_id);
    let status = Status(Arc::new(Mutex::new(Progress::Finalizing)));
    Upload::new(Client::require()?, id.clone(), status).archive(File::open(path)?)?;
    Ok(link(&id))
}

pub fn retry_pending() -> io::Result<()> {
    let client = Client::require()?;
    outbox::retry(|id, file| {
        let status = Status(Arc::new(Mutex::new(Progress::Finalizing)));
        Upload::new(client.clone(), id.into(), status).archive(file)?;
        println!("{}", link(id));
        Ok(())
    })
}
