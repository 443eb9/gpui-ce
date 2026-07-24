use std::{
    cell::{Cell, RefCell},
    ffi::c_void,
    rc::Rc,
    sync::Arc,
};

use anyhow::Result;
use futures::channel::oneshot;
use gpui::{
    Bounds, Capslock, Decorations, DevicePixels, DispatchEventResult, GpuSpecs, Modifiers, Pixels,
    PlatformAtlas, PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow, Point,
    PromptButton, PromptLevel, RequestFrameOptions, ResizeEdge, Scene, Size, WindowAppearance,
    WindowBackgroundAppearance, WindowBounds, WindowControlArea,
};
use gpui_wgpu::{GpuContext, WgpuRenderer, WgpuSurfaceConfig, wgpu};
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WindowHandle,
};
use winit::{
    cursor::CursorIcon,
    dpi::LogicalSize,
    event_loop::EventLoopProxy,
    monitor::Fullscreen,
    window::{ResizeDirection, Theme, Window},
};

use crate::{app_state::LoopCommand, display::WinitDisplay};

#[derive(Default)]
struct WindowCallbacks {
    request_frame: Cell<Option<Box<dyn FnMut(RequestFrameOptions)>>>,
    active_status_change: Cell<Option<Box<dyn FnMut(bool)>>>,
    hover_status_change: Cell<Option<Box<dyn FnMut(bool)>>>,
    resize: Cell<Option<Box<dyn FnMut(Size<Pixels>, f32)>>>,
    moved: Cell<Option<Box<dyn FnMut()>>>,
    should_close: Cell<Option<Box<dyn FnMut() -> bool>>>,
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
    modifiers: Cell<Modifiers>,
    capslock: Cell<Capslock>,
    active: Cell<bool>,
    hovered: Cell<bool>,
    background: Cell<WindowBackgroundAppearance>,
    title: RefCell<String>,
    force_render_after_recovery: Cell<bool>,
    cursor_visible: Rc<Cell<bool>>,
}

impl WindowState {
    pub(crate) fn new(
        window: Arc<dyn Window>,
        handle: gpui::AnyWindowHandle,
        cursor_visible: Rc<Cell<bool>>,
        title: String,
        gpu_context: GpuContext,
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
            None,
        )?;

        Ok(Rc::new(Self {
            renderer: RefCell::new(renderer),
            active: Cell::new(window.has_focus()),
            window,
            raw_window,
            handle,
            callbacks: WindowCallbacks::default(),
            input_handler: RefCell::new(None),
            mouse_position: Cell::new(Point::default()),
            modifiers: Cell::new(Modifiers::default()),
            capslock: Cell::new(Capslock::default()),
            hovered: Cell::new(false),
            background: Cell::new(WindowBackgroundAppearance::Opaque),
            title: RefCell::new(title),
            force_render_after_recovery: Cell::new(false),
            cursor_visible,
        }))
    }

    fn invoke_mut<T: ?Sized>(cell: &Cell<Option<Box<T>>>, invoke: impl FnOnce(&mut T)) {
        if let Some(mut callback) = cell.take() {
            invoke(&mut callback);
            cell.set(Some(callback));
        }
    }

    pub(crate) fn request_frame(&self) {
        if let Some(mut callback) = self.callbacks.request_frame.take() {
            callback(RequestFrameOptions {
                require_presentation: true,
                force_render: self.force_render_after_recovery.replace(false),
            });
            self.callbacks.request_frame.set(Some(callback));
        }
    }

    pub(crate) fn resized(&self) {
        self.renderer
            .borrow_mut()
            .update_drawable_size(device_size(self.window.surface_size()));
        let size = self.content_size();
        let scale_factor = self.scale_factor();
        Self::invoke_mut(&self.callbacks.resize, |callback| {
            callback(size, scale_factor);
        });
    }

    pub(crate) fn moved(&self) {
        Self::invoke_mut(&self.callbacks.moved, |callback| callback());
    }

    pub(crate) fn focused(&self, focused: bool) {
        self.active.set(focused);
        Self::invoke_mut(&self.callbacks.active_status_change, |callback| {
            callback(focused);
        });
    }

    pub(crate) fn hovered(&self, hovered: bool) {
        self.hovered.set(hovered);
        Self::invoke_mut(&self.callbacks.hover_status_change, |callback| {
            callback(hovered);
        });
    }

    pub(crate) fn appearance_changed(&self) {
        Self::invoke_mut(&self.callbacks.appearance_changed, |callback| callback());
    }

    pub(crate) fn should_close(&self) -> bool {
        let mut result = true;
        Self::invoke_mut(&self.callbacks.should_close, |callback| {
            result = callback();
        });
        result
    }

    pub(crate) fn closed(&self) {
        if let Some(callback) = self.callbacks.close.take() {
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

#[derive(Debug, Clone, Copy)]
pub(crate) struct RawWinitWindow {
    window: RawWindowHandle,
    display: RawDisplayHandle,
}

unsafe impl Send for RawWinitWindow {}
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
        Ok(unsafe { WindowHandle::borrow_raw(self.window) })
    }
}

impl HasDisplayHandle for RawWinitWindow {
    fn display_handle(&self) -> std::result::Result<DisplayHandle<'_>, HandleError> {
        Ok(unsafe { DisplayHandle::borrow_raw(self.display) })
    }
}

pub(crate) struct WinitWindow {
    state: Rc<WindowState>,
    command_sender: std::sync::mpsc::Sender<LoopCommand>,
    proxy: EventLoopProxy,
}

impl WinitWindow {
    pub(crate) fn new(
        state: Rc<WindowState>,
        command_sender: std::sync::mpsc::Sender<LoopCommand>,
        proxy: EventLoopProxy,
    ) -> Self {
        Self {
            state,
            command_sender,
            proxy,
        }
    }
}

impl Drop for WinitWindow {
    fn drop(&mut self) {
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
        self.state
            .window
            .current_monitor()
            .map(|monitor| WinitDisplay::from_monitor(&monitor) as Rc<dyn PlatformDisplay>)
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
        self.state.background.set(appearance);
        let transparent = appearance != WindowBackgroundAppearance::Opaque;
        let blurred = appearance == WindowBackgroundAppearance::Blurred;
        self.state.window.set_transparent(transparent);
        self.state.window.set_blur(blurred);
        self.state
            .renderer
            .borrow_mut()
            .update_transparency(transparent);
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

    fn on_input(&self, _callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {}

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

    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.state.callbacks.close.set(Some(callback));
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.state.callbacks.appearance_changed.set(Some(callback));
    }

    fn draw(&self, scene: &Scene) {
        let mut renderer = self.state.renderer.borrow_mut();
        if renderer.device_lost() {
            if let Err(error) = renderer.recover(&self.state.raw_window) {
                log::warn!("GPU recovery failed, will retry on next frame: {error}");
            }
            self.state.force_render_after_recovery.set(true);
            self.state.window.request_redraw();
            return;
        }

        self.state.window.pre_present_notify();
        if !renderer.draw(scene) {
            self.state.window.request_redraw();
        }
        if renderer.needs_redraw() {
            self.state.force_render_after_recovery.set(true);
            self.state.window.request_redraw();
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
        let handle = self
            .window_handle()
            .expect("winit window handle unavailable");
        match handle.as_raw() {
            RawWindowHandle::Win32(handle) => {
                windows::Win32::Foundation::HWND(handle.hwnd.get() as *mut c_void)
            }
            _ => unreachable!(),
        }
    }

    fn request_decorations(&self, decorations: gpui::WindowDecorations) {
        self.state
            .window
            .set_decorations(decorations == gpui::WindowDecorations::Server);
    }

    fn start_window_move(&self) {
        let _ = self.state.window.drag_window();
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

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
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
