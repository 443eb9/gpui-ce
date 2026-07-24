use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
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
    pub(crate) primary_display: Option<gpui::DisplayId>,
    pub(crate) active_window: Option<AnyWindowHandle>,
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
        Some(unsafe { callback(&*pointer) })
    })
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
        let primary_id = event_loop.primary_monitor().map(|monitor| monitor.id());
        let displays = event_loop
            .available_monitors()
            .map(|monitor| {
                let source_id = monitor.id();
                (source_id, WinitDisplay::from_monitor(&monitor))
            })
            .collect::<Vec<_>>();
        let primary_display = primary_id.and_then(|primary_id| {
            displays
                .iter()
                .find(|(source_id, _)| *source_id == primary_id)
                .map(|(_, display)| gpui::PlatformDisplay::id(display.as_ref()))
        });
        let mut registry = self.registry.borrow_mut();
        registry.displays = displays.into_iter().map(|(_, display)| display).collect();
        registry.primary_display = primary_display;
    }

    fn drain_queues(&mut self, event_loop: &dyn ActiveEventLoop) {
        while let Ok(Some(runnable)) = self.main_receiver.try_pop() {
            execute_runnable(runnable);
        }

        while let Ok(command) = self.command_receiver.try_recv() {
            match command {
                LoopCommand::CloseWindow(window_id) => {
                    self.remove_window(window_id);
                }
                LoopCommand::Quit => {
                    let mut registry = self.registry.borrow_mut();
                    registry.windows.clear();
                    registry.active_window = None;
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
        if state
            .as_ref()
            .is_some_and(|state| registry.active_window == Some(state.handle))
        {
            registry.active_window = None;
        }
        state
    }
}

impl ApplicationHandler for WinitAppState {
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
                WindowEvent::SurfaceResized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    state.resized();
                }
                WindowEvent::Moved(_) => {
                    self.refresh_displays(event_loop);
                    state.moved();
                }
                WindowEvent::Focused(focused) => {
                    state.focused(focused);
                    let mut registry = self.registry.borrow_mut();
                    if focused {
                        registry.active_window = Some(state.handle);
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
                WindowEvent::CloseRequested => {
                    if state.should_close() {
                        state.closed();
                        self.remove_window(window_id);
                    }
                }
                WindowEvent::Destroyed => {
                    if self.remove_window(window_id).is_some() {
                        state.closed();
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
