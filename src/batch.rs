use crate::commands::KeyStroke;
use crate::input_state::{InputError, InputState, MAX_ASCII_STROKES, ascii_strokes};
use crate::owned_command::OwnedCommand;
use crate::protocol::MAX_WAIT_MS;
use heapless::Vec;

pub const BATCH_MAX_COMMANDS: usize = 32;
pub const BATCH_MAX_PAYLOAD_BYTES: usize = 8 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchError {
    Capacity,
    TooManyKeys,
    UnsupportedAscii(u8),
    KeyboardBusy,
    WaitTooLong,
    NotBatchable,
}

pub struct BatchCollector {
    commands: Vec<OwnedCommand, BATCH_MAX_COMMANDS>,
    payload_bytes: usize,
    shadow: InputState,
}

impl BatchCollector {
    pub fn begin(initial_state: InputState) -> Self {
        Self {
            commands: Vec::new(),
            payload_bytes: 0,
            shadow: initial_state,
        }
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    pub fn payload_bytes(&self) -> usize {
        self.payload_bytes
    }

    pub fn shadow(&self) -> &InputState {
        &self.shadow
    }

    pub fn commands(&self) -> &[OwnedCommand] {
        self.commands.as_slice()
    }

    pub fn push(&mut self, command: OwnedCommand) -> Result<(), BatchError> {
        let mut next_shadow = self.shadow;
        validate_command(&mut next_shadow, &command)?;

        let next_command_count = self
            .commands
            .len()
            .checked_add(1)
            .ok_or(BatchError::Capacity)?;
        if next_command_count > BATCH_MAX_COMMANDS {
            return Err(BatchError::Capacity);
        }

        let next_payload_bytes = checked_payload_total(self.payload_bytes, command.payload_len())?;
        self.commands
            .push(command)
            .map_err(|_| BatchError::Capacity)?;
        self.payload_bytes = next_payload_bytes;
        self.shadow = next_shadow;
        Ok(())
    }
}

fn validate_command(shadow: &mut InputState, command: &OwnedCommand) -> Result<(), BatchError> {
    match command {
        OwnedCommand::Ping
        | OwnedCommand::GetInfo
        | OwnedCommand::GetCaps
        | OwnedCommand::Heartbeat
        | OwnedCommand::StopAll => Err(BatchError::NotBatchable),
        OwnedCommand::KeyDown(stroke) => shadow.keyboard.key_down(*stroke).map_err(map_input_error),
        OwnedCommand::KeyUp(stroke) => {
            shadow.keyboard.key_up(*stroke);
            Ok(())
        }
        OwnedCommand::KeyTap(stroke) => shadow
            .keyboard
            .tap_plan(*stroke)
            .map(|_| ())
            .map_err(map_input_error),
        OwnedCommand::TypeAscii(bytes) => {
            if !shadow.keyboard.is_idle() {
                return Err(BatchError::KeyboardBusy);
            }

            let mut strokes = Vec::<KeyStroke, MAX_ASCII_STROKES>::new();
            ascii_strokes(bytes.as_slice(), false, &mut strokes).map_err(map_input_error)
        }
        OwnedCommand::MouseMoveRel { .. } | OwnedCommand::MouseWheel(_) => Ok(()),
        OwnedCommand::MouseButtonDown(button) => {
            shadow.mouse.button_down(*button);
            Ok(())
        }
        OwnedCommand::MouseButtonUp(button) => {
            shadow.mouse.button_up(*button);
            Ok(())
        }
        OwnedCommand::MouseClick(button) => {
            let _ = shadow.mouse.click_plan(*button);
            Ok(())
        }
        OwnedCommand::WaitMs(wait_ms) if *wait_ms > MAX_WAIT_MS => Err(BatchError::WaitTooLong),
        OwnedCommand::WaitMs(_) => Ok(()),
    }
}

fn map_input_error(error: InputError) -> BatchError {
    match error {
        InputError::TooManyKeys => BatchError::TooManyKeys,
        InputError::TooManyStrokes => BatchError::Capacity,
        InputError::UnsupportedAscii(byte) => BatchError::UnsupportedAscii(byte),
    }
}

fn checked_payload_total(current: usize, additional: usize) -> Result<usize, BatchError> {
    let total = current
        .checked_add(additional)
        .ok_or(BatchError::Capacity)?;
    if total > BATCH_MAX_PAYLOAD_BYTES {
        Err(BatchError::Capacity)
    } else {
        Ok(total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::{KeyStroke, MouseButton};
    use crate::input_state::{InputState, MAX_ASCII_STROKES};
    use crate::owned_command::OwnedCommand;
    use crate::protocol::MAX_WAIT_MS;

    fn stroke(modifier: u8, keycode: u8) -> KeyStroke {
        KeyStroke { modifier, keycode }
    }

    fn assert_batch_state(
        batch: &BatchCollector,
        expected_len: usize,
        expected_payload_bytes: usize,
        expected_shadow: InputState,
    ) {
        assert_eq!(batch.len(), expected_len);
        assert_eq!(batch.payload_bytes(), expected_payload_bytes);
        assert_eq!(*batch.shadow(), expected_shadow);
    }

    #[test]
    fn batch_validates_shadow_state_before_accepting_text() {
        let mut batch = BatchCollector::begin(InputState::new());
        batch
            .push(OwnedCommand::KeyDown(KeyStroke {
                modifier: 0,
                keycode: 0x1A,
            }))
            .unwrap();
        let before = *batch.shadow();
        let before_payload = batch.payload_bytes();
        assert_eq!(
            batch.push(OwnedCommand::type_ascii(b"abc").unwrap()),
            Err(BatchError::KeyboardBusy)
        );
        assert_batch_state(&batch, 1, before_payload, before);
        assert_eq!(
            batch.commands(),
            &[OwnedCommand::KeyDown(KeyStroke {
                modifier: 0,
                keycode: 0x1A,
            })]
        );
    }

    #[test]
    fn batch_enforces_command_and_payload_capacity_without_partial_push() {
        let mut batch = BatchCollector::begin(InputState::new());
        for _ in 0..BATCH_MAX_COMMANDS {
            batch.push(OwnedCommand::WaitMs(1)).unwrap();
        }
        let commands_before = batch.commands.clone();
        let payload_before = batch.payload_bytes();
        let shadow_before = *batch.shadow();

        assert_eq!(
            batch.push(OwnedCommand::WaitMs(1)),
            Err(BatchError::Capacity)
        );
        assert_eq!(
            batch.push(OwnedCommand::KeyDown(stroke(0, 0x1A))),
            Err(BatchError::Capacity)
        );

        assert_eq!(batch.commands(), commands_before.as_slice());
        assert_batch_state(&batch, BATCH_MAX_COMMANDS, payload_before, shadow_before);
        assert_eq!(payload_before, BATCH_MAX_COMMANDS * 4);
        assert!(batch.shadow().keyboard.is_idle());
    }

    #[test]
    fn shadow_tracks_chords_exact_release_and_mouse_transitions() {
        let mut batch = BatchCollector::begin(InputState::new());
        batch.push(OwnedCommand::KeyDown(stroke(0, 0x1A))).unwrap(); // W
        batch.push(OwnedCommand::KeyDown(stroke(0, 0x07))).unwrap(); // D
        batch.push(OwnedCommand::KeyDown(stroke(0x02, 0))).unwrap(); // Shift
        batch.push(OwnedCommand::KeyUp(stroke(0, 0x07))).unwrap();
        batch
            .push(OwnedCommand::MouseButtonDown(MouseButton::Left))
            .unwrap();
        batch
            .push(OwnedCommand::MouseButtonDown(MouseButton::Right))
            .unwrap();
        batch
            .push(OwnedCommand::MouseButtonUp(MouseButton::Left))
            .unwrap();

        assert_eq!(batch.shadow().keyboard.modifiers(), 0x02);
        assert_eq!(batch.shadow().keyboard.keycodes(), &[0x1A]);
        assert_eq!(batch.shadow().mouse.buttons(), MouseButton::Right.mask());
    }

    #[test]
    fn seventh_key_and_seventh_key_tap_fail_transactionally() {
        let mut batch = BatchCollector::begin(InputState::new());
        for keycode in 0x04..=0x09 {
            batch
                .push(OwnedCommand::KeyDown(stroke(0, keycode)))
                .unwrap();
        }
        let before = *batch.shadow();
        let before_payload = batch.payload_bytes();

        assert_eq!(
            batch.push(OwnedCommand::KeyDown(stroke(0, 0x0A))),
            Err(BatchError::TooManyKeys)
        );
        assert_batch_state(&batch, 6, before_payload, before);

        assert_eq!(
            batch.push(OwnedCommand::KeyTap(stroke(0, 0x0A))),
            Err(BatchError::TooManyKeys)
        );
        assert_batch_state(&batch, 6, before_payload, before);
    }

    #[test]
    fn unsupported_text_invalid_wait_and_nonbatchable_commands_are_transactional() {
        let mut batch = BatchCollector::begin(InputState::new());
        batch.push(OwnedCommand::WaitMs(5)).unwrap();
        let before = *batch.shadow();
        let before_payload = batch.payload_bytes();

        assert_eq!(
            batch.push(OwnedCommand::type_ascii(&[0]).unwrap()),
            Err(BatchError::UnsupportedAscii(0))
        );
        assert_batch_state(&batch, 1, before_payload, before);

        assert_eq!(
            batch.push(OwnedCommand::WaitMs(MAX_WAIT_MS + 1)),
            Err(BatchError::WaitTooLong)
        );
        assert_batch_state(&batch, 1, before_payload, before);

        for command in [
            OwnedCommand::Ping,
            OwnedCommand::GetInfo,
            OwnedCommand::GetCaps,
            OwnedCommand::Heartbeat,
            OwnedCommand::StopAll,
        ] {
            assert_eq!(batch.push(command), Err(BatchError::NotBatchable));
            assert_batch_state(&batch, 1, before_payload, before);
        }
    }

    #[test]
    fn taps_and_clicks_validate_without_changing_final_shadow() {
        let mut initial = InputState::new();
        initial.keyboard.key_down(stroke(0x02, 0x04)).unwrap();
        initial.mouse.button_down(MouseButton::Left);
        let mut batch = BatchCollector::begin(initial);

        batch.push(OwnedCommand::KeyTap(stroke(0, 0x05))).unwrap();
        assert_eq!(*batch.shadow(), initial);
        batch
            .push(OwnedCommand::MouseClick(MouseButton::Left))
            .unwrap();
        assert_eq!(*batch.shadow(), initial);
    }

    #[test]
    fn insertion_order_and_payload_accounting_are_preserved() {
        let commands = [
            OwnedCommand::WaitMs(10),
            OwnedCommand::MouseMoveRel { dx: 1, dy: -2 },
            OwnedCommand::MouseWheel(1),
            OwnedCommand::type_ascii(b"abc").unwrap(),
        ];
        let mut batch = BatchCollector::begin(InputState::new());

        for command in commands.clone() {
            batch.push(command).unwrap();
        }

        assert_eq!(batch.commands(), commands.as_slice());
        assert_eq!(batch.payload_bytes(), 4 + 4 + 1 + 3);
        assert_eq!(*batch.shadow(), InputState::new());
        assert!(!batch.is_empty());
    }

    #[test]
    fn thirty_two_maximum_payloads_are_accounted_without_exceeding_byte_limit() {
        let mut batch = BatchCollector::begin(InputState::new());
        let text = OwnedCommand::type_ascii(&[b'a'; MAX_ASCII_STROKES]).unwrap();

        for _ in 0..BATCH_MAX_COMMANDS {
            batch.push(text.clone()).unwrap();
        }

        assert_eq!(batch.len(), BATCH_MAX_COMMANDS);
        assert_eq!(
            batch.payload_bytes(),
            BATCH_MAX_COMMANDS * MAX_ASCII_STROKES
        );
        assert!(batch.payload_bytes() < BATCH_MAX_PAYLOAD_BYTES);
        assert_eq!(batch.push(text), Err(BatchError::Capacity));
    }

    #[test]
    fn payload_limit_arithmetic_accepts_boundary_and_rejects_excess_or_overflow() {
        assert_eq!(
            checked_payload_total(BATCH_MAX_PAYLOAD_BYTES - 1, 1),
            Ok(BATCH_MAX_PAYLOAD_BYTES)
        );
        assert_eq!(
            checked_payload_total(BATCH_MAX_PAYLOAD_BYTES - 1, 2),
            Err(BatchError::Capacity)
        );
        assert_eq!(
            checked_payload_total(usize::MAX, 1),
            Err(BatchError::Capacity)
        );
    }

    #[test]
    fn semantic_validation_precedes_capacity_checks() {
        let mut batch = BatchCollector::begin(InputState::new());
        for _ in 0..BATCH_MAX_COMMANDS {
            batch.push(OwnedCommand::WaitMs(1)).unwrap();
        }

        assert_eq!(
            batch.push(OwnedCommand::WaitMs(MAX_WAIT_MS + 1)),
            Err(BatchError::WaitTooLong)
        );
        assert_eq!(batch.len(), BATCH_MAX_COMMANDS);
    }

    #[test]
    fn begin_honors_nonidle_initial_state() {
        let mut initial = InputState::new();
        initial.keyboard.key_down(stroke(0, 0x1A)).unwrap();
        initial.mouse.button_down(MouseButton::Middle);
        let mut batch = BatchCollector::begin(initial);

        assert!(batch.is_empty());
        assert_eq!(batch.payload_bytes(), 0);
        assert_eq!(*batch.shadow(), initial);
        assert_eq!(
            batch.push(OwnedCommand::type_ascii(b"x").unwrap()),
            Err(BatchError::KeyboardBusy)
        );

        batch.push(OwnedCommand::KeyUp(stroke(0, 0x1A))).unwrap();
        batch
            .push(OwnedCommand::MouseButtonUp(MouseButton::Middle))
            .unwrap();
        assert!(batch.shadow().is_idle());
    }
}
