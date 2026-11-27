#![forbid(unsafe_code)]

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match rune_cli::run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            if !err.message.is_empty() {
                eprintln!("rune: {}", err.message);
            }
            ExitCode::from(err.exit_code.clamp(1, 255) as u8)
        }
    }
}
