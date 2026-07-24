use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, mpsc},
};

use anyhow::{Result, anyhow};
use futures::channel::oneshot;
use gpui::{
    Action, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle, DummyKeyboardMapper,
    ForegroundExecutor, Keymap, Menu, MenuItem, PathPromptOptions, Platform, PlatformDisplay,
    PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem, PlatformWindow, Task,
    ThermalState, WindowAppearance, WindowKind, WindowParams,
};
use winit::{
    event_loop::{EventLoop, EventLoopProxy},
    icon::RgbaIcon,
    window::{Theme, WindowAttributes, WindowButtons, WindowLevel},
};

use crate::{
    app_state::{LoopCommand, WindowRegistry, WinitAppState, with_active_event_loop},
    dispatcher::WinitDispatcher,
    keyboard::WinitKeyboardLayout,
    window::{WindowState, WinitWindow},
};

#[derive(Default)]
struct PlatformCallbacks {
    open_urls: Cell<Option<Box<dyn FnMut(Vec<String>)>>>,
    quit: Cell<Option<Box<dyn FnMut()>>>,
    reopen: Cell<Option<Box<dyn FnMut()>>>,
    app_menu_action: Cell<Option<Box<dyn FnMut(&dyn Action)>>>,
    will_open_app_menu: Cell<Option<Box<dyn FnMut()>>>,
    validate_app_menu_command: Cell<Option<Box<dyn FnMut(&dyn Action) -> bool>>>,
    keyboard_layout_change: Cell<Option<Box<dyn FnMut()>>>,
    thermal_state_change: Cell<Option<Box<dyn FnMut()>>>,
}

pub struct WinitUnifiedPlatform {
    headless: bool,
    event_loop: RefCell<Option<EventLoop>>,
    event_loop_proxy: EventLoopProxy,
    main_receiver: gpui::PriorityQueueReceiver<gpui::RunnableVariant>,
    command_sender: mpsc::Sender<LoopCommand>,
    command_receiver: RefCell<Option<mpsc::Receiver<LoopCommand>>>,
    background_executor: BackgroundExecutor,
    foreground_executor: ForegroundExecutor,
    text_system: Arc<dyn PlatformTextSystem>,
    registry: Rc<RefCell<WindowRegistry>>,
    callbacks: Rc<PlatformCallbacks>,
    cursor_style: Cell<CursorStyle>,
    cursor_visible: Rc<Cell<bool>>,
}

impl WinitUnifiedPlatform {
    pub fn new(headless: bool) -> Self {
        let event_loop = EventLoop::new().expect("failed to initialize winit event loop");
        let event_loop_proxy = event_loop.create_proxy();
        let (dispatcher, main_receiver) = WinitDispatcher::new(event_loop_proxy.clone());
        let (command_sender, command_receiver) = mpsc::channel();

        Self {
            headless,
            event_loop: RefCell::new(Some(event_loop)),
            event_loop_proxy,
            main_receiver,
            command_sender,
            command_receiver: RefCell::new(Some(command_receiver)),
            background_executor: BackgroundExecutor::new(dispatcher.clone()),
            foreground_executor: ForegroundExecutor::new(dispatcher),
            text_system: Arc::new(gpui_wgpu::CosmicTextSystem::new("Segoe UI")),
            registry: Rc::new(RefCell::new(WindowRegistry::default())),
            callbacks: Rc::new(PlatformCallbacks::default()),
            cursor_style: Cell::new(CursorStyle::Arrow),
            cursor_visible: Rc::new(Cell::new(true)),
        }
    }

    fn send_command(&self, command: LoopCommand) {
        if self.command_sender.send(command).is_ok() {
            self.event_loop_proxy.wake_up();
        }
    }

    fn unsupported_receiver<T>(&self, operation: &str) -> oneshot::Receiver<Result<Option<T>>> {
        let (sender, receiver) = oneshot::channel();
        let _ = sender.send(Err(anyhow!("{operation} is not supported by gpui_winit")));
        receiver
    }

    fn window_appearance_from_theme(theme: Option<Theme>) -> WindowAppearance {
        match theme {
            Some(Theme::Dark) => WindowAppearance::Dark,
            _ => WindowAppearance::Light,
        }
    }
}

impl Platform for WinitUnifiedPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        let event_loop = self
            .event_loop
            .borrow_mut()
            .take()
            .expect("winit application is already running");
        let command_receiver = self
            .command_receiver
            .borrow_mut()
            .take()
            .expect("winit application is already running");
        let state = WinitAppState::new(
            self.registry.clone(),
            self.main_receiver.clone(),
            command_receiver,
            on_finish_launching,
            self.headless,
        );

        if let Err(error) = event_loop.run_app(state) {
            log::error!("winit event loop failed: {error}");
        }

        {
            let mut registry = self.registry.borrow_mut();
            registry.windows.clear();
            registry.active_window = None;
        }

        if let Some(mut callback) = self.callbacks.quit.take() {
            callback();
            self.callbacks.quit.set(Some(callback));
        }
    }

    fn quit(&self) {
        self.send_command(LoopCommand::Quit);
    }

    fn restart(&self, binary_path: Option<PathBuf>) {
        let executable = binary_path.or_else(|| std::env::current_exe().ok());
        if let Some(executable) = executable {
            let mut command = std::process::Command::new(executable);
            command.args(std::env::args_os().skip(1));
            if command.spawn().is_ok() {
                self.quit();
            }
        }
    }

    fn activate(&self, _ignoring_other_apps: bool) {
        let registry = self.registry.borrow();
        let state = registry
            .active_window
            .and_then(|handle| {
                registry
                    .windows
                    .values()
                    .find(|state| state.handle == handle)
            })
            .or_else(|| registry.windows.values().next());
        if let Some(state) = state {
            state.window.set_visible(true);
            state.window.focus_window();
        }
    }

    fn hide(&self) {
        for state in self.registry.borrow().windows.values() {
            state.window.set_visible(false);
        }
    }

    fn hide_other_apps(&self) {}

    fn unhide_other_apps(&self) {}

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        self.registry
            .borrow()
            .displays
            .iter()
            .cloned()
            .map(|display| display as Rc<dyn PlatformDisplay>)
            .collect()
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        let registry = self.registry.borrow();
        let primary = registry.primary_display?;
        registry
            .displays
            .iter()
            .find(|display| gpui::PlatformDisplay::id(display.as_ref()) == primary)
            .cloned()
            .map(|display| display as Rc<dyn PlatformDisplay>)
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.registry.borrow().active_window
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        with_active_event_loop(|event_loop| {
            let title = options
                .titlebar
                .as_ref()
                .and_then(|titlebar| titlebar.title.as_ref())
                .map(ToString::to_string)
                .unwrap_or_else(|| "GPUI".to_string());
            let mut buttons = WindowButtons::all();
            if !options.is_minimizable {
                buttons.remove(WindowButtons::MINIMIZE);
            }
            let level = match options.kind {
                WindowKind::PopUp | WindowKind::Floating => WindowLevel::AlwaysOnTop,
                _ => WindowLevel::Normal,
            };
            let mut attributes = WindowAttributes::default()
                .with_title(title.clone())
                .with_surface_size(winit::dpi::LogicalSize::new(
                    options.bounds.size.width.as_f32() as f64,
                    options.bounds.size.height.as_f32() as f64,
                ))
                .with_position(winit::dpi::LogicalPosition::new(
                    options.bounds.origin.x.as_f32() as f64,
                    options.bounds.origin.y.as_f32() as f64,
                ))
                .with_resizable(options.is_resizable)
                .with_enabled_buttons(buttons)
                .with_window_level(level)
                .with_visible(options.show)
                .with_active(options.focus);

            if let Some(min_size) = options.window_min_size {
                attributes = attributes.with_min_surface_size(winit::dpi::LogicalSize::new(
                    min_size.width.as_f32() as f64,
                    min_size.height.as_f32() as f64,
                ));
            }

            if let Some(icon) = options.icon {
                if let Ok(icon) = RgbaIcon::new(icon.as_raw().clone(), icon.width(), icon.height())
                {
                    attributes = attributes.with_window_icon(Some(icon.into()));
                }
            }

            let window: Arc<dyn winit::window::Window> = event_loop
                .create_window(attributes)
                .map(Arc::from)
                .map_err(|error| anyhow!("failed to create winit window: {error}"))?;
            let state = WindowState::new(window, handle, self.cursor_visible.clone(), title);
            state.set_cursor(self.cursor_style.get());
            let window_id = state.window.id();
            self.registry
                .borrow_mut()
                .windows
                .insert(window_id, state.clone());
            state.window.request_redraw();

            Ok(Box::new(WinitWindow::new(
                state,
                self.command_sender.clone(),
                self.event_loop_proxy.clone(),
            )) as Box<dyn PlatformWindow>)
        })
        .ok_or_else(|| anyhow!("open_window must be called from the winit event loop"))?
    }

    fn window_appearance(&self) -> WindowAppearance {
        let theme = self
            .registry
            .borrow()
            .windows
            .values()
            .next()
            .and_then(|state| state.window.theme());
        Self::window_appearance_from_theme(theme)
    }

    fn open_url(&self, _url: &str) {}

    fn on_open_urls(&self, callback: Box<dyn FnMut(Vec<String>)>) {
        self.callbacks.open_urls.set(Some(callback));
    }

    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow!(
            "URL scheme registration is not supported by gpui_winit"
        )))
    }

    fn prompt_for_paths(
        &self,
        _options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        self.unsupported_receiver("path prompts")
    }

    fn prompt_for_new_path(
        &self,
        _directory: &Path,
        _suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        self.unsupported_receiver("save prompts")
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        false
    }

    fn reveal_path(&self, _path: &Path) {}

    fn open_with_system(&self, _path: &Path) {}

    fn on_quit(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.quit.set(Some(callback));
    }

    fn on_reopen(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.reopen.set(Some(callback));
    }

    fn set_menus(&self, _menus: Vec<Menu>, _keymap: &Keymap) {}

    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {}

    fn on_app_menu_action(&self, callback: Box<dyn FnMut(&dyn Action)>) {
        self.callbacks.app_menu_action.set(Some(callback));
    }

    fn on_will_open_app_menu(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.will_open_app_menu.set(Some(callback));
    }

    fn on_validate_app_menu_command(&self, callback: Box<dyn FnMut(&dyn Action) -> bool>) {
        self.callbacks.validate_app_menu_command.set(Some(callback));
    }

    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }

    fn on_thermal_state_change(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.thermal_state_change.set(Some(callback));
    }

    fn app_path(&self) -> Result<PathBuf> {
        Ok(std::env::current_exe()?)
    }

    fn path_for_auxiliary_executable(&self, _name: &str) -> Result<PathBuf> {
        Err(anyhow!(
            "auxiliary executables are not supported by gpui_winit"
        ))
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        self.cursor_style.set(style);
        for state in self.registry.borrow().windows.values() {
            state.set_cursor(style);
        }
    }

    fn hide_cursor_until_mouse_moves(&self) {
        self.cursor_visible.set(false);
        for state in self.registry.borrow().windows.values() {
            state.window.set_cursor_visible(false);
        }
    }

    fn is_cursor_visible(&self) -> bool {
        self.cursor_visible.get()
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        false
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        None
    }

    fn write_to_clipboard(&self, _item: ClipboardItem) {}

    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Err(anyhow!(
            "credential storage is not supported by gpui_winit"
        )))
    }

    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Err(anyhow!(
            "credential storage is not supported by gpui_winit"
        )))
    }

    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow!(
            "credential storage is not supported by gpui_winit"
        )))
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(WinitKeyboardLayout)
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }

    fn on_keyboard_layout_change(&self, callback: Box<dyn FnMut()>) {
        self.callbacks.keyboard_layout_change.set(Some(callback));
    }
}
