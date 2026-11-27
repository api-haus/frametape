use frametape::TapeReader;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::RwLock;

pub(crate) fn id(path: &Path) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut hash);
    format!("{:016x}", hash.finish())
}

pub(crate) struct Recordings {
    pub root: PathBuf,
    pub selected: Option<(String, PathBuf)>,
    portal: bool,
    paths: RwLock<Option<HashMap<String, PathBuf>>>,
}

impl Recordings {
    pub fn open(path: &Path, portal: bool) -> io::Result<Self> {
        let path = fs::canonicalize(path)?;
        let stat = fs::metadata(&path)?;
        let (root, selected) = if stat.is_dir() {
            (path, None)
        } else if stat.is_file() {
            (path.parent().unwrap().to_owned(), Some((id(&path), path)))
        } else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "expected a recording or folder",
            ));
        };
        Ok(Self {
            root,
            selected,
            portal,
            paths: RwLock::default(),
        })
    }

    pub fn index(&self) -> io::Result<Vec<Value>> {
        let mut paths = HashMap::new();
        let mut rows = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "ftape") && entry.file_type()?.is_file() {
                let id = if self.portal {
                    let name = path.file_stem().unwrap().to_string_lossy();
                    if !frametape::transfer::valid_id(&name) {
                        continue;
                    }
                    name.into_owned()
                } else {
                    id(&path)
                };
                rows.push(file_info(&id, &path));
                paths.insert(id, path);
            }
        }
        *self
            .paths
            .write()
            .map_err(|_| io::Error::other("recording index poisoned"))? = Some(paths);
        Ok(rows)
    }

    pub fn resolve(&self, id: &str) -> io::Result<Option<PathBuf>> {
        if self.portal {
            if !frametape::transfer::valid_id(id) {
                return Ok(None);
            }
            let path = self.root.join(format!("{id}.ftape"));
            return Ok(fs::symlink_metadata(&path)
                .is_ok_and(|stat| stat.is_file())
                .then_some(path));
        }
        if let Some((selected, path)) = &self.selected
            && selected == id
        {
            return Ok(fs::symlink_metadata(path)
                .is_ok_and(|stat| stat.is_file())
                .then(|| path.clone()));
        }
        if id.len() != 16 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(None);
        }
        // A folder bookmark can select a recording before the sidebar requests the index.
        if self
            .paths
            .read()
            .map_err(|_| io::Error::other("recording index poisoned"))?
            .is_none()
        {
            self.index()?;
        }
        let paths = self
            .paths
            .read()
            .map_err(|_| io::Error::other("recording index poisoned"))?;
        Ok(paths
            .as_ref()
            .and_then(|paths| paths.get(id))
            .filter(|path| fs::symlink_metadata(path).is_ok_and(|stat| stat.is_file()))
            .cloned())
    }
}

fn file_info(id: &str, path: &Path) -> Value {
    let name = path.file_name().unwrap().to_string_lossy();
    match fs::symlink_metadata(path) {
        Ok(stat) if stat.is_file() => {
            let modified_ms = stat
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |time| time.as_millis());
            json!({"id":id,"name":name,"bytes":stat.len(),"modified_ms":modified_ms})
        }
        _ => json!({"id":id,"name":name,"error":"Recording is no longer available"}),
    }
}

pub(crate) fn detail(id: &str, path: &Path) -> Value {
    let mut row = file_info(id, path);
    if row["error"].is_null() {
        let inspected = (|| -> io::Result<()> {
            let mut tape = TapeReader::open(path)?;
            let annotations = tape.annotations()?;
            if frametape::transfer::valid_id(id) {
                for field in ["title", "project", "host"] {
                    if let Some(value) = tape.manifest.metadata[field]
                        .as_str()
                        .filter(|value| !value.is_empty())
                    {
                        row["name"] = json!(value);
                        break;
                    }
                }
            }
            row["manifest"] = serde_json::to_value(&tape.manifest).map_err(io::Error::other)?;
            row["labels"] = json!(
                annotations
                    .containers
                    .iter()
                    .flat_map(|c| c.labels())
                    .collect::<Vec<_>>()
            );
            Ok(())
        })();
        if let Err(error) = inspected {
            row["error"] = json!(error.to_string());
        }
    }
    row
}
