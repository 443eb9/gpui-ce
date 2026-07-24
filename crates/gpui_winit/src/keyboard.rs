use gpui::PlatformKeyboardLayout;

pub(crate) struct WinitKeyboardLayout;

impl PlatformKeyboardLayout for WinitKeyboardLayout {
    fn id(&self) -> &str {
        "unknown"
    }

    fn name(&self) -> &str {
        "Unknown"
    }
}
