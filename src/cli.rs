//! Hand-written command-line parsing: no dependency, `clap` would add about 300 KB.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use crate::collect::DEFAULT_DEADLINE;
use crate::config::{Charset, ColorMode, Display, Icons, MIN_WIDTH, Width};
use crate::sysroot::Sysroot;

/// Longest `--deadline` accepted: a login must never wait longer than this.
pub const MAX_DEADLINE_MS: u64 = 10_000;

/// Text printed by `--help`.
pub const HELP: &str = "\
motd-rs - compact system status for the Linux message of the day

Usage: motd-rs [OPTIONS]

Options:
      --config <FILE>           Read this config file instead of searching for one
      --width <auto|N>          Output width in columns (N >= 20); auto = terminal, 100 otherwise
      --icons <ascii|nerd>      Icon set (nerd needs a Nerd Font)
      --color <auto|always|never>
                                When to use colours (auto honours NO_COLOR and CLICOLOR_FORCE)
      --charset <auto|utf8|ascii>
                                Output character set; auto reads LC_ALL, LC_CTYPE, LANG
      --output <FILE>           Write the report to FILE atomically instead of stdout
      --deadline <MS>           Give up on slow sections after MS milliseconds (1-10000, default 250)
      --check-config            Check the config file, print the error if any, exit 1 on error
      --print-default-config    Print the default config file, with comments
  -h, --help                    Print this help
  -V, --version                 Print the version

Config search (first found wins): --config, $MOTD_RS_CONFIG,
$XDG_CONFIG_HOME/motd-rs/config.toml (not for root), /etc/motd-rs/config.toml.
--width, --icons, --color and --charset win over the config file.

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
#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    /// Root used for every file read (hidden `--sysroot`, for tests and fixtures).
    pub sysroot: Sysroot,
    /// `--config <file>`: the only config file considered.
    pub config: Option<PathBuf>,
    /// Display settings given on the command line.
    pub display: DisplayOverrides,
    /// `--output <file>`: written atomically instead of stdout.
    pub output: Option<PathBuf>,
    /// `--deadline`: how long the collectors get.
    pub deadline: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            sysroot: Sysroot::default(),
            config: None,
            display: DisplayOverrides::default(),
            output: None,
            deadline: DEFAULT_DEADLINE,
        }
    }
}

/// `[display]` keys set on the command line. They win over the config file,
/// so a systemd unit or a script can pin the look whatever the file says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DisplayOverrides {
    /// `--width`.
    pub width: Option<Width>,
    /// `--icons`.
    pub icons: Option<Icons>,
    /// `--color`.
    pub color: Option<ColorMode>,
    /// `--charset`.
    pub charset: Option<Charset>,
}

impl DisplayOverrides {
    /// Replaces the config file's values with the ones given on the command line.
    pub fn apply(&self, display: &mut Display) {
        if let Some(width) = self.width {
            display.width = width;
        }
        if let Some(icons) = self.icons {
            display.icons = icons;
        }
        if let Some(color) = self.color {
            display.color = color;
        }
        if let Some(charset) = self.charset {
            display.charset = charset;
        }
    }
}

/// Command-line errors.
#[derive(Debug, PartialEq, Eq)]
pub enum CliError {
    UnknownArgument(String),
    MissingValue(&'static str),
    InvalidValue {
        flag: &'static str,
        value: String,
        expected: &'static str,
    },
    NotUnicode(OsString),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownArgument(arg) => write!(f, "unknown argument '{arg}'"),
            Self::MissingValue(flag) => write!(f, "{flag} needs a value"),
            Self::InvalidValue {
                flag,
                value,
                expected,
            } => write!(f, "invalid value '{value}' for {flag}: expected {expected}"),
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
            ("--output", inline_value) => {
                options.output = Some(PathBuf::from(value("--output", inline_value, &mut args)?));
            }
            ("--width", inline_value) => {
                let raw = value("--width", inline_value, &mut args)?;
                options.display.width = Some(parse_width(raw)?);
            }
            ("--icons", inline_value) => {
                let raw = value("--icons", inline_value, &mut args)?;
                options.display.icons = Some(match raw.as_str() {
                    "ascii" => Icons::Ascii,
                    "nerd" => Icons::Nerd,
                    _ => return Err(invalid("--icons", raw, "ascii or nerd")),
                });
            }
            ("--color", inline_value) => {
                let raw = value("--color", inline_value, &mut args)?;
                options.display.color = Some(match raw.as_str() {
                    "auto" => ColorMode::Auto,
                    "always" => ColorMode::Always,
                    "never" => ColorMode::Never,
                    _ => return Err(invalid("--color", raw, "auto, always or never")),
                });
            }
            ("--charset", inline_value) => {
                let raw = value("--charset", inline_value, &mut args)?;
                options.display.charset = Some(match raw.as_str() {
                    "auto" => Charset::Auto,
                    "utf8" => Charset::Utf8,
                    "ascii" => Charset::Ascii,
                    _ => return Err(invalid("--charset", raw, "auto, utf8 or ascii")),
                });
            }
            ("--deadline", inline_value) => {
                let raw = value("--deadline", inline_value, &mut args)?;
                options.deadline = parse_deadline(raw)?;
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

fn invalid(flag: &'static str, value: String, expected: &'static str) -> CliError {
    CliError::InvalidValue {
        flag,
        value,
        expected,
    }
}

/// Plain decimal digits only: `u64::from_str` would also take `+20`.
fn decimal(raw: &str) -> Option<u64> {
    if raw.bytes().all(|b| b.is_ascii_digit()) {
        raw.parse().ok()
    } else {
        None
    }
}

fn parse_width(raw: String) -> Result<Width, CliError> {
    const EXPECTED: &str = "auto or a number of columns from 20 to 65535";
    if raw == "auto" {
        return Ok(Width::Auto);
    }
    match decimal(&raw).and_then(|n| u16::try_from(n).ok()) {
        Some(n) if n >= MIN_WIDTH => Ok(Width::Fixed(n)),
        _ => Err(invalid("--width", raw, EXPECTED)),
    }
}

fn parse_deadline(raw: String) -> Result<Duration, CliError> {
    match decimal(&raw) {
        Some(ms @ 1..=MAX_DEADLINE_MS) => Ok(Duration::from_millis(ms)),
        _ => Err(invalid(
            "--deadline",
            raw,
            "a number of milliseconds from 1 to 10000",
        )),
    }
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
    fn display_overrides() {
        let Ok(Command::Run(options)) = parse_strs(&[
            "--width",
            "80",
            "--icons=nerd",
            "--color",
            "never",
            "--charset=ascii",
        ]) else {
            panic!("expected a run");
        };
        assert_eq!(
            options.display,
            DisplayOverrides {
                width: Some(Width::Fixed(80)),
                icons: Some(Icons::Nerd),
                color: Some(ColorMode::Never),
                charset: Some(Charset::Ascii),
            }
        );
        let Ok(Command::Run(options)) = parse_strs(&["--width=auto", "--color=always"]) else {
            panic!("expected a run");
        };
        assert_eq!(options.display.width, Some(Width::Auto));
        assert_eq!(options.display.color, Some(ColorMode::Always));
    }

    #[test]
    fn last_flag_wins() {
        let Ok(Command::Run(options)) = parse_strs(&["--icons=nerd", "--icons=ascii"]) else {
            panic!("expected a run");
        };
        assert_eq!(options.display.icons, Some(Icons::Ascii));
    }

    #[test]
    fn overrides_win_over_the_config() {
        let mut display = Display {
            width: Width::Fixed(140),
            icons: Icons::Nerd,
            color: ColorMode::Always,
            charset: Charset::Utf8,
            ..Display::default()
        };
        let before = display.clone();
        DisplayOverrides::default().apply(&mut display);
        assert_eq!(display, before, "no flag changes nothing");
        DisplayOverrides {
            width: Some(Width::Auto),
            icons: None,
            color: Some(ColorMode::Never),
            charset: None,
        }
        .apply(&mut display);
        assert_eq!(display.width, Width::Auto);
        assert_eq!(display.icons, Icons::Nerd);
        assert_eq!(display.color, ColorMode::Never);
        assert_eq!(display.charset, Charset::Utf8);
    }

    #[test]
    fn bad_display_values_are_rejected() {
        for (args, flag) in [
            (&["--width", "19"][..], "--width"),
            (&["--width", "65536"], "--width"),
            (&["--width", "+80"], "--width"),
            (&["--width", "80x"], "--width"),
            (&["--width", "AUTO"], "--width"),
            (&["--icons", "emoji"], "--icons"),
            (&["--color", "yes"], "--color"),
            (&["--charset", "latin1"], "--charset"),
        ] {
            assert!(
                matches!(parse_strs(args), Err(CliError::InvalidValue { flag: f, .. }) if f == flag),
                "{args:?}"
            );
        }
        assert_eq!(
            parse_strs(&["--width", "20"]).map(|c| match c {
                Command::Run(o) => o.display.width,
                _ => None,
            }),
            Ok(Some(Width::Fixed(20)))
        );
        assert_eq!(
            parse_strs(&["--icons"]),
            Err(CliError::MissingValue("--icons"))
        );
    }

    #[test]
    fn deadline() {
        assert_eq!(Options::default().deadline, DEFAULT_DEADLINE);
        for (raw, ms) in [("1", 1), ("500", 500), ("10000", 10_000)] {
            let Ok(Command::Run(options)) = parse_strs(&["--deadline", raw]) else {
                panic!("expected a run for {raw}");
            };
            assert_eq!(options.deadline, Duration::from_millis(ms));
        }
        for raw in [
            "0",
            "10001",
            "-5",
            "1.5",
            "250ms",
            "99999999999999999999999",
        ] {
            assert!(
                matches!(
                    parse_strs(&["--deadline", raw]),
                    Err(CliError::InvalidValue {
                        flag: "--deadline",
                        ..
                    })
                ),
                "{raw}"
            );
        }
    }

    #[test]
    fn output_option() {
        assert_eq!(
            parse_strs(&["--output=/run/motd.d/50-motd-rs"]),
            Ok(Command::Run(Options {
                output: Some(PathBuf::from("/run/motd.d/50-motd-rs")),
                ..Options::default()
            }))
        );
        assert_eq!(
            parse_strs(&["--output", ""]),
            Err(CliError::MissingValue("--output"))
        );
    }

    #[test]
    fn invalid_value_message() {
        assert_eq!(
            invalid("--color", "yes".into(), "auto, always or never").to_string(),
            "invalid value 'yes' for --color: expected auto, always or never"
        );
    }

    #[test]
    fn non_utf8_argument_is_an_error() {
        use std::os::unix::ffi::OsStringExt;
        let bad = OsString::from_vec(vec![0x66, 0xff]);
        assert!(matches!(parse([bad]), Err(CliError::NotUnicode(_))));
    }
}
