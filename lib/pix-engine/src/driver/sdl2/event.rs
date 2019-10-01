use crate::{
    driver::{sdl2::Sdl2Driver, Driver, DriverOpts},
    event::{Key, Mouse, PixEvent},
};
use sdl2::{
    event::{Event, WindowEvent},
    keyboard::Keycode,
    mouse::MouseButton,
};

impl Sdl2Driver {
    pub(super) fn map_key(&self, key: Keycode, pressed: bool) -> PixEvent {
        let key = match key {
            Keycode::A => Key::A,
            Keycode::B => Key::B,
            Keycode::C => Key::C,
            Keycode::D => Key::D,
            Keycode::E => Key::E,
            Keycode::F => Key::F,
            Keycode::G => Key::G,
            Keycode::H => Key::H,
            Keycode::I => Key::I,
            Keycode::J => Key::J,
            Keycode::K => Key::K,
            Keycode::L => Key::L,
            Keycode::N => Key::N,
            Keycode::M => Key::M,
            Keycode::O => Key::O,
            Keycode::P => Key::P,
            Keycode::Q => Key::Q,
            Keycode::R => Key::R,
            Keycode::S => Key::S,
            Keycode::T => Key::T,
            Keycode::U => Key::U,
            Keycode::V => Key::V,
            Keycode::W => Key::W,
            Keycode::X => Key::X,
            Keycode::Y => Key::Y,
            Keycode::Z => Key::Z,
            Keycode::Num0 => Key::Num0,
            Keycode::Num1 => Key::Num1,
            Keycode::Num2 => Key::Num2,
            Keycode::Num3 => Key::Num3,
            Keycode::Num4 => Key::Num4,
            Keycode::Num5 => Key::Num5,
            Keycode::Num6 => Key::Num6,
            Keycode::Num7 => Key::Num7,
            Keycode::Num8 => Key::Num8,
            Keycode::Num9 => Key::Num9,
            Keycode::Kp0 => Key::Kp0,
            Keycode::Kp1 => Key::Kp1,
            Keycode::Kp2 => Key::Kp2,
            Keycode::Kp3 => Key::Kp3,
            Keycode::Kp4 => Key::Kp4,
            Keycode::Kp5 => Key::Kp5,
            Keycode::Kp6 => Key::Kp6,
            Keycode::Kp7 => Key::Kp7,
            Keycode::Kp8 => Key::Kp8,
            Keycode::Kp9 => Key::Kp9,
            Keycode::F1 => Key::F1,
            Keycode::F2 => Key::F2,
            Keycode::F3 => Key::F3,
            Keycode::F4 => Key::F4,
            Keycode::F5 => Key::F5,
            Keycode::F6 => Key::F6,
            Keycode::F7 => Key::F7,
            Keycode::F8 => Key::F8,
            Keycode::F9 => Key::F9,
            Keycode::F10 => Key::F10,
            Keycode::F11 => Key::F11,
            Keycode::F12 => Key::F12,
            Keycode::Left => Key::Left,
            Keycode::Up => Key::Up,
            Keycode::Down => Key::Down,
            Keycode::Right => Key::Right,
            Keycode::Tab => Key::Tab,
            Keycode::Insert => Key::Insert,
            Keycode::Delete => Key::Delete,
            Keycode::Home => Key::Home,
            Keycode::End => Key::End,
            Keycode::PageUp => Key::PageUp,
            Keycode::PageDown => Key::PageDown,
            Keycode::Escape => Key::Escape,
            Keycode::Backspace => Key::Backspace,
            Keycode::Return => Key::Return,
            Keycode::KpEnter => Key::KpEnter,
            Keycode::Pause => Key::Pause,
            Keycode::ScrollLock => Key::ScrollLock,
            Keycode::Plus => Key::Plus,
            Keycode::Minus => Key::Minus,
            Keycode::Period => Key::Period,
            Keycode::Underscore => Key::Underscore,
            Keycode::Equals => Key::Equals,
            Keycode::KpMultiply => Key::KpMultiply,
            Keycode::KpDivide => Key::KpDivide,
            Keycode::KpPlus => Key::KpPlus,
            Keycode::KpMinus => Key::KpMinus,
            Keycode::KpPeriod => Key::KpPeriod,
            Keycode::Backquote => Key::Backquote,
            Keycode::Exclaim => Key::Exclaim,
            Keycode::At => Key::At,
            Keycode::Hash => Key::Hash,
            Keycode::Dollar => Key::Dollar,
            Keycode::Percent => Key::Percent,
            Keycode::Caret => Key::Caret,
            Keycode::Ampersand => Key::Ampersand,
            Keycode::Asterisk => Key::Asterisk,
            Keycode::LeftParen => Key::LeftParen,
            Keycode::RightParen => Key::RightParen,
            Keycode::LeftBracket => Key::LeftBracket,
            Keycode::RightBracket => Key::RightBracket,
            Keycode::Backslash => Key::Backslash,
            Keycode::CapsLock => Key::CapsLock,
            Keycode::Semicolon => Key::Semicolon,
            Keycode::Colon => Key::Colon,
            Keycode::Quotedbl => Key::Quotedbl,
            Keycode::Quote => Key::Quote,
            Keycode::Less => Key::Less,
            Keycode::Comma => Key::Comma,
            Keycode::Greater => Key::Greater,
            Keycode::Question => Key::Question,
            Keycode::Slash => Key::Slash,
            Keycode::LShift | Keycode::RShift => Key::Shift,
            Keycode::Space => Key::Space,
            Keycode::LCtrl | Keycode::RCtrl => Key::Control,
            Keycode::LAlt | Keycode::RAlt => Key::Alt,
            Keycode::LGui | Keycode::RGui => Key::Meta,
            _ => Key::Unknown,
        };
        PixEvent::KeyPress(key, pressed)
    }

    pub(super) fn map_mouse(&self, btn: MouseButton, x: i32, y: i32, pressed: bool) -> PixEvent {
        let btn = match btn {
            MouseButton::Left => Mouse::Left,
            MouseButton::Middle => Mouse::Middle,
            MouseButton::Right => Mouse::Right,
            MouseButton::X1 => Mouse::X1,
            MouseButton::X2 => Mouse::X2,
            _ => Mouse::Unknown,
        };
        PixEvent::MousePress(btn, x, y, pressed)
    }
}
