mod client;
mod keys;
mod script;

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use hid_protocol::protocol::CommandType;

use client::{list_ports, ClientOptions, HidClient};
use keys::parse_combo;
use script::{parse_script, MouseButtonName, ScriptCommand};

#[derive(Parser)]
#[command(author, version, about = "RP2350 USB HID bridge control tool")]
struct Cli {
    #[arg(long)]
    port: Option<String>,
    #[arg(long, default_value_t = 115_200)]
    baud: u32,
    #[arg(long, default_value_t = 1_000)]
    timeout_ms: u64,
    #[arg(long, default_value_t = 2)]
    retries: u8,
    #[arg(long, default_value = "cafe")]
    vid: String,
    #[arg(long, default_value = "2350")]
    pid: String,
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Subcommand)]
enum TopCommand {
    List,
    Ping,
    Info,
    Caps,
    Type { text: String },
    Key(KeyArgs),
    Mouse {
        #[command(subcommand)]
        command: MouseCommand,
    },
    Wait { ms: u32 },
    Stop,
    Run { script: PathBuf },
}

#[derive(Args)]
struct KeyArgs {
    #[arg(value_enum)]
    action: KeyAction,
    combo: String,
}

#[derive(Clone, Copy, ValueEnum)]
enum KeyAction {
    Tap,
    Down,
    Up,
}

#[derive(Subcommand)]
enum MouseCommand {
    Move {
        #[arg(allow_hyphen_values = true)]
        dx: i16,
        #[arg(allow_hyphen_values = true)]
        dy: i16,
    },
    Click { button: String },
    Down { button: String },
    Up { button: String },
    Wheel {
        #[arg(allow_hyphen_values = true)]
        delta: i8,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if matches!(cli.command, TopCommand::List) {
        print_ports()?;
        return Ok(());
    }

    let options = ClientOptions {
        port: cli.port,
        baud: cli.baud,
        timeout: Duration::from_millis(cli.timeout_ms),
        retries: cli.retries,
        vid: parse_hex_u16(&cli.vid)?,
        pid: parse_hex_u16(&cli.pid)?,
    };
    let mut client = HidClient::open(&options)?;

    match cli.command {
        TopCommand::List => unreachable!(),
        TopCommand::Ping => {
            client.send_command(CommandType::Ping, &[])?;
            println!("OK");
        }
        TopCommand::Info => {
            let response = client.send_command(CommandType::GetInfo, &[])?;
            println!("{}", format_payload("INFO", &response.payload));
        }
        TopCommand::Caps => {
            let response = client.send_command(CommandType::GetCaps, &[])?;
            println!("{}", format_payload("CAPS", &response.payload));
        }
        TopCommand::Type { text } => {
            client.send_command(CommandType::TypeAscii, text.as_bytes())?;
            println!("OK");
        }
        TopCommand::Key(args) => {
            let combo = parse_combo(&args.combo)?;
            let command = match args.action {
                KeyAction::Tap => CommandType::KeyTap,
                KeyAction::Down => CommandType::KeyDown,
                KeyAction::Up => CommandType::KeyUp,
            };
            client.send_command(command, &[combo.modifier, combo.keycode])?;
            println!("OK");
        }
        TopCommand::Mouse { command } => {
            execute_mouse_command(&mut client, command)?;
            println!("OK");
        }
        TopCommand::Wait { ms } => {
            client.send_command(CommandType::WaitMs, &ms.to_be_bytes())?;
            println!("OK");
        }
        TopCommand::Stop => {
            client.send_command(CommandType::StopAll, &[])?;
            println!("OK");
        }
        TopCommand::Run { script } => {
            let content = fs::read_to_string(&script)
                .with_context(|| format!("read script {}", script.display()))?;
            let commands = parse_script(&content)?;
            run_script(&mut client, &commands)?;
            println!("OK {} commands", commands.len());
        }
    }

    Ok(())
}

fn execute_mouse_command(client: &mut HidClient, command: MouseCommand) -> Result<()> {
    match command {
        MouseCommand::Move { dx, dy } => {
            client.send_command(CommandType::MouseMoveRel, &mouse_move_payload(dx, dy))?;
        }
        MouseCommand::Click { button } => {
            client.send_command(CommandType::MouseClick, &[parse_button_arg(&button)?.mask()])?;
        }
        MouseCommand::Down { button } => {
            client.send_command(CommandType::MouseButtonDown, &[parse_button_arg(&button)?.mask()])?;
        }
        MouseCommand::Up { button } => {
            client.send_command(CommandType::MouseButtonUp, &[parse_button_arg(&button)?.mask()])?;
        }
        MouseCommand::Wheel { delta } => {
            client.send_command(CommandType::MouseWheel, &[delta as u8])?;
        }
    }
    Ok(())
}

fn run_script(client: &mut HidClient, commands: &[ScriptCommand]) -> Result<()> {
    client.send_command(CommandType::BatchBegin, &[])?;
    let result = (|| {
        for command in commands {
            execute_script_command(client, command)?;
        }
        Ok(())
    })();

    if let Err(err) = result {
        let _ = client.send_command(CommandType::StopAll, &[]);
        return Err(err);
    }

    client.send_command(CommandType::BatchEnd, &[])?;
    Ok(())
}

fn execute_script_command(client: &mut HidClient, command: &ScriptCommand) -> Result<()> {
    match command {
        ScriptCommand::TypeAscii(text) => {
            client.send_command(CommandType::TypeAscii, text.as_bytes())?;
        }
        ScriptCommand::KeyTap(combo) => {
            client.send_command(CommandType::KeyTap, &[combo.modifier, combo.keycode])?;
        }
        ScriptCommand::KeyDown(combo) => {
            client.send_command(CommandType::KeyDown, &[combo.modifier, combo.keycode])?;
        }
        ScriptCommand::KeyUp(combo) => {
            client.send_command(CommandType::KeyUp, &[combo.modifier, combo.keycode])?;
        }
        ScriptCommand::MouseMove { dx, dy } => {
            client.send_command(CommandType::MouseMoveRel, &mouse_move_payload(*dx, *dy))?;
        }
        ScriptCommand::MouseClick(button) => {
            client.send_command(CommandType::MouseClick, &[button.mask()])?;
        }
        ScriptCommand::MouseDown(button) => {
            client.send_command(CommandType::MouseButtonDown, &[button.mask()])?;
        }
        ScriptCommand::MouseUp(button) => {
            client.send_command(CommandType::MouseButtonUp, &[button.mask()])?;
        }
        ScriptCommand::MouseWheel(delta) => {
            client.send_command(CommandType::MouseWheel, &[*delta as u8])?;
        }
        ScriptCommand::WaitMs(ms) => {
            client.send_command(CommandType::WaitMs, &ms.to_be_bytes())?;
        }
        ScriptCommand::StopAll => {
            client.send_command(CommandType::StopAll, &[])?;
        }
    }
    Ok(())
}

fn mouse_move_payload(dx: i16, dy: i16) -> [u8; 4] {
    let mut payload = [0u8; 4];
    payload[0..2].copy_from_slice(&dx.to_be_bytes());
    payload[2..4].copy_from_slice(&dy.to_be_bytes());
    payload
}

fn parse_button_arg(input: &str) -> Result<MouseButtonName> {
    match input.to_ascii_lowercase().as_str() {
        "left" | "l" => Ok(MouseButtonName::Left),
        "right" | "r" => Ok(MouseButtonName::Right),
        "middle" | "m" => Ok(MouseButtonName::Middle),
        _ => bail!("unknown mouse button `{input}`"),
    }
}

fn parse_hex_u16(input: &str) -> Result<u16> {
    let trimmed = input.trim_start_matches("0x");
    Ok(u16::from_str_radix(trimmed, 16)?)
}

fn print_ports() -> Result<()> {
    for port in list_ports()? {
        let vid = port.vid.map(|v| format!("{v:04X}")).unwrap_or_else(|| "----".into());
        let pid = port.pid.map(|p| format!("{p:04X}")).unwrap_or_else(|| "----".into());
        let product = port.product.unwrap_or_default();
        let serial = port.serial_number.unwrap_or_default();
        println!("{}\t{}:{}\t{}\t{}", port.name, vid, pid, product, serial);
    }
    Ok(())
}

fn format_payload(label: &str, payload: &[u8]) -> String {
    let hex = payload.iter().map(|byte| format!("{byte:02X}")).collect::<Vec<_>>().join(" ");
    format!("{label} {hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_negative_mouse_move_without_separator() {
        let cli = Cli::try_parse_from(["hidctl", "mouse", "move", "-20", "0"]).unwrap();

        match cli.command {
            TopCommand::Mouse {
                command: MouseCommand::Move { dx, dy },
            } => {
                assert_eq!(dx, -20);
                assert_eq!(dy, 0);
            }
            _ => panic!("expected mouse move command"),
        }
    }
}
