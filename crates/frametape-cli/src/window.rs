use serde::{Deserialize, Serialize};
use std::{io, process::Command};

/// Compositor identity, fixed at recording start. Focus changes never retarget a take.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Window {
    pub address: String,
    #[serde(default, deserialize_with = "process_id")]
    pub pid: u32,
    pub class: String,
    pub title: String,
}

fn process_id<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    Ok(Option::<i64>::deserialize(deserializer)?
        .and_then(|pid| u32::try_from(pid).ok())
        .unwrap_or(0))
}

fn query(argument: &str) -> io::Result<serde_json::Value> {
    let result = Command::new("hyprctl").args([argument, "-j"]).output()?;
    if !result.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&result.stderr).to_string(),
        ));
    }
    serde_json::from_slice(&result.stdout).map_err(io::Error::other)
}

pub fn foreground() -> io::Result<Window> {
    serde_json::from_value(query("activewindow")?)
        .map_err(|_| io::Error::other("no foreground application window"))
}

pub fn windows() -> io::Result<Vec<Window>> {
    serde_json::from_value(query("clients")?).map_err(io::Error::other)
}

pub fn select(app: Option<&str>, pid: Option<u32>, address: Option<&str>) -> io::Result<Window> {
    let found: Vec<_> = windows()?
        .into_iter()
        .filter(|w| {
            app.is_none_or(|name| w.class.eq_ignore_ascii_case(name) || w.title == name)
                && pid.is_none_or(|pid| w.pid == pid)
                && address.is_none_or(|address| w.address == address)
        })
        .collect();
    match found.as_slice() {
        [window] => Ok(window.clone()),
        [] => Err(io::Error::other("no matching application window")),
        _ => Err(io::Error::other(
            "multiple matching windows; select --window ADDRESS",
        )),
    }
}

impl Window {
    pub fn handle(&self) -> io::Result<u32> {
        // The compositor's protocol uses the low32bits of its hyprctl window address.
        let address = u64::from_str_radix(self.address.trim_start_matches("0x"), 16)
            .map_err(io::Error::other)?;
        Ok(address as u32)
    }
}
