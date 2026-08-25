//! Layout-independent keyboard input on Windows.
//!
//! crossterm reports the *character* the active keyboard layout produces, so
//! shortcuts break under non-Latin layouts (Persian, Russian, …). This module
//! reads the console input buffer directly and maps **virtual key codes**
//! (physical keys) instead: `VK_M` is the M key no matter which layout is
//! active. Events are converted into ordinary [`KeyEvent`] values so the rest
//! of the app — including the central KEYMAP — works unchanged.

#![cfg(windows)]

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::{channel, Receiver};
use winapi::um::consoleapi::ReadConsoleInputW;
use winapi::um::processenv::GetStdHandle;
use winapi::um::winbase::STD_INPUT_HANDLE;
use winapi::um::consoleapi::GetNumberOfConsoleInputEvents;
use winapi::um::wincon::{
    INPUT_RECORD, KEY_EVENT, LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, RIGHT_ALT_PRESSED,
    RIGHT_CTRL_PRESSED, SHIFT_PRESSED,
};

/// Start the background reader thread. Returns a receiver of synthetic
/// standard key events built from physical virtual-key codes.
pub fn spawn_key_reader() -> Receiver<KeyEvent> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        unsafe {
            let handle = GetStdHandle(STD_INPUT_HANDLE);
            if handle.is_null() || handle == winapi::um::handleapi::INVALID_HANDLE_VALUE {
                return;
            }
            let mut records: [INPUT_RECORD; 16] = std::mem::zeroed();

            loop {
                let mut available: u32 = 0;
                if GetNumberOfConsoleInputEvents(handle, &mut available) == 0 {
                    return; // console gone
                }
                if available == 0 {
                    std::thread::sleep(std::time::Duration::from_millis(8));
                    continue;
                }

                let mut count: u32 = 0;
                if ReadConsoleInputW(handle, records.as_mut_ptr(), 16, &mut count) == 0 {
                    return;
                }
                for record in &records[..count as usize] {
                    if record.EventType != KEY_EVENT {
                        // Resize/mouse/focus are covered by the continuous redraw.
                        continue;
                    }
                    let key = record.Event.KeyEvent();
                    if key.bKeyDown == 0 {
                        continue; // key release
                    }

                    let mut modifiers = KeyModifiers::empty();
                    if key.dwControlKeyState & SHIFT_PRESSED != 0 {
                        modifiers |= KeyModifiers::SHIFT;
                    }
                    if key.dwControlKeyState & (LEFT_CTRL_PRESSED | RIGHT_CTRL_PRESSED) != 0 {
                        modifiers |= KeyModifiers::CONTROL;
                    }
                    if key.dwControlKeyState & (LEFT_ALT_PRESSED | RIGHT_ALT_PRESSED) != 0 {
                        modifiers |= KeyModifiers::ALT;
                    }

                    if let Some(code) = map_vk(key.wVirtualKeyCode, modifiers.contains(KeyModifiers::SHIFT))
                    {
                        if tx.send(KeyEvent::new(code, modifiers)).is_err() {
                            return; // app closed
                        }
                    }
                }
            }
        }
    });
    rx
}

/// Map a Win32 virtual key code to a logical key code. Letters and digits are
/// derived from their VK numbers — physical positions, identical on every
/// keyboard language (Persian, Russian, German, …).
fn map_vk(vk: u16, shift: bool) -> Option<KeyCode> {
    let vk_i32 = vk as i32;
    Some(match vk_i32 {
        winapi::um::winuser::VK_RETURN => KeyCode::Enter,
        winapi::um::winuser::VK_ESCAPE => KeyCode::Esc,
        winapi::um::winuser::VK_TAB => {
            if shift {
                KeyCode::BackTab
            } else {
                KeyCode::Tab
            }
        }
        winapi::um::winuser::VK_BACK => KeyCode::Backspace,
        winapi::um::winuser::VK_INSERT => KeyCode::Insert,
        winapi::um::winuser::VK_DELETE => KeyCode::Delete,
        winapi::um::winuser::VK_UP => KeyCode::Up,
        winapi::um::winuser::VK_DOWN => KeyCode::Down,
        winapi::um::winuser::VK_LEFT => KeyCode::Left,
        winapi::um::winuser::VK_RIGHT => KeyCode::Right,
        winapi::um::winuser::VK_HOME => KeyCode::Home,
        winapi::um::winuser::VK_END => KeyCode::End,
        winapi::um::winuser::VK_PRIOR => KeyCode::PageUp,
        winapi::um::winuser::VK_NEXT => KeyCode::PageDown,

        // Digits row — same physical position on every layout.
        vk @ 0x30..=0x39 => KeyCode::Char((b'0' + (vk as u8 - 0x30)) as char),

        // Letters — VK codes are US positions regardless of active layout.
        vk @ 0x41..=0x5A => KeyCode::Char((b'a' + (vk as u8 - 0x41)) as char),

        // Function keys F1..F24.
        vk @ 0x70..=0x87 => KeyCode::F(vk as u8 - 0x70 + 1),

        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_map_from_physical_positions() {
        assert_eq!(map_vk(0x4D_u16, false), Some(KeyCode::Char('m'))); // VK_M
        assert_eq!(map_vk(0x51_u16, false), Some(KeyCode::Char('q'))); // VK_Q
        assert_eq!(map_vk(0x52_u16, false), Some(KeyCode::Char('r'))); // VK_R
    }

    #[test]
    fn universal_keys_map() {
        assert_eq!(map_vk(0x70_u16, false), Some(KeyCode::F(1))); // VK_F1
        assert_eq!(map_vk(0x7B_u16, false), Some(KeyCode::F(12))); // VK_F12
        assert_eq!(map_vk(0x0D_u16, false), Some(KeyCode::Enter)); // VK_RETURN
        assert_eq!(map_vk(0x2D_u16, false), Some(KeyCode::Insert)); // VK_INSERT
        assert_eq!(map_vk(0x09_u16, true), Some(KeyCode::BackTab)); // SHIFT+TAB
        assert_eq!(map_vk(0x14_u16, false), None); // caps lock: unmapped
    }
}
