use gpui::PlatformKeyboardLayout;

pub(crate) struct WinitKeyboardLayout;

impl PlatformKeyboardLayout for WinitKeyboardLayout {
    fn id(&self) -> &str {
        // TODO(winit): Report the active Windows keyboard layout.
        "unknown"
    }

    fn name(&self) -> &str {
        "Unknown"
    }
}
