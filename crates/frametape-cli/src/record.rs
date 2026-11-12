use crate::{
    desktop,
    window::{self, Window},
};
use clap::{Args, ValueEnum};
use frametape_control::{Request, call, process_birth, runtime_dir};
use frametape_desktop::CaptureTarget;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn checked_call(pid: u32, request: &Request) -> io::Result<frametape_control::Reply> {
    let reply = call(pid, request)?;
    if let Some(error) = &reply.error {
        Err(io::Error::other(error.clone()))
    } else {
        Ok(reply)
    }
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
pub enum Native {
    Auto,
    Required,
    Off,
}

#[derive(Args)]
pub struct Options {
    /// Destination; defaults to Videos/Frametape/<application>-<timestamp>.ftape.
    pub path: Option<PathBuf>,
    /// Exact compositor app class or window title.
    #[arg(long, conflicts_with_all = ["pid", "window"])]
    pub app: Option<String>,
    #[arg(long, conflicts_with = "window")]
    pub pid: Option<u32>,
    #[arg(long)]
    pub window: Option<String>,
    /// Prefer a registered native recorder; require it, or force isolated window capture.
    #[arg(long, value_enum, default_value = "auto")]
    pub native: Native,
    #[command(flatten)]
    pub capture: desktop::Options,
    /// Launch an app, wait for its window, then record it: frametape record -- PROGRAM ARGS.
    #[arg(last = true)]
    pub command: Vec<String>,
}

#[derive(Serialize, Deserialize)]
struct Session {
    pid: u32,
    birth: String,
    path: PathBuf,
    target: Option<Window>,
    native: bool,
    target_birth: Option<String>,
}

struct Lease {
    _file: File,
    state: PathBuf,
    preserve: bool,
}
impl Drop for Lease {
    fn drop(&mut self) {
        if !self.preserve {
            let _ = fs::remove_file(&self.state);
        }
    }
}

fn lease() -> io::Result<Lease> {
    let root = runtime_dir()?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(root.join("record.lock"))?;
    file.try_lock()
        .map_err(|_| io::Error::other("a Frametape recording is already running or starting"))?;
    Ok(Lease {
        _file: file,
        state: root.join("record.json"),
        preserve: false,
    })
}

fn recover_native(session: &Session) -> io::Result<bool> {
    let Some(target) = session.target.as_ref().filter(|_| session.native) else {
        return Ok(false);
    };
    if !process_birth(target.pid).is_ok_and(|birth| Some(birth) == session.target_birth) {
        return Ok(false);
    }
    let status = call(target.pid, &Request::Status)?;
    if status.path.as_ref() != Some(&session.path) {
        return Ok(false);
    }
    if status.recording {
        let reply = checked_call(
            target.pid,
            &Request::Stop {
                path: session.path.clone(),
            },
        )?;
        if reply.recording || reply.path.as_ref() != Some(&session.path) || !session.path.is_file()
        {
            return Err(io::Error::other(
                "native take has not finished; session retained",
            ));
        }
        println!("{}", session.path.display());
        return Ok(true);
    }
    if status.error.is_some() {
        let receipt = session
            .path
            .with_extension(format!("failed-{}-{}.json", session.pid, session.birth));
        frametape::atomic_json(
            &receipt,
            &serde_json::json!({"session":session,"native_result":status}),
        )?;
        eprintln!(
            "previous native take failed; recovery receipt: {}",
            receipt.display()
        );
        return Ok(false);
    }
    if !session.path.is_file() {
        return Err(io::Error::other(
            "native take is still finalizing; session retained",
        ));
    }
    Ok(false)
}

pub fn status() -> io::Result<()> {
    let path = runtime_dir()?.join("record.json");
    let value = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other)?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => serde_json::json!({"recording":false}),
        Err(e) => return Err(e),
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

pub fn toggle(options: Options) -> io::Result<()> {
    let state = runtime_dir()?.join("record.json");
    if let Ok(bytes) = fs::read(&state) {
        let session: Session = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        if process_birth(session.pid).is_ok_and(|birth| birth == session.birth) {
            signal_stop(&session)?;
            println!(
                "stopping {}; archive is published after encoding finishes",
                session.path.display()
            );
            return Ok(());
        }
        // Re-read after taking the stable lease; another invocation may have replaced the session.
        let mut lease = lease()?;
        lease.preserve = true;
        let current: Session =
            serde_json::from_slice(&fs::read(&lease.state)?).map_err(io::Error::other)?;
        let recovered = recover_native(&current)?;
        lease.preserve = false;
        drop(lease);
        if recovered {
            return Ok(());
        }
    }
    record(options)
}

fn signal_stop(session: &Session) -> io::Result<()> {
    use std::os::fd::{AsRawFd, FromRawFd};
    let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, session.pid, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let process = unsafe { File::from_raw_fd(fd as i32) };
    if process_birth(session.pid)? != session.birth {
        return Err(io::Error::other("recording controller process changed"));
    }
    if unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            process.as_raw_fd(),
            libc::SIGINT,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    } != 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn destination(path: Option<PathBuf>, target: Option<&Window>) -> io::Result<PathBuf> {
    let path = match path {
        Some(path) => path,
        None => {
            let videos = Command::new("xdg-user-dir")
                .arg("VIDEOS")
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| PathBuf::from(s.trim()))
                .filter(|p| p.is_absolute());
            let root = videos
                .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Videos")))
                .ok_or_else(|| {
                    io::Error::other("cannot find Videos directory; provide a destination")
                })?;
            let name: String = target
                .map_or("desktop", |w| w.class.as_str())
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .take(80)
                .collect();
            let stamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_err(io::Error::other)?
                .as_nanos();
            let recordings = root.join("Frametape");
            fs::create_dir_all(&root)?;
            use std::os::unix::fs::DirBuilderExt;
            match fs::DirBuilder::new().mode(0o700).create(&recordings) {
                Ok(()) => (),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => (),
                Err(error) => return Err(error),
            }
            recordings.join(format!("{name}-{stamp}.ftape"))
        }
    };
    std::path::absolute(path)
}

fn target(options: &Options) -> io::Result<Option<Window>> {
    let selected = options.app.is_some() || options.pid.is_some() || options.window.is_some();
    if options.capture.output.is_some() {
        if selected || !options.command.is_empty() {
            return Err(io::Error::other(
                "--output records a monitor; omit it when selecting an app",
            ));
        }
        return Ok(None);
    }
    if options.command.is_empty() {
        return if selected {
            window::select(
                options.app.as_deref(),
                options.pid,
                options.window.as_deref(),
            )
            .map(Some)
        } else {
            window::foreground().map(Some)
        };
    }
    let mut child = Command::new(&options.command[0])
        .args(&options.command[1..])
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let found = if selected {
            window::select(
                options.app.as_deref(),
                options.pid,
                options.window.as_deref(),
            )
            .ok()
        } else {
            let owned: Vec<_> = window::windows()?
                .into_iter()
                .filter(|w| descendant(w.pid, child.id()))
                .collect();
            match owned.as_slice() {
                [window] => Some(window.clone()),
                _ => None,
            }
        };
        if let Some(window) = found {
            return Ok(Some(window));
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                return Err(io::Error::other(format!("app launch failed: {status}")));
            }
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(
                "app window did not appear; select it with --app, --pid or --window",
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn descendant(mut pid: u32, ancestor: u32) -> bool {
    while pid > 1 {
        if pid == ancestor {
            return true;
        }
        let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let Some(parent) = stat
            .rsplit_once(") ")
            .and_then(|(_, tail)| tail.split_whitespace().nth(1))
            .and_then(|value| value.parse::<u32>().ok())
        else {
            return false;
        };
        if parent == pid {
            return false;
        }
        pid = parent;
    }
    false
}

fn notify(title: &str, body: &str) {
    let shown = Command::new("notify-send")
        .args(["--app-name=Frametape", title, body])
        .output()
        .is_ok_and(|output| output.status.success());
    if !shown {
        let _ = Command::new("hyprctl")
            .args([
                "notify",
                "-1",
                "3500",
                "rgb(52b788)",
                &format!("Frametape: {title}: {body}"),
            ])
            .output();
    }
}

pub fn record(options: Options) -> io::Result<()> {
    let mut lease = lease()?;
    if let Ok(bytes) = fs::read(&lease.state) {
        let previous: Session = serde_json::from_slice(&bytes).map_err(io::Error::other)?;
        lease.preserve = true;
        recover_native(&previous)?;
        lease.preserve = false;
    }
    if options
        .capture
        .seconds
        .is_some_and(|s| !s.is_finite() || s <= 0.0)
    {
        return Err(io::Error::other("seconds must be finite and positive"));
    }
    let target = target(&options)?;
    let path = destination(options.path.clone(), target.as_ref())?;
    if path.exists() {
        return Err(io::Error::other("output exists"));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let native = if options.native == Native::Off {
        false
    } else if let Some(window) = &target {
        // Process-level providers cannot disambiguate multiple compositor windows.
        window::windows()?
            .iter()
            .filter(|w| w.pid == window.pid)
            .count()
            == 1
            && window.pid > 0
            && frametape_control::available(window.pid).unwrap_or(false)
    } else {
        false
    };
    if options.native == Native::Required && !native {
        return Err(io::Error::other(
            "this app has no registered native Frametape recorder",
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let signal = stop.clone();
    ctrlc::set_handler(move || signal.store(true, Ordering::Relaxed)).map_err(io::Error::other)?;
    let session = Session {
        pid: std::process::id(),
        birth: process_birth(std::process::id())?,
        path: path.clone(),
        target_birth: target.as_ref().and_then(|w| process_birth(w.pid).ok()),
        target: target.clone(),
        native,
    };
    let staged = lease
        .state
        .with_file_name(format!("record-{}-{}.json", session.pid, session.birth));
    frametape::atomic_json(&staged, &session)?;
    fs::rename(staged, &lease.state)?;
    let description = target.as_ref().map_or("desktop", |w| w.class.as_str());
    let result = if native {
        let pid = target.as_ref().unwrap().pid;
        lease.preserve = true;
        let started = checked_call(
            pid,
            &Request::Start {
                path: path.clone(),
                window_title: target.as_ref().unwrap().title.clone(),
            },
        )?;
        if !started.recording || started.path.as_ref() != Some(&path) {
            return Err(io::Error::other(
                "native recorder did not acknowledge requested take",
            ));
        }
        notify("Recording natively", description);
        let began = Instant::now();
        let mut next_status = Instant::now() + Duration::from_secs(1);
        while !stop.load(Ordering::Relaxed)
            && options
                .capture
                .seconds
                .is_none_or(|s| began.elapsed().as_secs_f64() < s)
        {
            if !process_birth(pid).is_ok_and(|birth| Some(birth) == session.target_birth) {
                break;
            }
            if Instant::now() >= next_status {
                let status = call(pid, &Request::Status)?;
                if !status.recording {
                    break;
                }
                next_status = Instant::now() + Duration::from_secs(1);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        checked_call(pid, &Request::Stop { path: path.clone() }).and_then(|reply| {
            if reply.recording || reply.path.as_ref() != Some(&path) || !path.is_file() {
                Err(io::Error::other(
                    "native recorder did not publish requested archive",
                ))
            } else {
                println!("{}", path.display());
                Ok(())
            }
        })
    } else {
        notify("Recording window", description);
        let output = options.capture.output.clone();
        let capture_target = match &target {
            Some(window) => CaptureTarget::Window(window.handle()?),
            None => CaptureTarget::Output(output.as_deref()),
        };
        desktop::record(
            &path,
            options.capture,
            capture_target,
            serde_json::to_value(&target)?,
            stop,
            || match &target {
                Some(target) => Ok(window::windows()?
                    .iter()
                    .any(|w| w.address == target.address && w.pid == target.pid)),
                None => Ok(true),
            },
        )
    };
    if result.is_ok() {
        lease.preserve = false;
    }
    match &result {
        Ok(()) => notify("Recording saved", &path.display().to_string()),
        Err(e) => notify("Recording failed; journal retained", &e.to_string()),
    }
    result
}

/// Install the current executable and the compositor shortcut without editing shell configuration.
pub fn setup() -> io::Result<()> {
    let home = PathBuf::from(
        std::env::var_os("HOME").ok_or_else(|| io::Error::other("HOME is required"))?,
    );
    let bin = home.join(".local/bin/frametape");
    fs::create_dir_all(bin.parent().unwrap())?;
    let current = std::env::current_exe()?;
    if fs::canonicalize(&bin).ok().as_ref() != Some(&current) {
        let temp = bin.with_extension(format!("{}.installing", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&temp)?;
        std::io::copy(&mut File::open(current)?, &mut file)?;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
        file.sync_all()?;
        fs::rename(temp, &bin)?;
    }
    let command = format!("{} toggle", bin.display());
    let lua = format!(
        "hl.unbind(\"SUPER + SHIFT + R\"); hl.bind(\"SUPER + SHIFT + R\", hl.dsp.exec_cmd({}))",
        serde_json::to_string(&command)?
    );
    let result = Command::new("hyprctl").args(["eval", &lua]).output()?;
    if !result.status.success() || String::from_utf8_lossy(&result.stdout).trim() != "ok" {
        return Err(io::Error::other(format!(
            "Hyprland Lua shortcut registration failed: {} {}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )));
    }
    println!(
        "installed {}; Super+Shift+R toggles foreground recording for this compositor session",
        bin.display()
    );
    Ok(())
}
