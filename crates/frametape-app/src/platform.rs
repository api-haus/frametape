pub fn prepare() {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::path::Path::new("/sys/module/nvidia").exists()
    {
        // NVIDIA EGL/GTK can commit a buffer without its acquire timeline, and GStreamer's GL
        // sink hands WebKit's compositor blank (green) pictures. Keep GPU page compositing:
        // the shared-memory renderer copies the whole window on the CPU every frame.
        // https://github.com/NVIDIA/egl-wayland2#environment-variables
        for name in ["__NV_DISABLE_EXPLICIT_SYNC", "WEBKIT_GST_DISABLE_GL_SINK"] {
            if std::env::var_os(name).is_none() {
                // Called before GTK and the review server start any threads.
                unsafe { std::env::set_var(name, "1") };
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub fn configure_window(window: &impl gtk::prelude::IsA<gtk::Widget>) {
    use gtk::prelude::WidgetExt;
    // The web view covers the window; GTK's own background is a full-window CPU fill per frame.
    window.set_app_paintable(true);
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

#[cfg(target_os = "linux")]
pub fn configure_view(view: &wry::WebView) {
    use gtk::glib::translate::ToGlibPtr;
    use std::ffi::{CStr, c_char, c_int, c_void};
    use webkit2gtk::WebViewExt;
    use wry::WebViewExtUnix;
    // webkit2gtk-rs stops at the 2.40 API; runtime features arrived in 2.42.
    unsafe extern "C" {
        fn webkit_settings_get_all_features() -> *mut c_void;
        fn webkit_feature_list_get_length(list: *mut c_void) -> usize;
        fn webkit_feature_list_get(list: *mut c_void, index: usize) -> *mut c_void;
        fn webkit_feature_list_unref(list: *mut c_void);
        fn webkit_feature_get_identifier(feature: *mut c_void) -> *const c_char;
        fn webkit_settings_set_feature_enabled(
            settings: *mut c_void,
            feature: *mut c_void,
            on: c_int,
        );
    }
    let Some(settings) = view.webview().settings() else {
        return;
    };
    let settings: *mut webkit2gtk::ffi::WebKitSettings = settings.to_glib_none().0;
    // WebKit otherwise presents near 60 Hz whatever the display and recording rates are.
    unsafe {
        let features = webkit_settings_get_all_features();
        for index in 0..webkit_feature_list_get_length(features) {
            let feature = webkit_feature_list_get(features, index);
            if CStr::from_ptr(webkit_feature_get_identifier(feature)).to_bytes()
                == b"PreferPageRenderingUpdatesNear60FPSEnabled"
            {
                webkit_settings_set_feature_enabled(settings.cast(), feature, 0);
            }
        }
        webkit_feature_list_unref(features);
    }
}
