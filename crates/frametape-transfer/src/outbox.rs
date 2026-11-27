use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

fn directory() -> io::Result<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .or_else(|| std::env::var_os("LOCALAPPDATA"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .map(|root| root.join("frametape/outbox"))
        .ok_or_else(|| io::Error::other("Frametape outbox directory unavailable"))
}

pub(crate) struct Pending {
    _lock: File,
    marker: PathBuf,
    archive: PathBuf,
}

impl Pending {
    pub fn register(output: &Path, id: &str) -> io::Result<Self> {
        let directory = directory()?;
        fs::create_dir_all(&directory)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        }
        let archive =
            std::path::absolute(output)?.with_file_name(format!(".frametape-{id}.pending"));
        let marker = directory.join(format!("{id}.json"));
        let temporary = marker.with_extension("writing");
        let mut lock = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)?;
        let result = (|| {
            lock.try_lock().map_err(io::Error::other)?;
            fs::hard_link(output, &archive)?;
            let recorded = (|| {
                lock.write_all(&serde_json::to_vec(&archive)?)?;
                lock.sync_all()?;
                fs::rename(&temporary, &marker)
            })();
            if recorded.is_err() {
                let _ = fs::remove_file(&archive);
            }
            recorded
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result?;
        Ok(Self {
            _lock: lock,
            marker,
            archive,
        })
    }

    pub fn complete(self) -> io::Result<()> {
        fs::remove_file(&self.archive)?;
        fs::remove_file(&self.marker)
    }
}

pub(crate) fn retry(mut publish: impl FnMut(&str, File) -> io::Result<()>) -> io::Result<()> {
    let entries = match fs::read_dir(directory()?) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut failed = None;
    for entry in entries {
        let marker = entry?.path();
        let Some(id) = marker.file_stem().and_then(|s| s.to_str()).filter(|id| {
            marker.extension().is_some_and(|ext| ext == "json") && super::valid_id(id)
        }) else {
            continue;
        };
        let lock = match OpenOptions::new().read(true).write(true).open(&marker) {
            Ok(lock) if lock.try_lock().is_ok() => lock,
            _ => continue,
        };
        let archive: PathBuf = serde_json::from_reader(&lock)?;
        let result = File::open(&archive).and_then(|file| publish(id, file));
        match result {
            Ok(()) => Pending {
                _lock: lock,
                marker,
                archive,
            }
            .complete()?,
            Err(error) => failed = Some(error),
        }
    }
    failed.map_or(Ok(()), Err)
}
