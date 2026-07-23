use crate::commands::{KeyStroke, MouseButton, ascii_to_keystroke_with_caps_lock};
use heapless::Vec;

const MAX_HELD_KEYS: usize = 6;
const MAX_ASCII_STROKES: usize = 240;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputError {
    TooManyKeys,
    TooManyStrokes,
    UnsupportedAscii(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeyboardState {
    modifiers: u8,
    keycodes: [u8; MAX_HELD_KEYS],
    len: u8,
}

impl KeyboardState {
    pub const fn new() -> Self {
        Self {
            modifiers: 0,
            keycodes: [0; MAX_HELD_KEYS],
            len: 0,
        }
    }

    pub fn modifiers(&self) -> u8 {
        self.modifiers
    }

    pub fn keycodes(&self) -> &[u8] {
        &self.keycodes[..usize::from(self.len)]
    }

    pub fn is_idle(&self) -> bool {
        self.modifiers == 0 && self.len == 0
    }

    pub fn key_down(&mut self, stroke: KeyStroke) -> Result<(), InputError> {
        let key_position = self.key_position(stroke.keycode);
        if stroke.keycode != 0 && key_position.is_none() && usize::from(self.len) == MAX_HELD_KEYS {
            return Err(InputError::TooManyKeys);
        }

        self.modifiers |= stroke.modifier;
        if stroke.keycode != 0 && key_position.is_none() {
            self.keycodes[usize::from(self.len)] = stroke.keycode;
            self.len += 1;
        }

        Ok(())
    }

    pub fn key_up(&mut self, stroke: KeyStroke) {
        self.modifiers &= !stroke.modifier;

        let Some(position) = self.key_position(stroke.keycode) else {
            return;
        };

        let len = usize::from(self.len);
        self.keycodes.copy_within(position + 1..len, position);
        self.len -= 1;
        self.keycodes[usize::from(self.len)] = 0;
    }

    pub fn clear(&mut self) {
        *self = Self::new();
    }

    pub fn tap_plan(&self, stroke: KeyStroke) -> Result<KeyboardPulse, InputError> {
        let requested_part_is_held = if stroke.keycode == 0 {
            self.modifiers & stroke.modifier != 0
        } else {
            self.key_position(stroke.keycode).is_some()
        };

        if requested_part_is_held {
            let mut released = *self;
            released.key_up(KeyStroke {
                modifier: if stroke.keycode == 0 {
                    stroke.modifier
                } else {
                    0
                },
                keycode: stroke.keycode,
            });
            let mut pressed = released;
            pressed.key_down(stroke)?;
            Ok(KeyboardPulse::ReleasePressRestore {
                released,
                pressed,
                restore: *self,
            })
        } else {
            let mut pressed = *self;
            pressed.key_down(stroke)?;
            Ok(KeyboardPulse::PressRestore {
                pressed,
                restore: *self,
            })
        }
    }

    fn key_position(&self, keycode: u8) -> Option<usize> {
        if keycode == 0 {
            return None;
        }
        self.keycodes().iter().position(|held| *held == keycode)
    }
}

impl Default for KeyboardState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyboardPulse {
    PressRestore {
        pressed: KeyboardState,
        restore: KeyboardState,
    },
    ReleasePressRestore {
        released: KeyboardState,
        pressed: KeyboardState,
        restore: KeyboardState,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MouseState {
    buttons: u8,
}

impl MouseState {
    pub const fn new() -> Self {
        Self { buttons: 0 }
    }

    pub const fn from_buttons(buttons: u8) -> Self {
        Self { buttons }
    }

    pub const fn buttons(&self) -> u8 {
        self.buttons
    }

    pub const fn is_idle(&self) -> bool {
        self.buttons == 0
    }

    pub fn button_down(&mut self, button: MouseButton) {
        self.buttons |= button.mask();
    }

    pub fn button_up(&mut self, button: MouseButton) {
        self.buttons &= !button.mask();
    }

    pub fn clear(&mut self) {
        self.buttons = 0;
    }

    pub fn click_plan(&self, button: MouseButton) -> MousePulse {
        let mask = button.mask();
        if self.buttons & mask == 0 {
            MousePulse::PressRestore {
                pressed: self.buttons | mask,
                restore: self.buttons,
            }
        } else {
            MousePulse::ReleasePressRestore {
                released: self.buttons & !mask,
                pressed: self.buttons,
                restore: self.buttons,
            }
        }
    }
}

impl Default for MouseState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MousePulse {
    PressRestore {
        pressed: u8,
        restore: u8,
    },
    ReleasePressRestore {
        released: u8,
        pressed: u8,
        restore: u8,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputState {
    pub keyboard: KeyboardState,
    pub mouse: MouseState,
}

impl InputState {
    pub const fn new() -> Self {
        Self {
            keyboard: KeyboardState::new(),
            mouse: MouseState::new(),
        }
    }

    pub fn is_idle(&self) -> bool {
        self.keyboard.is_idle() && self.mouse.is_idle()
    }

    pub fn clear(&mut self) {
        self.keyboard.clear();
        self.mouse.clear();
    }
}

impl Default for InputState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn ascii_strokes(
    bytes: &[u8],
    caps_lock: bool,
    out: &mut Vec<KeyStroke, MAX_ASCII_STROKES>,
) -> Result<(), InputError> {
    if bytes.len() > MAX_ASCII_STROKES {
        return Err(InputError::TooManyStrokes);
    }

    let mut validated = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        let stroke = ascii_to_keystroke_with_caps_lock(byte, caps_lock)
            .ok_or(InputError::UnsupportedAscii(byte))?;
        validated
            .push(stroke)
            .map_err(|_| InputError::TooManyStrokes)?;

        index += if byte == b'\r' && bytes.get(index + 1) == Some(&b'\n') {
            2
        } else {
            1
        };
    }

    *out = validated;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{KeyStroke, MOD_LEFT_SHIFT, MouseButton};
    use heapless::Vec;

    fn stroke(modifier: u8, keycode: u8) -> KeyStroke {
        KeyStroke { modifier, keycode }
    }

    fn six_key_keyboard() -> KeyboardState {
        let mut keyboard = KeyboardState::new();
        for keycode in 0x04..=0x09 {
            keyboard.key_down(stroke(0, keycode)).unwrap();
        }
        keyboard
    }

    #[test]
    fn holds_multiple_keys_and_releases_exactly_one() {
        let mut state = InputState::new();
        state.keyboard.key_down(stroke(0, 0x1A)).unwrap(); // W
        state.keyboard.key_down(stroke(0, 0x07)).unwrap(); // D
        state.keyboard.key_down(stroke(0x02, 0)).unwrap(); // Shift

        state.keyboard.key_up(stroke(0, 0x07));

        assert_eq!(state.keyboard.modifiers(), 0x02);
        assert_eq!(state.keyboard.keycodes(), &[0x1A]);
    }

    #[test]
    fn seventh_distinct_key_is_rejected_without_mutation() {
        let mut keyboard = six_key_keyboard();
        let before = keyboard;

        assert_eq!(
            keyboard.key_down(stroke(0, 0x0A)),
            Err(InputError::TooManyKeys)
        );
        assert_eq!(keyboard, before);
    }

    #[test]
    fn click_plan_restores_an_already_held_button() {
        let state = MouseState::from_buttons(0x01);

        assert_eq!(
            state.click_plan(MouseButton::Left),
            MousePulse::ReleasePressRestore {
                released: 0,
                pressed: 1,
                restore: 1,
            }
        );
    }

    #[test]
    fn duplicate_keyboard_down_and_up_are_idempotent() {
        let mut keyboard = KeyboardState::new();
        let requested = stroke(0x03, 0x1A);

        keyboard.key_down(requested).unwrap();
        let held = keyboard;
        keyboard.key_down(requested).unwrap();
        assert_eq!(keyboard, held);

        keyboard.key_up(requested);
        assert!(keyboard.is_idle());
        let released = keyboard;
        keyboard.key_up(requested);
        assert_eq!(keyboard, released);
    }

    #[test]
    fn modifier_only_down_and_up_preserve_ordinary_keys() {
        let mut keyboard = KeyboardState::new();
        keyboard.key_down(stroke(0, 0x1A)).unwrap();

        keyboard.key_down(stroke(0x82, 0)).unwrap();
        assert_eq!(keyboard.modifiers(), 0x82);
        assert_eq!(keyboard.keycodes(), &[0x1A]);

        keyboard.key_up(stroke(0x02, 0));
        assert_eq!(keyboard.modifiers(), 0x80);
        assert_eq!(keyboard.keycodes(), &[0x1A]);
    }

    #[test]
    fn rejected_seventh_key_does_not_add_requested_modifiers() {
        let mut keyboard = six_key_keyboard();
        keyboard.key_down(stroke(0x01, 0)).unwrap();
        let before = keyboard;

        assert_eq!(
            keyboard.key_down(stroke(0x82, 0x0A)),
            Err(InputError::TooManyKeys)
        );
        assert_eq!(keyboard, before);
    }

    #[test]
    fn tapping_new_key_with_held_modifier_does_not_release_modifier() {
        let mut keyboard = KeyboardState::new();
        keyboard.key_down(stroke(0x02, 0x1A)).unwrap();
        let original = keyboard;
        let requested = stroke(0x02, 0x07);
        let mut pressed = original;
        pressed.key_down(requested).unwrap();

        assert_eq!(
            keyboard.tap_plan(requested),
            Ok(KeyboardPulse::PressRestore {
                pressed,
                restore: original,
            })
        );
        assert_eq!(keyboard, original);
    }

    #[test]
    fn tapping_held_key_preserves_held_modifier_during_release() {
        let mut keyboard = KeyboardState::new();
        let requested = stroke(0x02, 0x1A);
        keyboard.key_down(requested).unwrap();
        let original = keyboard;

        let mut released = original;
        released.key_up(stroke(0, requested.keycode));
        let mut pressed = released;
        pressed.key_down(requested).unwrap();

        assert_eq!(
            keyboard.tap_plan(requested),
            Ok(KeyboardPulse::ReleasePressRestore {
                released,
                pressed,
                restore: original,
            })
        );
        assert_eq!(keyboard, original);
    }

    #[test]
    fn tapping_held_modifier_only_stroke_releases_and_restores_it() {
        let mut keyboard = KeyboardState::new();
        keyboard.key_down(stroke(0x02, 0x1A)).unwrap();
        let original = keyboard;
        let requested = stroke(0x02, 0);

        let mut released = original;
        released.key_up(requested);
        let mut pressed = released;
        pressed.key_down(requested).unwrap();

        assert_eq!(
            keyboard.tap_plan(requested),
            Ok(KeyboardPulse::ReleasePressRestore {
                released,
                pressed,
                restore: original,
            })
        );
        assert_eq!(keyboard, original);
    }

    #[test]
    fn tapping_unheld_modifier_only_stroke_presses_then_restores_it() {
        let mut keyboard = KeyboardState::new();
        keyboard.key_down(stroke(0, 0x1A)).unwrap();
        let original = keyboard;
        let requested = stroke(0x02, 0);
        let mut pressed = original;
        pressed.key_down(requested).unwrap();

        assert_eq!(
            keyboard.tap_plan(requested),
            Ok(KeyboardPulse::PressRestore {
                pressed,
                restore: original,
            })
        );
        assert_eq!(keyboard, original);
    }

    #[test]
    fn tapping_a_new_key_contains_complete_pressed_and_restore_reports() {
        let mut keyboard = KeyboardState::new();
        keyboard.key_down(stroke(0x01, 0x1A)).unwrap();
        let original = keyboard;
        let requested = stroke(0x02, 0x07);
        let mut pressed = original;
        pressed.key_down(requested).unwrap();

        assert_eq!(
            keyboard.tap_plan(requested),
            Ok(KeyboardPulse::PressRestore {
                pressed,
                restore: original,
            })
        );
        assert_eq!(keyboard, original);
    }

    #[test]
    fn tap_at_capacity_accepts_held_keys_and_modifier_only_strokes() {
        let keyboard = six_key_keyboard();

        assert!(keyboard.tap_plan(stroke(0, 0x04)).is_ok());
        assert!(keyboard.tap_plan(stroke(0x02, 0)).is_ok());
        assert_eq!(
            keyboard.tap_plan(stroke(0x02, 0x0A)),
            Err(InputError::TooManyKeys)
        );
    }

    #[test]
    fn keycodes_retain_stable_insertion_order_after_removal() {
        let mut keyboard = KeyboardState::new();
        for keycode in [0x1A, 0x07, 0x04, 0x16] {
            keyboard.key_down(stroke(0, keycode)).unwrap();
        }

        keyboard.key_up(stroke(0, 0x07));

        assert_eq!(keyboard.keycodes(), &[0x1A, 0x04, 0x16]);
    }

    #[test]
    fn keyboard_clear_returns_to_idle() {
        let mut keyboard = KeyboardState::new();
        assert!(keyboard.is_idle());
        keyboard.key_down(stroke(0xFF, 0x04)).unwrap();
        assert!(!keyboard.is_idle());

        keyboard.clear();

        assert!(keyboard.is_idle());
        assert_eq!(keyboard.modifiers(), 0);
        assert!(keyboard.keycodes().is_empty());
    }

    #[test]
    fn mouse_transitions_are_idempotent_and_preserve_unrelated_buttons() {
        let mut mouse = MouseState::from_buttons(0x02);

        mouse.button_down(MouseButton::Left);
        mouse.button_down(MouseButton::Left);
        assert_eq!(mouse.buttons(), 0x03);

        mouse.button_up(MouseButton::Left);
        mouse.button_up(MouseButton::Left);
        assert_eq!(mouse.buttons(), 0x02);

        mouse.clear();
        assert_eq!(mouse.buttons(), 0);
        assert!(mouse.is_idle());
    }

    #[test]
    fn unheld_mouse_click_preserves_unrelated_buttons() {
        let mouse = MouseState::from_buttons(0x03);

        assert_eq!(
            mouse.click_plan(MouseButton::Middle),
            MousePulse::PressRestore {
                pressed: 0x07,
                restore: 0x03,
            }
        );
        assert_eq!(mouse.buttons(), 0x03);
    }

    #[test]
    fn held_mouse_click_preserves_unrelated_buttons() {
        let mouse = MouseState::from_buttons(0x03);

        assert_eq!(
            mouse.click_plan(MouseButton::Left),
            MousePulse::ReleasePressRestore {
                released: 0x02,
                pressed: 0x03,
                restore: 0x03,
            }
        );
        assert_eq!(mouse.buttons(), 0x03);
    }

    #[test]
    fn ascii_validation_is_transactional_for_unsupported_bytes() {
        let mut out = Vec::<KeyStroke, 240>::new();
        out.push(stroke(0, 0x2C)).unwrap();
        let before = out.clone();

        assert_eq!(
            ascii_strokes(b"ok\x7f", false, &mut out),
            Err(InputError::UnsupportedAscii(0x7F))
        );
        assert_eq!(out, before);
    }

    #[test]
    fn ascii_validation_replaces_output_only_after_success() {
        let mut out = Vec::<KeyStroke, 240>::new();
        out.push(stroke(0, 0x2C)).unwrap();

        ascii_strokes(b"a", false, &mut out).unwrap();

        assert_eq!(out.as_slice(), &[stroke(0, 0x04)]);
    }

    #[test]
    fn ascii_normalizes_crlf_but_keeps_standalone_cr_and_lf() {
        let mut out = Vec::<KeyStroke, 240>::new();

        ascii_strokes(b"\r\n\r\n\n\r", false, &mut out).unwrap();

        assert_eq!(out.as_slice(), &[stroke(0, 0x28); 4]);
    }

    #[test]
    fn caps_lock_inverts_shift_for_letters_but_not_symbols() {
        let mut out = Vec::<KeyStroke, 240>::new();

        ascii_strokes(b"aA!_", true, &mut out).unwrap();

        assert_eq!(
            out.as_slice(),
            &[
                stroke(MOD_LEFT_SHIFT, 0x04),
                stroke(0, 0x04),
                stroke(MOD_LEFT_SHIFT, 0x1E),
                stroke(MOD_LEFT_SHIFT, 0x2D),
            ]
        );
    }

    #[test]
    fn ascii_capacity_boundary_is_transactional() {
        let mut out = Vec::<KeyStroke, 240>::new();
        ascii_strokes(&[b'a'; 240], false, &mut out).unwrap();
        assert_eq!(out.len(), 240);

        let before = out.clone();
        assert_eq!(
            ascii_strokes(&[b'a'; 241], false, &mut out),
            Err(InputError::TooManyStrokes)
        );
        assert_eq!(out, before);
    }

    #[test]
    fn input_state_new_and_default_are_idle_and_clear_releases_everything() {
        let mut state = InputState::new();
        assert_eq!(state, InputState::default());
        assert!(state.is_idle());

        state.keyboard.key_down(stroke(0x02, 0x04)).unwrap();
        state.mouse.button_down(MouseButton::Right);
        state.clear();

        assert!(state.is_idle());
        assert!(state.keyboard.is_idle());
        assert!(state.mouse.is_idle());
    }
}
