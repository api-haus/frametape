pub fn prepare() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::path::Path::new("/sys/module/nvidia").exists()
    {
        // NVIDIA EGL/GTK can miss acquire timelines or lose composited page layers.
        // Keep this process on WebKit's SHM path; explicit user overrides win.
        // https://github.com/NVIDIA/egl-wayland2#environment-variables
        for name in [
            "__NV_DISABLE_EXPLICIT_SYNC",
            "WEBKIT_DISABLE_DMABUF_RENDERER",
        ] {
            if std::env::var_os(name).is_none() {
                // Called before GTK and the review server start any threads.
                unsafe { std::env::set_var(name, "1") };
            }
        }
    }
}

pub fn configure_display() {
    #[cfg(target_os = "linux")]
    if let Some(screen) = gtk::gdk::Screen::default() {
        // WebKitGTK 2.54 turns GDK's unset DPI (-1) into negative viewport dimensions.
        if screen.resolution() <= 0.0 {
            screen.set_resolution(96.0);
        }
    }
}
