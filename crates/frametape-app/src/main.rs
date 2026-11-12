mod platform;
use frametape_review::server::AppServer;
use std::path::PathBuf;
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    platform::run_return::EventLoopExtRunReturn,
    window::WindowBuilder,
};
use wry::WebViewBuilder;
#[cfg(target_os = "linux")]
use {
    tao::platform::unix::{EventLoopBuilderExtUnix, WindowExtUnix},
    wry::WebViewBuilderExtUnix,
};

#[derive(Debug)]
enum Action {
    OpenFile,
    OpenFolder,
    Dirty(bool),
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    platform::prepare();
    let path = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join("Videos/Frametape")
        });
    if !path.exists() && path.extension().is_none() {
        std::fs::create_dir_all(&path)?;
    }
    let server = AppServer::start(path, 0)?;
    let mut builder = EventLoopBuilder::<Action>::with_user_event();
    #[cfg(target_os = "linux")]
    builder.with_app_id("org.apihaus.Frametape");
    let mut events = builder.build();
    platform::configure_display();
    let window = WindowBuilder::new()
        .with_title("Frametape")
        .with_inner_size(LogicalSize::new(1400.0, 900.0))
        .build(&events)?;
    let proxy = events.create_proxy();
    let base = server.url.split('?').next().unwrap().to_owned();
    let allowed = base.clone();
    let blob_origin = format!(
        "blob:{}",
        base.split('/').take(3).collect::<Vec<_>>().join("/")
    );
    let ipc_base = base.clone();
    let view=WebViewBuilder::new().with_url(&server.url)
        .with_initialization_script("window.frametapeNative={open:(kind)=>window.ipc.postMessage(JSON.stringify({action:kind})),dirty:(value)=>window.ipc.postMessage(JSON.stringify({action:'dirty',value}))};")
        .with_navigation_handler(move |url| url.starts_with(&allowed)||url=="about:srcdoc"||url=="about:blank"||url.starts_with(&blob_origin))
        .with_ipc_handler(move |request| {
            if !request.uri().to_string().starts_with(&ipc_base) { return; }
            if let Ok(value)=serde_json::from_str::<serde_json::Value>(request.body()) {
                let action=match value["action"].as_str() { Some("file")=>Action::OpenFile,Some("folder")=>Action::OpenFolder,Some("dirty")=>Action::Dirty(value["value"].as_bool().unwrap_or(false)),_=>return };
                let _=proxy.send_event(action);
            }
        })
        .with_download_started_handler(|_,destination| {
            let name=destination.file_name().and_then(|x|x.to_str()).unwrap_or("annotated.ftape");
            if let Some(path)=rfd::FileDialog::new().set_title("Save recording copy").set_file_name(name).add_filter("Frametape",&["ftape"]).save_file() {
                if path.exists() { rfd::MessageDialog::new().set_title("Choose a new filename").set_description("Save copy preserves existing recordings. Use Save to edit the open recording.").show(); return false; }
                *destination=path;true
            } else {false}
        });
    #[cfg(target_os = "linux")]
    let view = view.build_gtk(
        window
            .default_vbox()
            .ok_or("native window container unavailable")?,
    )?;
    #[cfg(not(target_os = "linux"))]
    let view = view.build(&window)?;
    let mut dirty = false;
    events.run_return(|event,_,flow| {
        *flow=ControlFlow::Wait;
        match event {
            Event::UserEvent(Action::Dirty(value))=>dirty=value,
            Event::UserEvent(action @ (Action::OpenFile|Action::OpenFolder))=>{
                let picker=rfd::FileDialog::new();
                let chosen=if matches!(action,Action::OpenFolder){picker.pick_folder()}else{picker.add_filter("Frametape recordings",&["ftape"]).pick_file()};
                if let Some(path)=chosen {
                    if dirty && rfd::MessageDialog::new().set_title("Unsaved annotations").set_description("Discard unsaved annotations and open another recording?").set_buttons(rfd::MessageButtons::YesNo).show()!=rfd::MessageDialogResult::Yes { return; }
                    if dirty { let _=view.evaluate_script("document.querySelector('#review').contentWindow.postMessage({type:'discard'},'*')"); }
                    match server.open(&path) {
                        Ok(url)=>{dirty=false;let _=view.load_url(&url);},
                        Err(error)=>{rfd::MessageDialog::new().set_title("Cannot open recording").set_description(error.to_string()).show();}
                    }
                }
            }
            Event::WindowEvent{event:WindowEvent::CloseRequested,..}=>{
                if !dirty || rfd::MessageDialog::new().set_title("Unsaved annotations").set_description("Keep editing to save your changes, or discard them and close.").set_buttons(rfd::MessageButtons::OkCancelCustom("Discard and close".into(),"Keep editing".into())).show()==rfd::MessageDialogResult::Custom("Discard and close".into()) {*flow=ControlFlow::Exit;}
            }
            _=>()
        }
    });
    drop(view);
    drop(window);
    drop(server);
    Ok(())
}
