use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use futures::channel::oneshot;
use gpui::{
    Bounds, Capslock, Decorations, DevicePixels, DispatchEventResult, ExternalPaths, FileDropEvent,
    GpuSpecs, KeyDownEvent, KeyUpEvent, Modifiers, ModifiersChangedEvent, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, Pixels, PlatformAtlas, PlatformDisplay,
    PlatformInput, PlatformInputHandler, PlatformWindow, Point, PromptButton, PromptLevel,
    RequestFrameOptions, ResizeEdge, Scene, ScrollDelta, ScrollWheelEvent, Size, TabletTool,
    TabletToolMove, WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowControlArea,
    WindowKind,
};
use gpui_wgpu::{GpuContext, WgpuDeviceRequirements, WgpuRenderer, WgpuSurfaceConfig, wgpu};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};
#[cfg(target_os = "windows")]
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, WPARAM},
    UI::WindowsAndMessaging::{
        GetCursorPos, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTLEFT, HTRIGHT, HTTOP,
        HTTOPLEFT, HTTOPRIGHT, MSG, PM_REMOVE, PeekMessageW, PostMessageW, WM_NCLBUTTONDOWN,
    },
};
use winit::{
    cursor::CursorIcon,
    dpi::{LogicalPosition, LogicalSize},
    event::{ElementState, Ime, KeyEvent, MouseScrollDelta},
    event_loop::EventLoopProxy,
    monitor::Fullscreen,
    window::{
        ImeCapabilities, ImeEnableRequest, ImeHint, ImePurpose, ImeRequest, ImeRequestData,
        ResizeDirection, Theme, Window,
    },
};

use crate::{
    app_state::{LoopCommand, WindowRegistry},
    input::{
        ClickState, current_capslock, current_modifiers, keystroke_from_winit, logical_position,
        mouse_button_from_winit, touch_phase_from_winit, utf8_cursor_to_utf16,
    },
};

#[derive(Default)]
struct WindowCallbacks {
    request_frame: Cell<Option<Box<dyn FnMut(RequestFrameOptions)>>>,
    input: Cell<Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>>,
    active_status_change: Cell<Option<Box<dyn FnMut(bool)>>>,
    hover_status_change: Cell<Option<Box<dyn FnMut(bool)>>>,
    resize: Cell<Option<Box<dyn FnMut(Size<Pixels>, f32)>>>,
    moved: Cell<Option<Box<dyn FnMut()>>>,
    should_close: Cell<Option<Box<dyn FnMut() -> bool>>>,
    hit_test_window_control: Cell<Option<Box<dyn FnMut() -> Option<WindowControlArea>>>>,
    close: Cell<Option<Box<dyn FnOnce()>>>,
    appearance_changed: Cell<Option<Box<dyn FnMut()>>>,
}

pub(crate) struct WindowState {
    renderer: RefCell<WgpuRenderer>,
    pub(crate) window: Arc<dyn Window>,
    raw_window: RawWinitWindow,
    pub(crate) handle: gpui::AnyWindowHandle,
    callbacks: WindowCallbacks,
    input_handler: RefCell<Option<PlatformInputHandler>>,
    mouse_position: Cell<Point<Pixels>>,
    pressed_button: Cell<Option<gpui::MouseButton>>,
    pressed_window_control: Cell<Option<WindowControlArea>>,
    drag_paths: RefCell<Option<ExternalPaths>>,
    click_state: RefCell<ClickState>,
    modifiers: Cell<Modifiers>,
    capslock: Cell<Capslock>,
    active: Cell<bool>,
    hovered: Cell<bool>,
    background: Cell<WindowBackgroundAppearance>,
    title: RefCell<String>,
    kind: WindowKind,
    is_movable: bool,
    use_client_decorations: bool,
    force_render_after_recovery: Cell<bool>,
    continuous_frames: Cell<bool>,
    scheduled_redraw: Cell<bool>,
    require_presentation: Cell<bool>,
    presented_in_frame: Cell<bool>,
    idle_frame_count: Cell<u8>,
    gpu_recovery_pending: Cell<bool>,
    gpu_recovery_failures: Cell<u32>,
    gpu_recovery_after: Cell<Option<Instant>>,
    occluded: Cell<bool>,
    closing: Cell<bool>,
    ime_active: Cell<bool>,
    ime_preedit: Cell<bool>,
    ime_area: Cell<Bounds<Pixels>>,
    cursor_visible: Rc<Cell<bool>>,
}

impl WindowState {
    pub(crate) fn new(
        window: Arc<dyn Window>,
        handle: gpui::AnyWindowHandle,
        cursor_visible: Rc<Cell<bool>>,
        title: String,
        kind: WindowKind,
        is_movable: bool,
        use_client_decorations: bool,
        gpu_context: GpuContext,
        gpu_requirements: Option<WgpuDeviceRequirements>,
    ) -> Result<Rc<Self>> {
        let raw_window = RawWinitWindow::new(window.as_ref())?;
        let physical_size = window.surface_size();
        let renderer = WgpuRenderer::new(
            gpu_context,
            &raw_window,
            WgpuSurfaceConfig {
                size: device_size(physical_size),
                transparent: false,
                preferred_present_mode: Some(wgpu::PresentMode::Fifo),
            },
            None,
            gpu_requirements,
        )?;

        let state = Rc::new(Self {
            renderer: RefCell::new(renderer),
            active: Cell::new(window.has_focus()),
            window,
            raw_window,
            handle,
            callbacks: WindowCallbacks::default(),
            input_handler: RefCell::new(None),
            mouse_position: Cell::new(Point::default()),
            pressed_button: Cell::new(None),
            pressed_window_control: Cell::new(None),
            drag_paths: RefCell::new(None),
            click_state: RefCell::new(ClickState::default()),
            modifiers: Cell::new(current_modifiers()),
            capslock: Cell::new(current_capslock()),
            hovered: Cell::new(false),
            background: Cell::new(WindowBackgroundAppearance::Opaque),
            title: RefCell::new(title),
            kind,
            is_movable,
            use_client_decorations,
            force_render_after_recovery: Cell::new(false),
            continuous_frames: Cell::new(true),
            scheduled_redraw: Cell::new(false),
            require_presentation: Cell::new(true),
            presented_in_frame: Cell::new(false),
            idle_frame_count: Cell::new(0),
            gpu_recovery_pending: Cell::new(false),
            gpu_recovery_failures: Cell::new(0),
            gpu_recovery_after: Cell::new(None),
            occluded: Cell::new(false),
            closing: Cell::new(false),
            ime_active: Cell::new(false),
            ime_preedit: Cell::new(false),
            ime_area: Cell::new(Bounds::default()),
            cursor_visible,
        });
        state.enable_ime();
        Ok(state)
    }

    fn invoke_mut<T: ?Sized>(cell: &Cell<Option<Box<T>>>, invoke: impl FnOnce(&mut T)) {
        if let Some(mut callback) = cell.take() {
            invoke(&mut callback);
            cell.set(Some(callback));
        }
    }

    pub(crate) fn request_redraw(&self, require_presentation: bool) {
        let size = self.window.surface_size();
        if self.closing.get()
            || self.occluded.get()
            || size.width == 0
            || size.height == 0
            || !self.window.is_visible().unwrap_or(true)
        {
            return;
        }
        if require_presentation {
            self.require_presentation.set(true);
        }
        self.idle_frame_count.set(0);
        self.continuous_frames.set(true);
        self.window.request_redraw();
    }

    pub(crate) fn should_schedule_frame(&self) -> bool {
        let size = self.window.surface_size();
        self.continuous_frames.get()
            && !self.closing.get()
            && !self.occluded.get()
            && size.width > 0
            && size.height > 0
            && self.window.is_visible().unwrap_or(true)
    }

    pub(crate) fn schedule_frame(&self) -> bool {
        if !self.should_schedule_frame() {
            return false;
        }
        self.scheduled_redraw.set(true);
        self.window.request_redraw();
        true
    }

    pub(crate) fn redraw_requested(&self) {
        if !self.scheduled_redraw.replace(false) {
            self.require_presentation.set(true);
            self.idle_frame_count.set(0);
            self.continuous_frames.set(true);
        }
        if !self.should_schedule_frame() {
            return;
        }
        if self.gpu_recovery_pending.get()
            && self
                .gpu_recovery_after
                .get()
                .is_some_and(|retry_after| Instant::now() < retry_after)
        {
            return;
        }
        self.presented_in_frame.set(false);
        if let Some(mut callback) = self.callbacks.request_frame.take() {
            callback(RequestFrameOptions {
                require_presentation: self.require_presentation.replace(false),
                force_render: self.force_render_after_recovery.replace(false),
            });
            self.callbacks.request_frame.set(Some(callback));
        }
        if self.presented_in_frame.get() {
            self.idle_frame_count.set(0);
        } else {
            let idle_frames = self.idle_frame_count.get().saturating_add(1);
            self.idle_frame_count.set(idle_frames);
            if idle_frames >= 3 {
                self.continuous_frames.set(false);
            }
        }
    }

    pub(crate) fn resized(&self) {
        if self.closing.get() {
            return;
        }
        self.renderer
            .borrow_mut()
            .update_drawable_size(device_size(self.window.surface_size()));
        let size = self.content_size();
        let scale_factor = self.scale_factor();
        Self::invoke_mut(&self.callbacks.resize, |callback| {
            callback(size, scale_factor);
        });
        self.request_redraw(true);
    }

    pub(crate) fn moved(&self) {
        Self::invoke_mut(&self.callbacks.moved, |callback| callback());
        self.request_redraw(false);
    }

    pub(crate) fn focused(&self, focused: bool) {
        self.active.set(focused);
        if focused {
            self.modifiers_changed(current_modifiers(), current_capslock());
        } else {
            self.pressed_button.set(None);
            self.pressed_window_control.set(None);
        }
        Self::invoke_mut(&self.callbacks.active_status_change, |callback| {
            callback(focused);
        });
        self.request_redraw(false);
    }

    pub(crate) fn hovered(&self, hovered: bool) {
        if self.hovered.replace(hovered) != hovered {
            Self::invoke_mut(&self.callbacks.hover_status_change, |callback| {
                callback(hovered);
            });
            self.request_redraw(false);
        }
    }

    pub(crate) fn appearance_changed(&self) {
        Self::invoke_mut(&self.callbacks.appearance_changed, |callback| callback());
        self.request_redraw(true);
    }

    pub(crate) fn set_occluded(&self, occluded: bool) {
        self.occluded.set(occluded);
        if !occluded && !self.closing.get() {
            self.request_redraw(true);
        }
    }

    pub(crate) fn keyboard_input(&self, event: KeyEvent, is_synthetic: bool) {
        let modifiers = current_modifiers();
        let capslock = current_capslock();
        if modifiers != self.modifiers.get() || capslock != self.capslock.get() {
            self.modifiers_changed(modifiers, capslock);
        }

        let Some((keystroke, prefer_character_input)) = keystroke_from_winit(&event, modifiers)
        else {
            return;
        };
        let key_char = keystroke.key_char.clone();
        let text_modifiers = keystroke.modifiers;
        let pressed = event.state == ElementState::Pressed;
        let input = if pressed {
            PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: event.repeat,
                prefer_character_input,
            })
        } else {
            PlatformInput::KeyUp(KeyUpEvent { keystroke })
        };
        let result = self.dispatch_input(input);

        if pressed
            && !is_synthetic
            && result.propagate
            && !result.default_prevented
            && (prefer_character_input || text_modifiers.is_subset_of(&Modifiers::shift()))
            && let Some(key_char) = key_char
        {
            self.with_input_handler(|handler| handler.replace_text_in_range(None, &key_char));
        }
    }

    pub(crate) fn modifiers_changed(&self, modifiers: Modifiers, capslock: Capslock) {
        self.modifiers.set(modifiers);
        self.capslock.set(capslock);
        self.dispatch_input(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
            modifiers,
            capslock,
        }));
    }

    pub(crate) fn pointer_entered(&self, position: winit::dpi::PhysicalPosition<f64>) {
        self.restore_cursor();
        self.mouse_position
            .set(logical_position(position, self.scale_factor()));
        self.hovered(true);
    }

    pub(crate) fn pointer_moved(
        &self,
        position: winit::dpi::PhysicalPosition<f64>,
        source: winit::event::PointerSource,
    ) {
        self.restore_cursor();
        let position = logical_position(position, self.scale_factor());
        self.mouse_position.set(position);
        self.hovered(true);
        self.dispatch_input(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: self.pressed_button.get(),
            modifiers: self.modifiers.get(),
            tablet_tool: match source {
                winit::event::PointerSource::TabletTool { kind, data } => {
                    Some(TabletToolMove { kind, data })
                }
                _ => None,
            },
        }));
    }

    pub(crate) fn pointer_left(&self, position: Option<winit::dpi::PhysicalPosition<f64>>) {
        if let Some(position) = position {
            self.mouse_position
                .set(logical_position(position, self.scale_factor()));
        }
        self.hovered(false);
        self.dispatch_input(PlatformInput::MouseExited(MouseExitEvent {
            position: self.mouse_position.get(),
            pressed_button: self.pressed_button.get(),
            modifiers: self.modifiers.get(),
        }));
    }

    pub(crate) fn pointer_button(
        &self,
        state: ElementState,
        position: winit::dpi::PhysicalPosition<f64>,
        button: winit::event::ButtonSource,
    ) -> Option<WindowControlArea> {
        let tablet_tool = match button.clone() {
            winit::event::ButtonSource::TabletTool { kind, button, data } => {
                Some(TabletTool { kind, button, data })
            }
            _ => None,
        };
        let Some(button) = mouse_button_from_winit(button) else {
            return None;
        };
        let position = logical_position(position, self.scale_factor());
        self.mouse_position.set(position);
        let modifiers = self.modifiers.get();

        match state {
            ElementState::Pressed => {
                self.pressed_button.set(Some(button));
                let click_count = self.click_state.borrow_mut().update(button, position);
                let result = self.dispatch_input(PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position,
                    modifiers,
                    click_count,
                    first_mouse: false,
                    tablet_tool,
                }));
                if button == gpui::MouseButton::Left {
                    let control = self.hit_test_window_control();
                    self.pressed_window_control.set(control);
                    #[cfg(target_os = "windows")]
                    if control == Some(WindowControlArea::Drag)
                        && result.propagate
                        && !result.default_prevented
                    {
                        self.start_window_move();
                    }
                }
                None
            }
            ElementState::Released => {
                self.pressed_button.set(None);
                let click_count = self.click_state.borrow().count_for(button);
                self.dispatch_input(PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position,
                    modifiers,
                    click_count,
                    tablet_tool,
                }));
                if button != gpui::MouseButton::Left {
                    return None;
                }

                let pressed_control = self.pressed_window_control.take();
                let released_control = self.hit_test_window_control();
                pressed_control.filter(|pressed| Some(*pressed) == released_control)
            }
        }
    }

    fn hit_test_window_control(&self) -> Option<WindowControlArea> {
        let mut area = None;
        Self::invoke_mut(&self.callbacks.hit_test_window_control, |callback| {
            area = callback();
        });
        area
    }

    fn start_window_move(&self) {
        if !self.is_movable
            || matches!(
                self.pressed_window_control.get(),
                Some(WindowControlArea::Close | WindowControlArea::Max | WindowControlArea::Min)
            )
        {
            return;
        }
        let _ = self.window.drag_window();
        #[cfg(target_os = "windows")]
        self.post_valid_window_drag_message(HTCAPTION);
    }

    #[cfg(target_os = "windows")]
    fn post_valid_window_drag_message(&self, hit_test: u32) {
        let RawWindowHandle::Win32(handle) = self.raw_window.window else {
            return;
        };
        let hwnd = HWND(handle.hwnd.get() as *mut c_void);
        let mut cursor = POINT::default();
        unsafe {
            if GetCursorPos(&mut cursor).is_err() {
                return;
            }

            // winit 0.31.0-beta.2 posts a pointer to POINTS as lParam instead of the
            // packed screen coordinates expected by WM_NCLBUTTONDOWN. Replace that queued
            // message while retaining Winit's private state used to synthesize MouseUp.
            let mut malformed_message = MSG::default();
            let _ = PeekMessageW(
                &mut malformed_message,
                Some(hwnd),
                WM_NCLBUTTONDOWN,
                WM_NCLBUTTONDOWN,
                PM_REMOVE,
            );
            let position = (cursor.x as u32 & 0xffff) | ((cursor.y as u32 & 0xffff) << 16);
            let _ = PostMessageW(
                Some(hwnd),
                WM_NCLBUTTONDOWN,
                WPARAM(hit_test as usize),
                LPARAM(position as isize),
            );
        }
    }

    pub(crate) fn mouse_wheel(&self, delta: MouseScrollDelta, phase: winit::event::TouchPhase) {
        let delta = match delta {
            MouseScrollDelta::LineDelta(x, y) => ScrollDelta::Lines(gpui::point(x, y)),
            MouseScrollDelta::PixelDelta(delta) => {
                let position = logical_position(delta, self.scale_factor());
                ScrollDelta::Pixels(position)
            }
        };
        self.dispatch_input(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position: self.mouse_position.get(),
            delta,
            modifiers: self.modifiers.get(),
            touch_phase: touch_phase_from_winit(phase),
        }));
    }

    pub(crate) fn drag_entered(
        &self,
        paths: Vec<std::path::PathBuf>,
        position: winit::dpi::PhysicalPosition<f64>,
    ) {
        let paths = ExternalPaths(paths.into_iter().collect());
        self.drag_paths.borrow_mut().replace(paths.clone());
        self.dispatch_input(PlatformInput::FileDrop(FileDropEvent::Entered {
            position: logical_position(position, self.scale_factor()),
            paths,
        }));
    }

    pub(crate) fn drag_moved(&self, position: winit::dpi::PhysicalPosition<f64>) {
        if self.drag_paths.borrow().is_none() {
            return;
        }
        self.dispatch_input(PlatformInput::FileDrop(FileDropEvent::Pending {
            position: logical_position(position, self.scale_factor()),
        }));
    }

    pub(crate) fn drag_dropped(
        &self,
        paths: Vec<std::path::PathBuf>,
        position: winit::dpi::PhysicalPosition<f64>,
    ) {
        self.drag_paths
            .borrow_mut()
            .replace(ExternalPaths(paths.into_iter().collect()));
        self.dispatch_input(PlatformInput::FileDrop(FileDropEvent::Submit {
            position: logical_position(position, self.scale_factor()),
        }));
        self.drag_paths.borrow_mut().take();
    }

    pub(crate) fn drag_left(&self) {
        if self.drag_paths.borrow_mut().take().is_some() {
            self.dispatch_input(PlatformInput::FileDrop(FileDropEvent::Exited));
        }
    }

    pub(crate) fn ime(&self, event: Ime) {
        match event {
            Ime::Enabled => {
                self.ime_active.set(true);
                self.ime_preedit.set(false);
                self.update_ime_area();
            }
            Ime::Preedit(text, cursor) => {
                self.ime_preedit.set(!text.is_empty());
                let selection = cursor.map(|(start, end)| {
                    let start = utf8_cursor_to_utf16(&text, start);
                    let end = utf8_cursor_to_utf16(&text, end);
                    start.min(end)..start.max(end)
                });
                let bounds = self
                    .with_input_handler(|handler| {
                        handler.replace_and_mark_text_in_range(None, &text, selection);
                        let selection = handler.selected_text_range(true)?;
                        let cursor = if selection.reversed {
                            selection.range.start
                        } else {
                            selection.range.end
                        };
                        handler.bounds_for_range(cursor..cursor)
                    })
                    .flatten();
                if let Some(bounds) = bounds {
                    self.ime_area.set(bounds);
                    self.update_ime_area();
                }
            }
            Ime::Commit(text) => {
                self.ime_preedit.set(false);
                self.with_input_handler(|handler| {
                    handler.replace_text_in_range(None, &text);
                    handler.unmark_text();
                });
            }
            Ime::Disabled => {
                self.ime_active.set(false);
                let had_preedit = self.ime_preedit.replace(false);
                self.with_input_handler(|handler| {
                    if had_preedit {
                        handler.replace_and_mark_text_in_range(None, "", None);
                    }
                    handler.unmark_text();
                });
            }
            Ime::DeleteSurrounding { .. } => {
                // TODO(winit): Apply IME surrounding-text deletion.
            }
        }
        self.request_redraw(false);
    }

    pub(crate) fn should_close(&self) -> bool {
        let mut result = true;
        Self::invoke_mut(&self.callbacks.should_close, |callback| {
            result = callback();
        });
        result
    }

    pub(crate) fn shutdown(&self, notify: bool) {
        if self.closing.replace(true) {
            return;
        }
        self.continuous_frames.set(false);
        self.renderer.borrow_mut().destroy();
        self.input_handler.borrow_mut().take();
        self.drag_paths.borrow_mut().take();
        if notify && let Some(callback) = self.callbacks.close.take() {
            callback();
        }
    }

    pub(crate) fn restore_cursor(&self) {
        if !self.cursor_visible.replace(true) {
            self.window.set_cursor_visible(true);
        }
    }

    pub(crate) fn set_cursor(&self, style: gpui::CursorStyle) {
        self.window.set_cursor(cursor_icon(style).into());
    }

    fn dispatch_input(&self, input: PlatformInput) -> DispatchEventResult {
        let Some(mut callback) = self.callbacks.input.take() else {
            return DispatchEventResult {
                propagate: true,
                default_prevented: false,
            };
        };
        let result = callback(input);
        self.callbacks.input.set(Some(callback));
        self.request_redraw(false);
        result
    }

    fn with_input_handler<R>(
        &self,
        callback: impl FnOnce(&mut PlatformInputHandler) -> R,
    ) -> Option<R> {
        let mut handler = self.input_handler.borrow_mut().take()?;
        let result = callback(&mut handler);
        self.input_handler.borrow_mut().replace(handler);
        Some(result)
    }

    fn enable_ime(&self) {
        let capabilities = ImeCapabilities::new()
            .with_hint_and_purpose()
            .with_cursor_area();
        let request_data = ImeRequestData::default()
            .with_hint_and_purpose(ImeHint::NONE, ImePurpose::Normal)
            .with_cursor_area(
                LogicalPosition::new(0.0, 0.0).into(),
                LogicalSize::new(0.0, 0.0).into(),
            );
        if let Some(request) = ImeEnableRequest::new(capabilities, request_data) {
            let _ = self.window.request_ime_update(ImeRequest::Enable(request));
        }
    }

    fn update_ime_area(&self) {
        if !self.ime_active.get() {
            return;
        }
        let bounds = self.ime_area.get();
        let request = ImeRequestData::default().with_cursor_area(
            LogicalPosition::new(
                bounds.origin.x.as_f32() as f64,
                bounds.origin.y.as_f32() as f64,
            )
            .into(),
            LogicalSize::new(
                bounds.size.width.as_f32() as f64,
                bounds.size.height.as_f32() as f64,
            )
            .into(),
        );
        let _ = self.window.request_ime_update(ImeRequest::Update(request));
    }

    fn content_size(&self) -> Size<Pixels> {
        let scale = self.scale_factor();
        let physical = self.window.surface_size();
        gpui::size(
            gpui::px(physical.width as f32 / scale),
            gpui::px(physical.height as f32 / scale),
        )
    }

    fn scale_factor(&self) -> f32 {
        self.window.scale_factor() as f32
    }
}

impl Drop for WindowState {
    fn drop(&mut self) {
        self.renderer.get_mut().destroy();
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct RawWinitWindow {
    window: RawWindowHandle,
    display: RawDisplayHandle,
}

// SAFETY: WindowState keeps the native window alive while these handles are used.
unsafe impl Send for RawWinitWindow {}
// SAFETY: Wgpu only reads the handles while WindowState owns the native window.
unsafe impl Sync for RawWinitWindow {}

impl RawWinitWindow {
    fn new(window: &dyn Window) -> std::result::Result<Self, HandleError> {
        Ok(Self {
            window: window.window_handle()?.as_raw(),
            display: window.display_handle()?.as_raw(),
        })
    }
}

impl HasWindowHandle for RawWinitWindow {
    fn window_handle(&self) -> std::result::Result<WindowHandle<'_>, HandleError> {
        // SAFETY: The borrowed handle cannot outlive RawWinitWindow.
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

impl HasDisplayHandle for RawWinitWindow {
    fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
        // SAFETY: The borrowed handle cannot outlive RawWinitWindow.
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

pub(crate) struct WinitWindow {
    state: Rc<WindowState>,
    registry: Rc<RefCell<WindowRegistry>>,
    command_sender: std::sync::mpsc::Sender<LoopCommand>,
    proxy: EventLoopProxy,
}

impl WinitWindow {
    pub(crate) fn new(
        state: Rc<WindowState>,
        registry: Rc<RefCell<WindowRegistry>>,
        command_sender: std::sync::mpsc::Sender<LoopCommand>,
        proxy: EventLoopProxy,
    ) -> Self {
        Self {
            state,
            registry,
            command_sender,
            proxy,
        }
    }
}

impl Drop for WinitWindow {
    fn drop(&mut self) {
        self.state.shutdown(false);
        let _ = self
            .command_sender
            .send(LoopCommand::CloseWindow(self.state.window.id()));
        self.proxy.wake_up();
    }
}

impl HasWindowHandle for WinitWindow {
    fn window_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError>
    {
        self.state.window.window_handle()
    }
}

impl HasDisplayHandle for WinitWindow {
    fn display_handle(
        &self,
    ) -> std::result::Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError>
    {
        self.state.window.display_handle()
    }
}

impl PlatformWindow for WinitWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        let scale = self.scale_factor();
        let origin = self
            .state
            .window
            .outer_position()
            .map(|position| {
                gpui::point(
                    gpui::px(position.x as f32 / scale),
                    gpui::px(position.y as f32 / scale),
                )
            })
            .unwrap_or_default();
        Bounds::new(origin, self.content_size())
    }

    fn is_maximized(&self) -> bool {
        self.state.window.is_maximized()
    }

    fn window_bounds(&self) -> WindowBounds {
        let bounds = self.bounds();
        if self.is_fullscreen() {
            WindowBounds::Fullscreen(bounds)
        } else if self.is_maximized() {
            WindowBounds::Maximized(bounds)
        } else {
            WindowBounds::Windowed(bounds)
        }
    }

    fn content_size(&self) -> Size<Pixels> {
        self.state.content_size()
    }

    fn resize(&mut self, size: Size<Pixels>) {
        let _ = self.state.window.request_surface_size(
            LogicalSize::new(size.width.as_f32() as f64, size.height.as_f32() as f64).into(),
        );
    }

    fn scale_factor(&self) -> f32 {
        self.state.scale_factor()
    }

    fn appearance(&self) -> WindowAppearance {
        match self.state.window.theme() {
            Some(Theme::Dark) => WindowAppearance::Dark,
            _ => WindowAppearance::Light,
        }
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        let monitor = self.state.window.current_monitor()?;
        self.registry
            .borrow()
            .displays
            .iter()
            .find(|display| display.matches_monitor(&monitor))
            .cloned()
            .map(|display| display as Rc<dyn PlatformDisplay>)
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.state.mouse_position.get()
    }

    fn modifiers(&self) -> Modifiers {
        self.state.modifiers.get()
    }

    fn capslock(&self) -> Capslock {
        self.state.capslock.get()
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        *self.state.input_handler.borrow_mut() = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.state.input_handler.borrow_mut().take()
    }

    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<oneshot::Receiver<usize>> {
        // TODO(winit): Support native prompt dialogs.
        None
    }

    fn activate(&self) {
        self.state.window.set_visible(true);
        self.state.window.focus_window();
    }

    fn is_active(&self) -> bool {
        self.state.active.get()
    }

    fn is_hovered(&self) -> bool {
        self.state.hovered.get()
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        self.state.background.get()
    }

    fn set_title(&mut self, title: &str) {
        self.state.window.set_title(title);
        *self.state.title.borrow_mut() = title.to_string();
    }

    fn set_background_appearance(&self, appearance: WindowBackgroundAppearance) {
        if self.state.closing.get() || self.state.background.replace(appearance) == appearance {
            return;
        }
        let transparent = appearance != WindowBackgroundAppearance::Opaque;
        let blurred = appearance == WindowBackgroundAppearance::Blurred;
        if matches!(
            appearance,
            WindowBackgroundAppearance::MicaBackdrop | WindowBackgroundAppearance::MicaAltBackdrop
        ) {
            // TODO(winit): Apply native Windows Mica backdrops.
        }
        self.state.window.set_transparent(transparent);
        self.state.window.set_blur(blurred);
        if !self.state.gpu_recovery_pending.get() && !self.state.closing.get() {
            self.state
                .renderer
                .borrow_mut()
                .update_transparency(transparent);
        }
    }

    fn minimize(&self) {
        self.state.window.set_minimized(true);
    }

    fn zoom(&self) {
        self.state
            .window
            .set_maximized(!self.state.window.is_maximized());
    }

    fn toggle_fullscreen(&self) {
        if self.state.window.fullscreen().is_some() {
            self.state.window.set_fullscreen(None);
        } else {
            self.state
                .window
                .set_fullscreen(Some(Fullscreen::Borderless(None)));
        }
    }

    fn is_fullscreen(&self) -> bool {
        self.state.window.fullscreen().is_some()
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.state.callbacks.request_frame.set(Some(callback));
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.state.callbacks.input.set(Some(callback));
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.state
            .callbacks
            .active_status_change
            .set(Some(callback));
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.state.callbacks.hover_status_change.set(Some(callback));
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.state.callbacks.resize.set(Some(callback));
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.state.callbacks.moved.set(Some(callback));
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.state.callbacks.should_close.set(Some(callback));
    }

    fn on_hit_test_window_control(&self, callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
        self.state
            .callbacks
            .hit_test_window_control
            .set(Some(callback));
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.state.callbacks.close.set(Some(callback));
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.state.callbacks.appearance_changed.set(Some(callback));
    }

    fn draw(&self, scene: &Scene) {
        let surface_size = self.state.window.surface_size();
        if self.state.closing.get()
            || self.state.occluded.get()
            || surface_size.width == 0
            || surface_size.height == 0
        {
            return;
        }

        let mut renderer = self.state.renderer.borrow_mut();
        if renderer.device_lost() || self.state.gpu_recovery_pending.get() {
            let now = Instant::now();
            if self
                .state
                .gpu_recovery_after
                .get()
                .is_some_and(|retry_after| now < retry_after)
            {
                return;
            }

            self.state.gpu_recovery_pending.set(true);
            match renderer.recover(&self.state.raw_window) {
                Ok(()) => {
                    self.state.gpu_recovery_pending.set(false);
                    self.state.gpu_recovery_failures.set(0);
                    self.state.gpu_recovery_after.set(None);
                    renderer.update_drawable_size(device_size(surface_size));
                    renderer.update_transparency(
                        self.state.background.get() != WindowBackgroundAppearance::Opaque,
                    );
                    self.state.force_render_after_recovery.set(true);
                    self.state.request_redraw(true);
                }
                Err(error) => {
                    let failures = self.state.gpu_recovery_failures.get().saturating_add(1);
                    self.state.gpu_recovery_failures.set(failures);
                    let delay = Duration::from_secs(1 << failures.saturating_sub(1).min(3));
                    self.state.gpu_recovery_after.set(Some(now + delay));
                    log::warn!(
                        "GPU recovery failed; retrying in {}s: {error}",
                        delay.as_secs()
                    );
                }
            }
            return;
        }

        self.state.presented_in_frame.set(true);
        self.state.window.pre_present_notify();
        if !renderer.draw(scene) {
            self.state.request_redraw(true);
        }
        if renderer.needs_redraw() {
            self.state.force_render_after_recovery.set(true);
            self.state.request_redraw(true);
        }
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.state.renderer.borrow().sprite_atlas().clone()
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        self.state.renderer.borrow().supports_dual_source_blending()
    }

    fn get_title(&self) -> String {
        self.state.title.borrow().clone()
    }

    #[cfg(target_os = "windows")]
    fn get_raw_handle(&self) -> windows::Win32::Foundation::HWND {
        let Ok(handle) = self.window_handle() else {
            log::error!("winit window handle is unavailable");
            return windows::Win32::Foundation::HWND::default();
        };
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            log::error!("winit returned a non-Win32 handle on Windows");
            return windows::Win32::Foundation::HWND::default();
        };
        windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut c_void)
    }

    fn request_decorations(&self, decorations: gpui::WindowDecorations) {
        let use_server_decorations = decorations == gpui::WindowDecorations::Server
            && !self.state.use_client_decorations
            && self.state.kind != WindowKind::PopUp;
        self.state.window.set_decorations(use_server_decorations);
    }

    fn start_window_move(&self) {
        self.state.start_window_move();
    }

    fn start_window_resize(&self, edge: ResizeEdge) {
        let direction = match edge {
            ResizeEdge::Top => ResizeDirection::North,
            ResizeEdge::TopRight => ResizeDirection::NorthEast,
            ResizeEdge::Right => ResizeDirection::East,
            ResizeEdge::BottomRight => ResizeDirection::SouthEast,
            ResizeEdge::Bottom => ResizeDirection::South,
            ResizeEdge::BottomLeft => ResizeDirection::SouthWest,
            ResizeEdge::Left => ResizeDirection::West,
            ResizeEdge::TopLeft => ResizeDirection::NorthWest,
        };
        let _ = self.state.window.drag_resize_window(direction);
        #[cfg(target_os = "windows")]
        self.state.post_valid_window_drag_message(match direction {
            ResizeDirection::North => HTTOP,
            ResizeDirection::NorthEast => HTTOPRIGHT,
            ResizeDirection::East => HTRIGHT,
            ResizeDirection::SouthEast => HTBOTTOMRIGHT,
            ResizeDirection::South => HTBOTTOM,
            ResizeDirection::SouthWest => HTBOTTOMLEFT,
            ResizeDirection::West => HTLEFT,
            ResizeDirection::NorthWest => HTTOPLEFT,
        });
    }

    fn window_decorations(&self) -> Decorations {
        if self.state.window.is_decorated() {
            Decorations::Server
        } else {
            Decorations::Client {
                tiling: gpui::Tiling::default(),
            }
        }
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        Some(self.state.renderer.borrow().gpu_specs())
    }

    #[cfg(any(target_os = "windows", target_os = "linux", target_os = "freebsd"))]
    fn gpu_context(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(self.state.renderer.borrow().gpu_context()))
    }

    fn update_ime_position(&self, bounds: Bounds<Pixels>) {
        self.state.ime_area.set(bounds);
        self.state.update_ime_area();
    }
}

fn device_size(size: winit::dpi::PhysicalSize<u32>) -> Size<DevicePixels> {
    Size {
        width: DevicePixels(size.width.min(i32::MAX as u32) as i32),
        height: DevicePixels(size.height.min(i32::MAX as u32) as i32),
    }
}

pub(crate) fn cursor_icon(style: gpui::CursorStyle) -> CursorIcon {
    match style {
        gpui::CursorStyle::Arrow => CursorIcon::Default,
        gpui::CursorStyle::IBeam => CursorIcon::Text,
        gpui::CursorStyle::Crosshair => CursorIcon::Crosshair,
        gpui::CursorStyle::ClosedHand => CursorIcon::Grabbing,
        gpui::CursorStyle::OpenHand => CursorIcon::Grab,
        gpui::CursorStyle::PointingHand => CursorIcon::Pointer,
        gpui::CursorStyle::ResizeLeft => CursorIcon::WResize,
        gpui::CursorStyle::ResizeRight => CursorIcon::EResize,
        gpui::CursorStyle::ResizeLeftRight => CursorIcon::EwResize,
        gpui::CursorStyle::ResizeUp => CursorIcon::NResize,
        gpui::CursorStyle::ResizeDown => CursorIcon::SResize,
        gpui::CursorStyle::ResizeUpDown => CursorIcon::NsResize,
        gpui::CursorStyle::ResizeUpLeftDownRight => CursorIcon::NwseResize,
        gpui::CursorStyle::ResizeUpRightDownLeft => CursorIcon::NeswResize,
        gpui::CursorStyle::ResizeColumn => CursorIcon::ColResize,
        gpui::CursorStyle::ResizeRow => CursorIcon::RowResize,
        gpui::CursorStyle::IBeamCursorForVerticalLayout => CursorIcon::VerticalText,
        gpui::CursorStyle::OperationNotAllowed => CursorIcon::NotAllowed,
        gpui::CursorStyle::DragLink => CursorIcon::Alias,
        gpui::CursorStyle::DragCopy => CursorIcon::Copy,
        gpui::CursorStyle::ContextualMenu => CursorIcon::ContextMenu,
    }
}
