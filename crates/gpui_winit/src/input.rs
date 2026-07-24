use std::time::{Duration, Instant};

use gpui::{
    Capslock, Keystroke, Modifiers, MouseButton, NavigationDirection, Pixels, Point, TouchPhase,
    point, px,
};
use winit::{
    event::{ButtonSource, MouseButton as WinitMouseButton, TouchPhase as WinitTouchPhase},
    keyboard::{Key, NamedKey},
};

pub(crate) struct ClickState {
    last_button: Option<MouseButton>,
    last_position: Point<Pixels>,
    last_time: Option<Instant>,
    current_count: usize,
}

impl Default for ClickState {
    fn default() -> Self {
        Self {
            last_button: None,
            last_position: Point::default(),
            last_time: None,
            current_count: 0,
        }
    }
}

impl ClickState {
    pub(crate) fn update(&mut self, button: MouseButton, position: Point<Pixels>) -> usize {
        let now = Instant::now();
        let elapsed = self
            .last_time
            .map(|time| now.saturating_duration_since(time));
        let dx = (position.x - self.last_position.x).as_f32();
        let dy = (position.y - self.last_position.y).as_f32();
        let close_enough = dx.mul_add(dx, dy * dy) < 25.0;

        if self.last_button == Some(button)
            && elapsed.is_some_and(|elapsed| elapsed < Duration::from_millis(500))
            && close_enough
        {
            self.current_count += 1;
        } else {
            self.current_count = 1;
        }

        self.last_button = Some(button);
        self.last_position = position;
        self.last_time = Some(now);
        self.current_count
    }

    pub(crate) fn count_for(&self, button: MouseButton) -> usize {
        if self.last_button == Some(button) {
            self.current_count
        } else {
            1
        }
    }
}

pub(crate) fn modifiers_from_winit(state: winit::keyboard::ModifiersState) -> Modifiers {
    Modifiers {
        control: state.control_key(),
        alt: state.alt_key(),
        shift: state.shift_key(),
        platform: state.meta_key(),
        function: false,
    }
}

pub(crate) fn keystroke_from_winit(
    event: &winit::event::KeyEvent,
    mut modifiers: Modifiers,
) -> Option<(Keystroke, bool)> {
    let key_char = event
        .text
        .as_ref()
        .map(ToString::to_string)
        .filter(|text| !text.is_empty() && !text.chars().all(char::is_control));

    let key = match &event.logical_key {
        Key::Character(character) if character.as_str() == " " => "space".to_string(),
        Key::Character(character) => {
            let key = character.to_lowercase();
            if modifiers.shift && key.to_lowercase() == key.to_uppercase() {
                modifiers.shift = false;
            }
            key
        }
        Key::Named(named) => named_key(named)?.to_string(),
        Key::Dead(_) | Key::Unidentified(_) => return None,
    };

    let key_char = key_char.or_else(|| {
        if modifiers.control || modifiers.platform || modifiers.function || modifiers.alt {
            return None;
        }
        match &event.logical_key {
            Key::Character(character) => Some(character.to_string()),
            _ => None,
        }
    });
    let prefer_character_input = modifiers.control && modifiers.alt && key_char.is_some();

    Some((
        Keystroke {
            modifiers,
            key,
            key_char,
        },
        prefer_character_input,
    ))
}

fn named_key(key: &NamedKey) -> Option<&'static str> {
    Some(match key {
        NamedKey::Backspace => "backspace",
        NamedKey::Tab => "tab",
        NamedKey::Enter => "enter",
        NamedKey::Escape => "escape",
        NamedKey::ArrowLeft => "left",
        NamedKey::ArrowRight => "right",
        NamedKey::ArrowUp => "up",
        NamedKey::ArrowDown => "down",
        NamedKey::Home => "home",
        NamedKey::End => "end",
        NamedKey::PageUp => "pageup",
        NamedKey::PageDown => "pagedown",
        NamedKey::Insert => "insert",
        NamedKey::Delete => "delete",
        NamedKey::ContextMenu => "menu",
        NamedKey::BrowserBack => "back",
        NamedKey::BrowserForward => "forward",
        NamedKey::Copy => "copy",
        NamedKey::Cut => "cut",
        NamedKey::Paste => "paste",
        NamedKey::Undo => "undo",
        NamedKey::Redo => "redo",
        NamedKey::F1 => "f1",
        NamedKey::F2 => "f2",
        NamedKey::F3 => "f3",
        NamedKey::F4 => "f4",
        NamedKey::F5 => "f5",
        NamedKey::F6 => "f6",
        NamedKey::F7 => "f7",
        NamedKey::F8 => "f8",
        NamedKey::F9 => "f9",
        NamedKey::F10 => "f10",
        NamedKey::F11 => "f11",
        NamedKey::F12 => "f12",
        NamedKey::F13 => "f13",
        NamedKey::F14 => "f14",
        NamedKey::F15 => "f15",
        NamedKey::F16 => "f16",
        NamedKey::F17 => "f17",
        NamedKey::F18 => "f18",
        NamedKey::F19 => "f19",
        NamedKey::F20 => "f20",
        NamedKey::F21 => "f21",
        NamedKey::F22 => "f22",
        NamedKey::F23 => "f23",
        NamedKey::F24 => "f24",
        NamedKey::F25 => "f25",
        NamedKey::F26 => "f26",
        NamedKey::F27 => "f27",
        NamedKey::F28 => "f28",
        NamedKey::F29 => "f29",
        NamedKey::F30 => "f30",
        NamedKey::F31 => "f31",
        NamedKey::F32 => "f32",
        NamedKey::F33 => "f33",
        NamedKey::F34 => "f34",
        NamedKey::F35 => "f35",
        NamedKey::Alt
        | NamedKey::AltGraph
        | NamedKey::CapsLock
        | NamedKey::Control
        | NamedKey::Fn
        | NamedKey::FnLock
        | NamedKey::Meta
        | NamedKey::Shift
        | NamedKey::Symbol
        | NamedKey::SymbolLock
        | NamedKey::NumLock
        | NamedKey::ScrollLock => return None,
        _ => return None,
    })
}

pub(crate) fn mouse_button_from_winit(source: ButtonSource) -> Option<MouseButton> {
    Some(match source.mouse_button()? {
        WinitMouseButton::Left => MouseButton::Left,
        WinitMouseButton::Right => MouseButton::Right,
        WinitMouseButton::Middle => MouseButton::Middle,
        WinitMouseButton::Back => MouseButton::Navigate(NavigationDirection::Back),
        WinitMouseButton::Forward => MouseButton::Navigate(NavigationDirection::Forward),
        _ => return None,
    })
}

pub(crate) fn logical_position(
    position: winit::dpi::PhysicalPosition<f64>,
    scale_factor: f32,
) -> Point<Pixels> {
    point(
        px(position.x as f32 / scale_factor),
        px(position.y as f32 / scale_factor),
    )
}

pub(crate) fn touch_phase_from_winit(phase: WinitTouchPhase) -> TouchPhase {
    match phase {
        WinitTouchPhase::Started => TouchPhase::Started,
        WinitTouchPhase::Moved => TouchPhase::Moved,
        WinitTouchPhase::Ended | WinitTouchPhase::Cancelled => TouchPhase::Ended,
    }
}

pub(crate) fn utf8_cursor_to_utf16(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    text[..index].encode_utf16().count()
}

#[cfg(target_os = "windows")]
pub(crate) fn current_capslock() -> Capslock {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CAPITAL};

    Capslock {
        on: unsafe { GetKeyState(VK_CAPITAL.0 as i32) & 1 } > 0,
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn current_capslock() -> Capslock {
    Capslock::default()
}

#[cfg(target_os = "windows")]
pub(crate) fn current_modifiers() -> Modifiers {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };

    let pressed = |key: i32| unsafe { GetKeyState(key) < 0 };
    Modifiers {
        control: pressed(VK_CONTROL.0 as i32),
        alt: pressed(VK_MENU.0 as i32),
        shift: pressed(VK_SHIFT.0 as i32),
        platform: pressed(VK_LWIN.0 as i32) || pressed(VK_RWIN.0 as i32),
        function: false,
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn current_modifiers() -> Modifiers {
    Modifiers::default()
}
