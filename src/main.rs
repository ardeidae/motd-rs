use std::io::Write;
use std::process::ExitCode;

use motd_rs::cli::{self, Command};

fn main() -> ExitCode {
    let command = match cli::parse(std::env::args_os().skip(1)) {
        Ok(command) => command,
        Err(err) => {
            // Nothing sensible to do if stderr is gone.
            let _ = writeln!(std::io::stderr(), "motd-rs: {err}\nTry 'motd-rs --help'.");
            return ExitCode::from(1);
        }
    };

    let mut output = None;
    let text = match command {
        Command::Help => cli::HELP.to_owned(),
        Command::Version => format!("motd-rs {}\n", env!("CARGO_PKG_VERSION")),
        Command::PrintDefaultConfig => motd_rs::config::DEFAULT_CONFIG.to_owned(),
        Command::CheckConfig(options) => match motd_rs::check_config(&options) {
            Ok(text) => text,
            Err(err) => {
                let _ = writeln!(std::io::stderr(), "motd-rs: {err}");
                return ExitCode::from(1);
            }
        },
        Command::Run(options) => match motd_rs::run(&options) {
            Ok(text) => {
                output = options.output;
                text
            }
            Err(err) => {
                let _ = writeln!(std::io::stderr(), "motd-rs: {err}");
                return ExitCode::from(1);
            }
        },
    };

    match output {
        Some(path) => match motd_rs::output::to_file(&path, text.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                let _ = writeln!(std::io::stderr(), "motd-rs: {}: {err}", path.display());
                ExitCode::from(2)
            }
        },
        // A closed stdout (e.g. `motd-rs | head -0`) is not worth a message.
        None => match motd_rs::output::to_stdout(text.as_bytes()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(_) => ExitCode::from(2),
        },
    }
}
