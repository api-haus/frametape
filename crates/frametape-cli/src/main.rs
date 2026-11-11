mod app;
#[cfg(target_os = "linux")]
mod desktop;
mod export;

use clap::{Parser, Subcommand};
use frametape::TapeReader;
use std::io;
use std::path::PathBuf;

#[derive(Parser)]
#[command(version, about = "Record, annotate and inspect synchronized evidence")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Capture a Wayland output and Linux input events into one recording.
    #[cfg(target_os = "linux")]
    Record(desktop::Options),
    /// Open the recording catalog and annotation app locally.
    App {
        path: PathBuf,
        #[arg(long, default_value_t = 0)]
        port: u16,
        #[arg(long)]
        no_open: bool,
    },
    /// Print manifest, channel data or annotations as JSON.
    Inspect {
        path: PathBuf,
        #[arg(long)]
        frames: bool,
        #[arg(long)]
        events: bool,
        #[arg(long)]
        annotations: bool,
        #[arg(long)]
        frame: Option<u64>,
        #[arg(long)]
        channel: Option<String>,
    },
    /// Extract one original member without decoding or modifying it.
    Extract {
        path: PathBuf,
        member: String,
        output: PathBuf,
    },
    /// Bundle completed Raycat JSONL or producer-exported BallRPG takes and sidecars.
    Pack {
        path: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        data: Option<PathBuf>,
    },
    /// Recover a recording directory left by an interrupted writer.
    Recover { directory: PathBuf, output: PathBuf },
    /// Export annotated images and matching channel records for agents.
    Export {
        path: PathBuf,
        output: PathBuf,
        #[arg(long, value_delimiter = ',')]
        frames: Vec<u64>,
        #[arg(long)]
        every: Option<u64>,
        #[arg(long)]
        list: bool,
    },
    /// List members in the single recording file.
    Members { path: PathBuf },
}

fn main() {
    if let Err(error) = run(Cli::parse()) {
        eprintln!("frametape: {error}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> io::Result<()> {
    match cli.command {
        #[cfg(target_os = "linux")]
        Command::Record(options) => desktop::record(options),
        Command::App {
            path,
            port,
            no_open,
        } => app::serve(path, port, no_open),
        Command::Inspect {
            path,
            frames,
            events,
            annotations,
            frame,
            channel,
        } => {
            let mut tape = TapeReader::open(path)?;
            let result = if annotations {
                serde_json::to_value(tape.annotations()?)
            } else if events || frame.is_some() || channel.is_some() {
                let frames = tape.frames()?;
                serde_json::to_value(
                    tape.events()?
                        .into_iter()
                        .filter(|e| {
                            let associated = e.frame.or_else(|| {
                                frametape::frame_at(&frames, e.timestamp_ns).map(|f| f.index)
                            });
                            frame.is_none_or(|f| associated == Some(f))
                                && channel.as_ref().is_none_or(|c| c == &e.channel)
                        })
                        .collect::<Vec<_>>(),
                )
            } else if frames {
                serde_json::to_value(tape.frames()?)
            } else {
                serde_json::to_value(&tape.manifest)
            };
            serde_json::to_writer_pretty(io::stdout().lock(), &result.map_err(io::Error::other)?)
                .map_err(io::Error::other)?;
            println!();
            Ok(())
        }
        Command::Extract {
            path,
            member,
            output,
        } => TapeReader::open(path)?.extract(&member, &output),
        Command::Pack { path, out, data } => {
            let output = out.unwrap_or_else(|| path.with_extension("ftape"));
            let packed = if path.extension().is_some_and(|e| e == "jsonl") {
                frametape::import::raycat(&path, &output)?
            } else {
                let data = data.unwrap_or_else(|| path.with_extension("data.json"));
                frametape::import::ballrpg_export(&path, &data, &output)?
            };
            println!("{}", packed.display());
            Ok(())
        }
        Command::Recover { directory, output } => {
            println!(
                "{}",
                frametape::import::recover(&directory, &output)?.display()
            );
            Ok(())
        }
        Command::Export {
            path,
            output,
            frames,
            every,
            list,
        } => export::export(&path, &output, &frames, every, list),
        Command::Members { path } => {
            for name in TapeReader::open(path)?.members() {
                println!("{name}");
            }
            Ok(())
        }
    }
}
