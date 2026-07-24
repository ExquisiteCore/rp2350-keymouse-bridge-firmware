use crate::keys::{KeyCombo, parse_combo};
use anyhow::{Result, anyhow, bail};
use hid_protocol::protocol::CommandType;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScriptCommand {
    TypeAscii(String),
    KeyTap(KeyCombo),
    KeyDown(KeyCombo),
    KeyUp(KeyCombo),
    MouseMove { dx: i16, dy: i16 },
    MouseClick(MouseButtonName),
    MouseDown(MouseButtonName),
    MouseUp(MouseButtonName),
    MouseWheel(i8),
    WaitMs(u32),
    StopAll,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseButtonName {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScriptPacket {
    pub command_type: CommandType,
    pub payload: Vec<u8>,
}

impl MouseButtonName {
    pub fn mask(self) -> u8 {
        match self {
            Self::Left => 0x01,
            Self::Right => 0x02,
            Self::Middle => 0x04,
        }
    }
}

pub fn plan_script_commands(commands: &[ScriptCommand]) -> Vec<ScriptPacket> {
    let mut plan = Vec::new();
    let mut segment = Vec::new();

    for command in commands {
        if matches!(command, ScriptCommand::StopAll) {
            append_script_segment(&mut plan, &segment);
            segment.clear();
            plan.push(ScriptPacket {
                command_type: CommandType::StopAll,
                payload: Vec::new(),
            });
        } else {
            segment.push(command);
        }
    }
    append_script_segment(&mut plan, &segment);
    plan
}

pub fn run_script_commands<F>(commands: &[ScriptCommand], mut send: F) -> Result<()>
where
    F: FnMut(CommandType, &[u8]) -> Result<()>,
{
    for packet in plan_script_commands(commands) {
        if let Err(error) = send(packet.command_type, &packet.payload) {
            let _ = send(CommandType::StopAll, &[]);
            return Err(error);
        }
    }
    Ok(())
}

fn append_script_segment(plan: &mut Vec<ScriptPacket>, segment: &[&ScriptCommand]) {
    if segment.is_empty() {
        return;
    }

    plan.push(ScriptPacket {
        command_type: CommandType::BatchBegin,
        payload: Vec::new(),
    });
    plan.extend(segment.iter().map(|command| script_command_packet(command)));
    plan.push(ScriptPacket {
        command_type: CommandType::BatchEnd,
        payload: Vec::new(),
    });
}

fn script_command_packet(command: &ScriptCommand) -> ScriptPacket {
    let (command_type, payload) = match command {
        ScriptCommand::TypeAscii(text) => (CommandType::TypeAscii, text.as_bytes().to_vec()),
        ScriptCommand::KeyTap(combo) => (CommandType::KeyTap, vec![combo.modifier, combo.keycode]),
        ScriptCommand::KeyDown(combo) => {
            (CommandType::KeyDown, vec![combo.modifier, combo.keycode])
        }
        ScriptCommand::KeyUp(combo) => (CommandType::KeyUp, vec![combo.modifier, combo.keycode]),
        ScriptCommand::MouseMove { dx, dy } => {
            let mut payload = Vec::with_capacity(4);
            payload.extend_from_slice(&dx.to_be_bytes());
            payload.extend_from_slice(&dy.to_be_bytes());
            (CommandType::MouseMoveRel, payload)
        }
        ScriptCommand::MouseClick(button) => (CommandType::MouseClick, vec![button.mask()]),
        ScriptCommand::MouseDown(button) => (CommandType::MouseButtonDown, vec![button.mask()]),
        ScriptCommand::MouseUp(button) => (CommandType::MouseButtonUp, vec![button.mask()]),
        ScriptCommand::MouseWheel(delta) => (CommandType::MouseWheel, vec![*delta as u8]),
        ScriptCommand::WaitMs(milliseconds) => {
            (CommandType::WaitMs, milliseconds.to_be_bytes().to_vec())
        }
        ScriptCommand::StopAll => (CommandType::StopAll, Vec::new()),
    };
    ScriptPacket {
        command_type,
        payload,
    }
}

pub fn parse_script(input: &str) -> Result<Vec<ScriptCommand>> {
    let mut commands = Vec::new();
    for (index, line) in input.lines().enumerate() {
        if let Some(command) =
            parse_line(line).map_err(|err| anyhow!("line {}: {err}", index + 1))?
        {
            commands.push(command);
        }
    }
    Ok(commands)
}

pub fn parse_line(line: &str) -> Result<Option<ScriptCommand>> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }

    let mut parts = split_words(line)?;
    if parts.is_empty() {
        return Ok(None);
    }

    let head = parts.remove(0).to_ascii_lowercase();
    let command = match head.as_str() {
        "type" | "text" => {
            if parts.len() != 1 {
                bail!("type expects one quoted or unquoted string");
            }
            ScriptCommand::TypeAscii(parts.remove(0))
        }
        "key" => parse_key_command(&parts)?,
        "mouse" => parse_mouse_command(&parts)?,
        "wait" => {
            if parts.len() != 1 {
                bail!("wait expects milliseconds");
            }
            ScriptCommand::WaitMs(parts[0].parse()?)
        }
        "stop" => {
            if !parts.is_empty() {
                bail!("stop takes no arguments");
            }
            ScriptCommand::StopAll
        }
        other => bail!("unknown script command `{other}`"),
    };

    Ok(Some(command))
}

fn parse_key_command(parts: &[String]) -> Result<ScriptCommand> {
    if parts.len() != 2 {
        bail!("key expects: key tap|down|up COMBO");
    }

    let combo = parse_combo(&parts[1])?;
    match parts[0].to_ascii_lowercase().as_str() {
        "tap" => Ok(ScriptCommand::KeyTap(combo)),
        "down" => Ok(ScriptCommand::KeyDown(combo)),
        "up" => Ok(ScriptCommand::KeyUp(combo)),
        other => bail!("unknown key action `{other}`"),
    }
}

fn parse_mouse_command(parts: &[String]) -> Result<ScriptCommand> {
    if parts.is_empty() {
        bail!("mouse expects an action");
    }

    match parts[0].to_ascii_lowercase().as_str() {
        "move" => {
            if parts.len() != 3 {
                bail!("mouse move expects dx dy");
            }
            Ok(ScriptCommand::MouseMove {
                dx: parts[1].parse()?,
                dy: parts[2].parse()?,
            })
        }
        "click" => {
            if parts.len() != 2 {
                bail!("mouse click expects button");
            }
            Ok(ScriptCommand::MouseClick(parse_button(&parts[1])?))
        }
        "down" => {
            if parts.len() != 2 {
                bail!("mouse down expects button");
            }
            Ok(ScriptCommand::MouseDown(parse_button(&parts[1])?))
        }
        "up" => {
            if parts.len() != 2 {
                bail!("mouse up expects button");
            }
            Ok(ScriptCommand::MouseUp(parse_button(&parts[1])?))
        }
        "wheel" => {
            if parts.len() != 2 {
                bail!("mouse wheel expects delta");
            }
            Ok(ScriptCommand::MouseWheel(parts[1].parse()?))
        }
        other => bail!("unknown mouse action `{other}`"),
    }
}

fn parse_button(input: &str) -> Result<MouseButtonName> {
    match input.to_ascii_lowercase().as_str() {
        "left" | "l" => Ok(MouseButtonName::Left),
        "right" | "r" => Ok(MouseButtonName::Right),
        "middle" | "m" => Ok(MouseButtonName::Middle),
        other => bail!("unknown mouse button `{other}`"),
    }
}

fn split_words(line: &str) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = line.chars().peekable();
    let mut in_quote = false;

    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                if in_quote {
                    out.push(core::mem::take(&mut current));
                    in_quote = false;
                    while matches!(chars.peek(), Some(' ' | '\t')) {
                        chars.next();
                    }
                } else {
                    if !current.is_empty() {
                        bail!("quote must start a new token");
                    }
                    in_quote = true;
                }
            }
            '\\' if in_quote => {
                let Some(next) = chars.next() else {
                    bail!("trailing escape in quoted string");
                };
                let escaped = match next {
                    'n' => '\n',
                    'r' => '\r',
                    't' => '\t',
                    '"' => '"',
                    '\\' => '\\',
                    other => other,
                };
                current.push(escaped);
            }
            ' ' | '\t' if !in_quote => {
                if !current.is_empty() {
                    out.push(core::mem::take(&mut current));
                }
            }
            '#' if !in_quote && current.is_empty() => break,
            _ => current.push(ch),
        }
    }

    if in_quote {
        bail!("unterminated quoted string");
    }
    if !current.is_empty() {
        out.push(current);
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hid_protocol::protocol::CommandType;

    #[test]
    fn parses_script_lines() {
        assert!(matches!(
            parse_line(r#"type "abc""#).unwrap(),
            Some(ScriptCommand::TypeAscii(text)) if text == "abc"
        ));
        assert!(matches!(
            parse_line("key tap ENTER").unwrap(),
            Some(ScriptCommand::KeyTap(combo)) if combo.keycode == 0x28
        ));
        assert_eq!(
            parse_line("mouse move 10 -5").unwrap(),
            Some(ScriptCommand::MouseMove { dx: 10, dy: -5 })
        );
        assert_eq!(
            parse_line("wait 100").unwrap(),
            Some(ScriptCommand::WaitMs(100))
        );
        assert_eq!(parse_line("stop").unwrap(), Some(ScriptCommand::StopAll));
    }

    #[test]
    fn ignores_blank_and_comment_lines() {
        assert_eq!(parse_line("").unwrap(), None);
        assert_eq!(parse_line("  # comment").unwrap(), None);
    }

    #[test]
    fn plans_stop_as_a_boundary_between_non_empty_batches() {
        let plan = plan_script_commands(&[
            ScriptCommand::WaitMs(10),
            ScriptCommand::StopAll,
            ScriptCommand::WaitMs(20),
        ]);

        assert_eq!(
            plan.iter()
                .map(|packet| packet.command_type)
                .collect::<Vec<_>>(),
            vec![
                CommandType::BatchBegin,
                CommandType::WaitMs,
                CommandType::BatchEnd,
                CommandType::StopAll,
                CommandType::BatchBegin,
                CommandType::WaitMs,
                CommandType::BatchEnd,
            ]
        );
        assert_eq!(plan[1].payload, 10u32.to_be_bytes());
        assert_eq!(plan[5].payload, 20u32.to_be_bytes());
    }

    #[test]
    fn planner_does_not_emit_empty_batches_around_stops() {
        let plan = plan_script_commands(&[
            ScriptCommand::StopAll,
            ScriptCommand::StopAll,
            ScriptCommand::WaitMs(10),
            ScriptCommand::StopAll,
        ]);

        assert_eq!(
            plan.iter()
                .map(|packet| packet.command_type)
                .collect::<Vec<_>>(),
            vec![
                CommandType::StopAll,
                CommandType::StopAll,
                CommandType::BatchBegin,
                CommandType::WaitMs,
                CommandType::BatchEnd,
                CommandType::StopAll,
            ]
        );
        assert!(plan_script_commands(&[]).is_empty());
    }

    #[test]
    fn runner_stops_without_continuing_after_command_error() {
        let commands = [ScriptCommand::WaitMs(10), ScriptCommand::WaitMs(20)];
        let mut sent = Vec::new();

        let error = run_script_commands(&commands, |command_type, _| {
            sent.push(command_type);
            if sent.len() == 2 {
                bail!("original command error");
            }
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.to_string(), "original command error");
        assert_eq!(
            sent,
            vec![
                CommandType::BatchBegin,
                CommandType::WaitMs,
                CommandType::StopAll,
            ]
        );
    }

    #[test]
    fn runner_stops_and_preserves_batch_end_error() {
        let commands = [ScriptCommand::WaitMs(10)];
        let mut sent = Vec::new();

        let error = run_script_commands(&commands, |command_type, _| {
            sent.push(command_type);
            if command_type == CommandType::BatchEnd {
                bail!("original batch error");
            }
            Ok(())
        })
        .unwrap_err();

        assert_eq!(error.to_string(), "original batch error");
        assert_eq!(
            sent,
            vec![
                CommandType::BatchBegin,
                CommandType::WaitMs,
                CommandType::BatchEnd,
                CommandType::StopAll,
            ]
        );
    }
}
