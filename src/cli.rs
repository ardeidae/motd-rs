//! Hand-written command-line parsing: no dependency, `clap` would add about 300 KB.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;

use crate::sysroot::Sysroot;

/// Text printed by `--help`.
pub const HELP: &str = "\
motd-rs - compact system status for the Linux message of the day

Usage: motd-rs [OPTIONS]

Options:
      --config <FILE>           Read this config file instead of searching for one
      --check-config            Check the config file, print the error if any, exit 1 on error
      --print-default-config    Print the default config file, with comments
  -h, --help                    Print this help
  -V, --version                 Print the version

Config search (first found wins): --config, $MOTD_RS_CONFIG,
$XDG_CONFIG_HOME/motd-rs/config.toml (not for root), /etc/motd-rs/config.toml.

This is an early development version: only the host line is shown.
";

/// What the program was asked to do.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Collect and print the report.
    Run(Options),
    /// `--check-config`: validate the config file the run would use.
    CheckConfig(Options),
    /// `--print-default-config`.
    PrintDefaultConfig,
    /// `--help`.
    Help,
    /// `--version`.
    Version,
}

/// Options for a normal run.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct Options {
    /// Root used for every file read (hidden `--sysroot`, for tests and fixtures).
    pub sysroot: Sysroot,
    /// `--config <file>`: the only config file considered.
    pub config: Option<PathBuf>,
}

/// Command-line errors.
#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    UnknownArgument(String),
    MissingValue(&'static str),
    NotUnicode(OsString),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownArgument(arg) => write!(f, "unknown argument '{arg}'"),
            Self::MissingValue(flag) => write!(f, "{flag} needs a value"),
            Self::NotUnicode(arg) => write!(f, "argument is not valid UTF-8: {}", arg.display()),
        }
    }
}

impl std::error::Error for CliError {}

/// Parses the arguments (without the program name).
///
/// `--help` and `--version` win over everything else, like most tools, then
/// `--print-default-config`, then `--check-config`.
///
/// # Errors
///
/// Returns a [`CliError`] for unknown flags, missing values and non-UTF-8 arguments.
pub fn parse<I>(args: I) -> Result<Command, CliError>
where
    I: IntoIterator<Item = OsString>,
{
    let mut options = Options::default();
    let mut help = false;
    let mut version = false;
    let mut print_default = false;
    let mut check = false;
    let mut args = args.into_iter();

    while let Some(raw) = args.next() {
        let arg = raw.into_string().map_err(CliError::NotUnicode)?;
        let (flag, inline_value) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value)),
            _ => (arg.as_str(), None),
        };
        match (flag, inline_value) {
            ("-h" | "--help", None) => help = true,
            ("-V" | "--version", None) => version = true,
            ("--print-default-config", None) => print_default = true,
            ("--check-config", None) => check = true,
            ("--sysroot", inline_value) => {
                options.sysroot = Sysroot::new(value("--sysroot", inline_value, &mut args)?);
            }
            ("--config", inline_value) => {
                options.config = Some(PathBuf::from(value("--config", inline_value, &mut args)?));
            }
            _ => return Err(CliError::UnknownArgument(arg)),
        }
    }

    Ok(if help {
        Command::Help
    } else if version {
        Command::Version
    } else if print_default {
        Command::PrintDefaultConfig
    } else if check {
        Command::CheckConfig(options)
    } else {
        Command::Run(options)
    })
}

/// The value of `flag`: after `=` or as the next argument. Never empty.
fn value(
    flag: &'static str,
    inline_value: Option<&str>,
    args: &mut impl Iterator<Item = OsString>,
) -> Result<String, CliError> {
    let value = match inline_value {
        Some(value) => value.to_owned(),
        None => args
            .next()
            .ok_or(CliError::MissingValue(flag))?
            .into_string()
            .map_err(CliError::NotUnicode)?,
    };
    if value.is_empty() {
        return Err(CliError::MissingValue(flag));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Command, CliError> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_runs_with_defaults() {
        assert_eq!(parse_strs(&[]), Ok(Command::Run(Options::default())));
    }

    #[test]
    fn help_and_version() {
        assert_eq!(parse_strs(&["-h"]), Ok(Command::Help));
        assert_eq!(parse_strs(&["--help"]), Ok(Command::Help));
        assert_eq!(parse_strs(&["-V"]), Ok(Command::Version));
        assert_eq!(parse_strs(&["--version"]), Ok(Command::Version));
        assert_eq!(parse_strs(&["--version", "--help"]), Ok(Command::Help));
    }

    #[test]
    fn sysroot_separate_and_inline() {
        let expected = Ok(Command::Run(Options {
            sysroot: Sysroot::new("/tmp/x"),
            ..Options::default()
        }));
        assert_eq!(parse_strs(&["--sysroot", "/tmp/x"]), expected);
        assert_eq!(parse_strs(&["--sysroot=/tmp/x"]), expected);
    }

    #[test]
    fn sysroot_needs_a_value() {
        assert_eq!(
            parse_strs(&["--sysroot"]),
            Err(CliError::MissingValue("--sysroot"))
        );
        assert_eq!(
            parse_strs(&["--sysroot="]),
            Err(CliError::MissingValue("--sysroot"))
        );
    }

    #[test]
    fn config_options() {
        let with_config = Options {
            config: Some(PathBuf::from("/etc/x.toml")),
            ..Options::default()
        };
        assert_eq!(
            parse_strs(&["--config", "/etc/x.toml"]),
            Ok(Command::Run(Options {
                config: Some(PathBuf::from("/etc/x.toml")),
                ..Options::default()
            }))
        );
        assert_eq!(
            parse_strs(&["--check-config", "--config=/etc/x.toml"]),
            Ok(Command::CheckConfig(with_config))
        );
        assert_eq!(
            parse_strs(&["--print-default-config", "--check-config"]),
            Ok(Command::PrintDefaultConfig)
        );
        assert_eq!(
            parse_strs(&["--print-default-config", "--help"]),
            Ok(Command::Help)
        );
        assert_eq!(
            parse_strs(&["--config"]),
            Err(CliError::MissingValue("--config"))
        );
        assert_eq!(
            parse_strs(&["--config="]),
            Err(CliError::MissingValue("--config"))
        );
        assert_eq!(
            parse_strs(&["--check-config=yes"]),
            Err(CliError::UnknownArgument("--check-config=yes".into()))
        );
    }

    #[test]
    fn unknown_arguments_are_rejected() {
        assert_eq!(
            parse_strs(&["--nope"]),
            Err(CliError::UnknownArgument("--nope".into()))
        );
        assert_eq!(
            parse_strs(&["--help=1"]),
            Err(CliError::UnknownArgument("--help=1".into()))
        );
        assert_eq!(
            parse_strs(&["extra"]),
            Err(CliError::UnknownArgument("extra".into()))
        );
    }

    #[test]
    fn non_utf8_argument_is_an_error() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![0x66, 0xff]);
        assert!(matches!(parse([bad]), Err(CliError::NotUnicode(_))));
    }
}
