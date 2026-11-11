// SPDX-License-Identifier: MIT
// Attach bytes supplied by any Box3D host; frame mappings come from that host's real timeline.
use frametape::{Channel, Event, Frame, TapeWriter};
use std::{env, fs, io, path::PathBuf};

fn main() -> io::Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 3 {
        return Err(io::Error::other("usage: attach-box3d RECORDING.b3rec TIMELINE.json OUTPUT.ftape"));
    }
    let output = PathBuf::from(&args[2]);
    let mut journal = output.as_os_str().to_owned();
    journal.push(".recording");
    let timeline: serde_json::Value = serde_json::from_slice(&fs::read(&args[1])?)?;
    let mut tape = TapeWriter::create(PathBuf::from(journal), serde_json::json!({"host":"Box3D Rust attachment example"}))?;
    tape.register(Channel { id: "physics.box3d".into(), kind: "physics".into(), schema: "application/x-box3d-recording".into() })?;
    tape.register(Channel { id: "physics.box3d.step".into(), kind: "physics".into(), schema: "application/json".into() })?;
    let frames: Vec<Frame> = serde_json::from_value(timeline["frames"].clone())?;
    let events: Vec<Event> = serde_json::from_value(timeline["events"].clone())?;
    for frame in frames { tape.frame(&frame)?; }
    for event in events { tape.event(&event)?; }
    tape.attach("physics.box3d", "simulation.b3rec", PathBuf::from(&args[0]), "application/x-box3d-recording")?;
    tape.finish(output)?;
    Ok(())
}
