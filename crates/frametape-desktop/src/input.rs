use frametape::Event;
use serde_json::json;
use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

pub fn input_devices() -> io::Result<Vec<PathBuf>> {
    let mut paths: Vec<_> = std::fs::read_dir("/dev/input")?
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("event"))
        })
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    Ok(paths)
}

#[derive(Debug, Default)]
pub struct InputSummary {
    pub events: u64,
    pub lost_packets: u64,
    pub devices: u64,
}

pub struct InputRecorder {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<io::Result<InputSummary>>>,
}

struct Device {
    path: PathBuf,
    file: File,
    name: String,
}

fn device(path: &Path) -> io::Result<Device> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    let clock: libc::c_int = libc::CLOCK_MONOTONIC;
    // EVIOCSCLOCKID: require kernel event timestamps in the capture clock domain.
    if unsafe { libc::ioctl(file.as_raw_fd(), 0x400445a0, &clock) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut name = [0u8; 256];
    if unsafe { libc::ioctl(file.as_raw_fd(), 0x81004506, name.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    let name =
        String::from_utf8_lossy(&name[..name.iter().position(|b| *b == 0).unwrap_or(name.len())])
            .into_owned();
    Ok(Device {
        path: path.into(),
        file,
        name,
    })
}

fn write(
    writer: &mut BufWriter<File>,
    timestamp: u64,
    epoch: u64,
    data: serde_json::Value,
) -> io::Result<()> {
    if timestamp < epoch {
        return Ok(());
    }
    let event = Event {
        frame: None,
        timestamp_ns: timestamp - epoch,
        channel: "input.evdev".into(),
        data,
    };
    serde_json::to_writer(&mut *writer, &event).map_err(io::Error::other)?;
    writer.write_all(b"\n")
}

fn added(writer: &mut BufWriter<File>, device: &Device, epoch: u64) -> io::Result<()> {
    let mut keys = [0u8; 96];
    let count = unsafe { libc::ioctl(device.file.as_raw_fd(), 0x80604518, keys.as_mut_ptr()) };
    if count < 0 {
        return Err(io::Error::last_os_error());
    }
    let pressed: Vec<usize> = (0..keys.len() * 8)
        .filter(|code| keys[code / 8] & (1 << (code % 8)) != 0)
        .collect();
    write(
        writer,
        crate::monotonic_ns()?,
        epoch,
        json!({"event":"device_added","device":device.path,"name":device.name,"held_keys":pressed}),
    )
}

impl InputRecorder {
    /// Empty selection follows all evdev devices, including hotplug. Unreadable devices fail loudly.
    pub fn start(selection: Vec<PathBuf>, output: &Path, epoch_ns: u64) -> io::Result<Self> {
        let follow = selection.is_empty();
        let paths = if follow {
            input_devices()?
        } else {
            selection.clone()
        };
        if paths.is_empty() {
            return Err(io::Error::other(
                "no input devices; select devices or use --no-input",
            ));
        }
        let devices = paths
            .iter()
            .map(|p| device(p))
            .collect::<io::Result<Vec<_>>>()?;
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output)?;
        let stop = Arc::new(AtomicBool::new(false));
        let signal = stop.clone();
        let worker=thread::Builder::new().name("frametape input".into()).spawn(move || {
            let mut devices=devices;
            let mut writer=BufWriter::new(file);
            let mut summary=InputSummary::default();
            for device in &devices { added(&mut writer,device,epoch_ns)?;summary.devices+=1; }
            let mut rescan=Instant::now();
            while !signal.load(Ordering::Relaxed) {
                let mut polls:Vec<_>=devices.iter().map(|d|libc::pollfd {fd:d.file.as_raw_fd(),events:libc::POLLIN,revents:0}).collect();
                let result=unsafe {libc::poll(polls.as_mut_ptr(),polls.len() as libc::nfds_t,50)};
                if result<0 {
                    let error=io::Error::last_os_error();
                    if error.kind()==io::ErrorKind::Interrupted {continue;} return Err(error);
                }
                let mut removed=HashSet::new();
                for (device,poll) in devices.iter_mut().zip(&polls) {
                    if poll.revents&(libc::POLLHUP|libc::POLLERR|libc::POLLNVAL)!=0 {removed.insert(device.path.clone());continue;}
                    if poll.revents&libc::POLLIN==0 {continue;}
                    loop {
                        let mut event:libc::input_event=unsafe {std::mem::zeroed()};
                        let bytes=unsafe {std::slice::from_raw_parts_mut((&mut event as *mut libc::input_event).cast::<u8>(),std::mem::size_of::<libc::input_event>())};
                        match device.file.read(bytes) {
                            Ok(0)=>{removed.insert(device.path.clone());break;}
                            Ok(n) if n!=bytes.len()=>return Err(io::Error::other("partial evdev event")),
                            Ok(_)=>{},
                            Err(e) if e.kind()==io::ErrorKind::WouldBlock=>break,
                            Err(e) if e.raw_os_error()==Some(libc::ENODEV)=>{removed.insert(device.path.clone());break;},
                            Err(e)=>return Err(e),
                        }
                        let timestamp=(event.time.tv_sec as u64).checked_mul(1_000_000_000).and_then(|t|t.checked_add(event.time.tv_usec as u64*1000)).ok_or_else(||io::Error::other("evdev timestamp overflow"))?;
                        if event.type_==0 && event.code==3 {summary.lost_packets+=1;}
                        write(&mut writer,timestamp,epoch_ns,json!({"device":device.path,"name":device.name,"type":event.type_,"code":event.code,"value":event.value,"lost":event.type_==0&&event.code==3}))?;
                        summary.events+=1;
                    }
                }
                for path in &removed {write(&mut writer,crate::monotonic_ns()?,epoch_ns,json!({"event":"device_removed","device":path}))?;}
                devices.retain(|d|!removed.contains(&d.path));
                if follow && rescan.elapsed()>=Duration::from_secs(1) {
                    for path in input_devices()? {
                        if !devices.iter().any(|d|d.path==path) {
                            let device=device(&path)?;added(&mut writer,&device,epoch_ns)?;devices.push(device);summary.devices+=1;
                        }
                    }
                    rescan=Instant::now();writer.flush()?;
                }
            }
            writer.flush()?;writer.get_ref().sync_all()?;Ok(summary)
        })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }

    pub fn stop(mut self) -> io::Result<InputSummary> {
        self.stop.store(true, Ordering::Relaxed);
        self.worker
            .take()
            .unwrap()
            .join()
            .map_err(|_| io::Error::other("input recorder panicked"))?
    }
}

impl Drop for InputRecorder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
