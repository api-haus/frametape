//! Opt-in native recorder control. Hosts receive intent on their own thread and acknowledge
//! only after recording starts or its archive has finished. No engine or video dependency.
#![cfg(unix)]

use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread::JoinHandle,
    time::Duration,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Start { path: PathBuf, window_title: String },
    Stop { path: PathBuf },
    Status,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Reply {
    pub recording: bool,
    pub path: Option<PathBuf>,
    pub error: Option<String>,
}

pub struct Pending {
    pub request: Request,
    reply: Sender<Reply>,
}
impl Pending {
    pub fn reply(self, reply: Reply) {
        let _ = self.reply.send(reply);
    }
}

/// Owner-only discovery directory, shared by the CLI, Rust hosts and C ABI.
pub fn runtime_dir() -> io::Result<PathBuf> {
    let root = PathBuf::from(
        std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| io::Error::other("XDG_RUNTIME_DIR is required"))?,
    )
    .join("frametape");
    match fs::create_dir(&root) {
        Ok(()) => fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e),
    }
    let metadata = fs::symlink_metadata(&root)?;
    if !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(io::Error::other(
            "Frametape runtime directory must be owned by this user, mode0700, and not a symlink",
        ));
    }
    Ok(root)
}

/// PID birth token prevents a stale shortcut from signalling a reused process ID.
pub fn process_birth(pid: u32) -> io::Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    stat.rsplit_once(") ")
        .and_then(|(_, tail)| tail.split_whitespace().nth(19))
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("invalid process identity"))
}

fn endpoint(pid: u32) -> io::Result<PathBuf> {
    Ok(runtime_dir()?.join(format!("{pid}-{}.sock", process_birth(pid)?)))
}

pub fn available(pid: u32) -> io::Result<bool> {
    Ok(endpoint(pid)?.exists())
}

pub fn call(pid: u32, request: &Request) -> io::Result<Reply> {
    let mut socket = UnixStream::connect(endpoint(pid)?)?;
    socket.set_read_timeout(Some(Duration::from_secs(180)))?;
    socket.set_write_timeout(Some(Duration::from_secs(2)))?;
    serde_json::to_writer(&mut socket, request)?;
    socket.write_all(b"\n")?;
    read_json(&mut socket)
}

fn read_json<T: serde::de::DeserializeOwned>(socket: &mut UnixStream) -> io::Result<T> {
    let mut line = String::new();
    BufReader::new(socket.take(16_385)).read_line(&mut line)?;
    if line.len() > 16_384 || !line.ends_with('\n') {
        return Err(io::Error::other(
            "control message must be newline-terminated JSON below16KiB",
        ));
    }
    serde_json::from_str(&line).map_err(io::Error::other)
}

pub struct ControlHost {
    requests: Receiver<Pending>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    path: PathBuf,
}

impl ControlHost {
    pub fn bind() -> io::Result<Self> {
        let path = endpoint(std::process::id())?;
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let (sender, requests) = mpsc::sync_channel(8);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let thread = std::thread::spawn(move || {
            while !stopping.load(Ordering::Relaxed) {
                let (mut socket, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(_) => break,
                };
                let _ = socket.set_read_timeout(Some(Duration::from_secs(1)));
                let _ = socket.set_write_timeout(Some(Duration::from_secs(1)));
                let result = (|| -> io::Result<Reply> {
                    let request = read_json(&mut socket)?;
                    let (reply, response) = mpsc::channel();
                    sender
                        .try_send(Pending { request, reply })
                        .map_err(io::Error::other)?;
                    for _ in 0..1800 {
                        if stopping.load(Ordering::Relaxed) {
                            break;
                        }
                        match response.recv_timeout(Duration::from_millis(100)) {
                            Ok(reply) => return Ok(reply),
                            Err(mpsc::RecvTimeoutError::Timeout) => (),
                            Err(error) => return Err(io::Error::other(error)),
                        }
                    }
                    Err(io::Error::other(
                        "native recorder did not acknowledge; inspect its retained journal",
                    ))
                })();
                let reply = result.unwrap_or_else(|e| Reply {
                    error: Some(e.to_string()),
                    ..Default::default()
                });
                if serde_json::to_writer(&mut socket, &reply).is_ok() {
                    let _ = socket.write_all(b"\n");
                }
            }
        });
        Ok(Self {
            requests,
            stop,
            thread: Some(thread),
            path,
        })
    }
    pub fn poll(&self) -> Option<Pending> {
        self.requests.try_recv().ok()
    }
}

impl Drop for ControlHost {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let _ = fs::remove_file(&self.path);
    }
}

/// Publish a completed native archive without replacing an existing recording.
pub fn publish(source: &Path, destination: &Path) -> io::Result<()> {
    if source == destination {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut name = destination.as_os_str().to_owned();
    name.push(format!(".{}.publishing", std::process::id()));
    let temp = PathBuf::from(name);
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| {
        io::copy(&mut fs::File::open(source)?, &mut output)?;
        output.sync_all()?;
        fs::hard_link(&temp, destination)
    })();
    let _ = fs::remove_file(temp);
    result
}
