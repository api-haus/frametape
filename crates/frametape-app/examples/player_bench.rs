//! Native player timing host; tools/bench-player drives real input in a private X display.
#[path = "../src/platform.rs"]
mod platform;
use frametape_review::server::AppServer;
use std::{fs::OpenOptions, io::Write, path::PathBuf};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::run_return::EventLoopExtRunReturn,
    window::WindowBuilder,
};
#[cfg(target_os = "linux")]
use {tao::platform::unix::WindowExtUnix, wry::WebViewBuilderExtUnix};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    platform::prepare();
    let mut args = std::env::args_os().skip(1);
    let recording = PathBuf::from(args.next().ok_or("recording path required")?);
    let output = PathBuf::from(args.next().ok_or("metrics path required")?);
    let metrics = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(output)?;
    let server = AppServer::start(recording, 0)?;
    let mut events = EventLoopBuilder::<()>::with_user_event().build();
    platform::configure_display();
    let window = WindowBuilder::new()
        .with_title("Frametape player benchmark")
        .with_inner_size(LogicalSize::new(1400., 900.))
        .build(&events)?;
    let base = server.url.split('?').next().unwrap().to_owned();
    let diagnostic = args
        .next()
        .map(std::fs::read_to_string)
        .transpose()?
        .unwrap_or_default();
    let observer = format!(
        "window.__benchImages={};{};{}",
        std::env::var_os("FRAMETAPE_BENCH_IMAGES").is_some(),
        include_str!("../../../tools/bench-player/instrument.js"),
        diagnostic
    );
    let builder = wry::WebViewBuilder::new()
        .with_url(&server.url)
        .with_initialization_script_for_main_only(&observer, false)
        .with_ipc_handler(move |request| {
            if !request.uri().to_string().starts_with(&base) {
                return;
            }
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(request.body()) {
                let mut output = &metrics;
                let _ = writeln!(output, "{value}");
                let _ = output.flush();
            }
        });
    #[cfg(target_os = "linux")]
    let view = builder.build_gtk(
        window
            .default_vbox()
            .ok_or("window container unavailable")?,
    )?;
    #[cfg(not(target_os = "linux"))]
    let view = builder.build(&window)?;
    events.run_return(|event, _, flow| {
        *flow = ControlFlow::Wait;
        if matches!(
            event,
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            }
        ) {
            *flow = ControlFlow::Exit
        }
    });
    drop(view);
    drop(window);
    drop(server);
    Ok(())
}
