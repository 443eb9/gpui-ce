use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
};

use anyhow::Result;
use futures::channel::oneshot;
use gpui::{
    Action, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle, ForegroundExecutor,
    Keymap, Menu, MenuItem, PathPromptOptions, Platform, PlatformDisplay, PlatformKeyboardLayout,
    PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow, Task, ThermalState,
    WindowAppearance, WindowParams,
};

pub struct WinitUnifiedPlatform {}

impl WinitUnifiedPlatform {
    pub fn new(headless: bool) -> Self {
        WinitUnifiedPlatform {}
    }
}

impl Platform for WinitUnifiedPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        todo!()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        todo!()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        todo!()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        todo!()
    }

    fn quit(&self) {
        todo!()
    }

    fn restart(&self, binary_path: Option<PathBuf>) {
        todo!()
    }

    fn activate(&self, ignoring_other_apps: bool) {
        todo!()
    }

    fn hide(&self) {
        todo!()
    }

    fn hide_other_apps(&self) {
        todo!()
    }

    fn unhide_other_apps(&self) {
        todo!()
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        todo!()
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        todo!()
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        todo!()
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        todo!()
    }

    fn window_appearance(&self) -> WindowAppearance {
        todo!()
    }

    fn open_url(&self, url: &str) {
        todo!()
    }

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        todo!()
    }

    fn register_url_scheme(&self, url: &str) -> Task<Result<()>> {
        todo!()
    }

    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        todo!()
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        todo!()
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        todo!()
    }

    fn reveal_path(&self, path: &Path) {
        todo!()
    }

    fn open_with_system(&self, path: &Path) {
        todo!()
    }

    fn on_quit(&self, callback: Box<dyn FnMut()>) {
        todo!()
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        todo!()
    }

    fn set_menus(&self, menus: Vec<Menu>, keymap: &Keymap) {
        todo!()
    }

    fn set_dock_menu(&self, menu: Vec<MenuItem>, keymap: &Keymap) {
        todo!()
    }

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        todo!()
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        todo!()
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        todo!()
    }

    fn thermal_state(&self) -> ThermalState {
        todo!()
    }

    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        todo!()
    }

    fn app_path(&self) -> Result<PathBuf> {
        todo!()
    }

    fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        todo!()
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        todo!()
    }

    fn hide_cursor_until_mouse_moves(&self) {
        todo!()
    }

    fn is_cursor_visible(&self) -> bool {
        todo!()
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        todo!()
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        todo!()
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        todo!()
    }

    fn write_credentials(&self, url: &str, username: &str, password: &[u8]) -> Task<Result<()>> {
        todo!()
    }

    fn read_credentials(&self, url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        todo!()
    }

    fn delete_credentials(&self, url: &str) -> Task<Result<()>> {
        todo!()
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        todo!()
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        todo!()
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        todo!()
    }
}
