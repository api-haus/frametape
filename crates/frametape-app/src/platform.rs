pub fn prepare() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::path::Path::new("/sys/module/nvidia").exists()
        && std::env::var_os("__NV_DISABLE_EXPLICIT_SYNC").is_none()
    {
        // NVIDIA EGL/GTK can attach a buffer without its required acquire timeline.
        // https://github.com/NVIDIA/egl-wayland2#environment-variables
        // Called before GTK and the review server start any threads.
        unsafe { std::env::set_var("__NV_DISABLE_EXPLICIT_SYNC", "1") };
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
