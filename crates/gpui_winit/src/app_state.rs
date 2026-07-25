use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    sync::mpsc::Receiver,
};

use gpui::{AnyWindowHandle, PriorityQueueReceiver, RunnableVariant};
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow},
    window::WindowId,
};

use crate::{
    dispatcher::execute_runnable,
    display::WinitDisplay,
    input::{current_capslock, modifiers_from_winit},
    window::WindowState,
};

pub(crate) enum LoopCommand {
    CloseWindow(WindowId),
    Quit,
}

#[derive(Default)]
pub(crate) struct WindowRegistry {
    pub(crate) windows: HashMap<WindowId, Rc<WindowState>>,
    pub(crate) displays: Vec<Rc<WinitDisplay>>,
    pub(crate) display_ids: HashMap<uuid::Uuid, gpui::DisplayId>,
    pub(crate) native_display_ids: HashMap<u64, gpui::DisplayId>,
    pub(crate) primary_display: Option<gpui::DisplayId>,
    pub(crate) active_window: Option<AnyWindowHandle>,
    pub(crate) last_active_window: Option<AnyWindowHandle>,
    pub(crate) hidden_windows: Vec<AnyWindowHandle>,
}

type ActiveEventLoopPointer = *const (dyn ActiveEventLoop + 'static);

thread_local! {
    static ACTIVE_EVENT_LOOP: Cell<Option<ActiveEventLoopPointer>> = Cell::new(None);
}

struct ActiveEventLoopReset(Option<ActiveEventLoopPointer>);

impl Drop for ActiveEventLoopReset {
    fn drop(&mut self) {
        ACTIVE_EVENT_LOOP.with(|active| active.set(self.0));
    }
}

pub(crate) fn with_event_loop_scope<R>(
    event_loop: &dyn ActiveEventLoop,
    callback: impl FnOnce() -> R,
) -> R {
    let pointer = event_loop as *const dyn ActiveEventLoop;
    // SAFETY: The pointer is cleared by the scope guard before event_loop can expire.
    let pointer = unsafe {
        std::mem::transmute::<*const dyn ActiveEventLoop, ActiveEventLoopPointer>(pointer)
    };
    let previous = ACTIVE_EVENT_LOOP.with(|active| active.replace(Some(pointer)));
    let _reset = ActiveEventLoopReset(previous);
    callback()
}

pub(crate) fn with_active_event_loop<R>(
    callback: impl FnOnce(&dyn ActiveEventLoop) -> R,
) -> Option<R> {
    ACTIVE_EVENT_LOOP.with(|active| {
        let pointer = active.get()?;
        // SAFETY: with_event_loop_scope limits this pointer to the active callback.
        Some(unsafe { callback(&*pointer) })
    })
}

pub(crate) fn refresh_displays(
    registry: &Rc<RefCell<WindowRegistry>>,
    event_loop: &dyn ActiveEventLoop,
) {
    let primary_uuid = event_loop
        .primary_monitor()
        .map(|monitor| WinitDisplay::uuid_for_monitor(&monitor));
    let monitors = event_loop.available_monitors().collect::<Vec<_>>();
    let mut registry = registry.borrow_mut();
    let mut used_ids = registry
        .display_ids
        .values()
        .copied()
        .collect::<HashSet<_>>();
    let mut displays = Vec::with_capacity(monitors.len());
    let mut primary_display = None;

    for monitor in monitors {
        let uuid = WinitDisplay::uuid_for_monitor(&monitor);
        let native_id = monitor.native_id();
        let id = registry
            .display_ids
            .get(&uuid)
            .copied()
            .or_else(|| registry.native_display_ids.get(&native_id).copied())
            .unwrap_or_else(|| {
                let mut candidate = native_id;
                if candidate == 0 || used_ids.contains(&gpui::DisplayId::new(candidate)) {
                    let mut uuid_prefix = [0; 8];
                    uuid_prefix.copy_from_slice(&uuid.as_bytes()[..8]);
                    candidate = u64::from_le_bytes(uuid_prefix).max(1);
                    while used_ids.contains(&gpui::DisplayId::new(candidate)) {
                        candidate = candidate.wrapping_add(1).max(1);
                    }
                }
                gpui::DisplayId::new(candidate)
            });
        used_ids.insert(id);
        registry.display_ids.insert(uuid, id);
        registry.native_display_ids.insert(native_id, id);
        if Some(uuid) == primary_uuid {
            primary_display = Some(id);
        }
        displays.push(WinitDisplay::from_monitor(&monitor, id));
    }

    registry.displays = displays;
    registry.primary_display = primary_display;
}

pub(crate) struct WinitAppState {
    registry: Rc<RefCell<WindowRegistry>>,
    main_receiver: PriorityQueueReceiver<RunnableVariant>,
    command_receiver: Receiver<LoopCommand>,
    on_finish_launching: Option<Box<dyn FnOnce()>>,
    headless: bool,
}

impl WinitAppState {
    pub(crate) fn new(
        registry: Rc<RefCell<WindowRegistry>>,
        main_receiver: PriorityQueueReceiver<RunnableVariant>,
        command_receiver: Receiver<LoopCommand>,
        on_finish_launching: Box<dyn FnOnce()>,
        headless: bool,
    ) -> Self {
        Self {
            registry,
            main_receiver,
            command_receiver,
            on_finish_launching: Some(on_finish_launching),
            headless,
        }
    }

    fn refresh_displays(&self, event_loop: &dyn ActiveEventLoop) {
        refresh_displays(&self.registry, event_loop);
    }

    fn drain_queues(&mut self, event_loop: &dyn ActiveEventLoop) {
        while let Ok(Some(runnable)) = self.main_receiver.try_pop() {
            execute_runnable(runnable);
        }

        while let Ok(command) = self.command_receiver.try_recv() {
            match command {
                LoopCommand::CloseWindow(window_id) => {
                    if let Some(state) = self.remove_window(window_id) {
                        state.shutdown(false);
                    }
                }
                LoopCommand::Quit => {
                    self.shutdown_windows(true);
                    event_loop.exit();
                }
            }
        }
    }

    fn state(&self, window_id: WindowId) -> Option<Rc<WindowState>> {
        self.registry.borrow().windows.get(&window_id).cloned()
    }

    fn remove_window(&self, window_id: WindowId) -> Option<Rc<WindowState>> {
        let mut registry = self.registry.borrow_mut();
        let state = registry.windows.remove(&window_id);
        if let Some(state) = state.as_ref() {
            if registry.active_window == Some(state.handle) {
                registry.active_window = None;
            }
            if registry.last_active_window == Some(state.handle) {
                registry.last_active_window = None;
            }
            registry
                .hidden_windows
                .retain(|handle| *handle != state.handle);
        }
        state
    }

    fn shutdown_windows(&self, notify: bool) {
        let windows = self
            .registry
            .borrow()
            .windows
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for state in windows {
            state.shutdown(notify);
        }
        let mut registry = self.registry.borrow_mut();
        registry.windows.clear();
        registry.active_window = None;
        registry.last_active_window = None;
        registry.hidden_windows.clear();
    }
}

impl ApplicationHandler for WinitAppState {
    fn resumed(&mut self, event_loop: &dyn ActiveEventLoop) {
        with_event_loop_scope(event_loop, || self.refresh_displays(event_loop));
    }

    fn can_create_surfaces(&mut self, event_loop: &dyn ActiveEventLoop) {
        event_loop.set_control_flow(if self.headless {
            ControlFlow::Wait
        } else {
            ControlFlow::Poll
        });
        with_event_loop_scope(event_loop, || {
            self.refresh_displays(event_loop);
            if let Some(callback) = self.on_finish_launching.take() {
                callback();
            }
        });
    }

    fn proxy_wake_up(&mut self, event_loop: &dyn ActiveEventLoop) {
        with_event_loop_scope(event_loop, || self.drain_queues(event_loop));
    }

    fn window_event(
        &mut self,
        event_loop: &dyn ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        with_event_loop_scope(event_loop, || {
            let Some(state) = self.state(window_id) else {
                return;
            };

            match event {
                WindowEvent::SurfaceResized(_) => {
                    state.resized();
                }
                WindowEvent::ScaleFactorChanged { .. } => {
                    self.refresh_displays(event_loop);
                    state.resized();
                }
                WindowEvent::Moved(_) => {
                    self.refresh_displays(event_loop);
                    state.moved();
                }
                WindowEvent::DragEntered { paths, position } => {
                    state.drag_entered(paths, position);
                }
                WindowEvent::DragMoved { position } => {
                    state.drag_moved(position);
                }
                WindowEvent::DragDropped { paths, position } => {
                    state.drag_dropped(paths, position);
                }
                WindowEvent::DragLeft { .. } => {
                    state.drag_left();
                }
                WindowEvent::Focused(focused) => {
                    state.focused(focused);
                    let mut registry = self.registry.borrow_mut();
                    if focused {
                        registry.active_window = Some(state.handle);
                        registry.last_active_window = Some(state.handle);
                    } else if registry.active_window == Some(state.handle) {
                        registry.active_window = None;
                    }
                }
                WindowEvent::KeyboardInput {
                    event,
                    is_synthetic,
                    ..
                } => {
                    state.keyboard_input(event, is_synthetic);
                }
                WindowEvent::ModifiersChanged(modifiers) => {
                    state.modifiers_changed(
                        modifiers_from_winit(modifiers.state()),
                        current_capslock(),
                    );
                }
                WindowEvent::Ime(event) => {
                    state.ime(event);
                }
                WindowEvent::PointerEntered { position, .. } => {
                    state.pointer_entered(position);
                }
                WindowEvent::PointerMoved { position, .. } => {
                    state.pointer_moved(position);
                }
                WindowEvent::PointerLeft { position, .. } => {
                    state.pointer_left(position);
                }
                WindowEvent::PointerButton {
                    state: button_state,
                    position,
                    button,
                    ..
                } => {
                    state.pointer_button(button_state, position, button);
                }
                WindowEvent::MouseWheel { delta, phase, .. } => {
                    state.mouse_wheel(delta, phase);
                }
                WindowEvent::ThemeChanged(_) => {
                    state.appearance_changed();
                }
                WindowEvent::Occluded(occluded) => {
                    state.set_occluded(occluded);
                }
                WindowEvent::CloseRequested => {
                    if state.should_close() {
                        state.shutdown(true);
                        self.remove_window(window_id);
                    }
                }
                WindowEvent::Destroyed => {
                    if self.remove_window(window_id).is_some() {
                        state.shutdown(true);
                    }
                }
                WindowEvent::RedrawRequested => {
                    state.request_frame();
                }
                _ => {}
            }
        });
    }

    fn about_to_wait(&mut self, event_loop: &dyn ActiveEventLoop) {
        with_event_loop_scope(event_loop, || {
            self.drain_queues(event_loop);
            if !self.headless {
                let windows = self
                    .registry
                    .borrow()
                    .windows
                    .values()
                    .cloned()
                    .collect::<Vec<_>>();
                for state in windows {
                    state.window.request_redraw();
                }
            }
        });
    }
}
