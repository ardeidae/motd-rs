//! Configuration: a TOML file read with serde, merged over built-in defaults.
//!
//! A broken file never blocks a login: [`load`] then returns the defaults plus
//! the error, which the output shows as one alert line. `--check-config`
//! reports the same error and fails, for CI and configuration management.

use std::ffi::OsString;
use std::fmt;
use std::fs::File;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::{self, Deserializer, Visitor};

use crate::sysroot::Sysroot;

/// The commented default file, printed by `--print-default-config`. A test
/// checks that it parses to exactly [`Config::default`].
pub const DEFAULT_CONFIG: &str = r#"# motd-rs configuration. Every key is optional: a missing key keeps the
# default shown here. Search order: --config <file>, $MOTD_RS_CONFIG,
# $XDG_CONFIG_HOME/motd-rs/config.toml (not for root), /etc/motd-rs/config.toml.

[display]
width    = "auto"    # "auto" (terminal width, 100 when not a terminal) or a number >= 20
max_rows = 8         # rows per column; the rest is summarised as "+N more"
icons    = "ascii"   # "ascii" | "nerd" (Nerd Font glyphs, same layout)
color    = "auto"    # "auto" | "always" | "never" (NO_COLOR and CLICOLOR_FORCE are honoured)
charset  = "auto"    # "auto" (from LC_ALL / LC_CTYPE / LANG) | "utf8" | "ascii"
header   = ["host", "net", "users", "load", "temp", "pressure", "alerts"]
columns  = ["disks", "memory", "services"]

# [warn, crit]. A value at or above warn is shown as a warning, at or above
# crit as critical. 0 is valid (always at least a warning).
[thresholds]
disk        = [80, 90]      # % of space used
inodes      = [80, 90]      # % of inodes used
memory      = [80, 90]      # % used: (MemTotal - MemAvailable) / MemTotal
swap        = [10, 20]      # % used
load        = [1.0, 2.0]    # load average per online CPU
pressure    = [10, 25]      # PSI "some avg10", %
temperature = [70, 85]      # degrees Celsius

[disks]
exclude         = ["/boot", "/boot/efi", "/snap/*"]   # mount points; globs: ? and * (* also matches /)
include_network = false                               # NFS, CIFS, ... (statvfs can hang)

[services]
watch        = []      # systemd units ("sshd", ".service" implied) or "process:<name>"
failed_units = true    # alert on any failed unit, watched or not

[ports]
watch = []             # "22/tcp", "22/tcp6", "53/udp", "53/udp6"
"#;

/// Largest config file accepted. Guards against `--config /dev/zero` or a
/// runaway file: the login must not wait on it.
const MAX_FILE_BYTES: u64 = 64 * 1024;

/// The whole configuration.
#[derive(Debug, Clone, PartialEq, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    /// Layout and look.
    pub display: Display,
    /// `[warn, crit]` levels.
    pub thresholds: Thresholds,
    /// Which filesystems are shown.
    pub disks: Disks,
    /// Watched units and processes.
    pub services: Services,
    /// Watched listening ports.
    pub ports: Ports,
}

/// `[display]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Display {
    /// Output width.
    pub width: Width,
    /// Rows per column before `+N more`.
    pub max_rows: u16,
    /// Icon set.
    pub icons: Icons,
    /// When to emit colour escapes.
    pub color: ColorMode,
    /// Output character set.
    pub charset: Charset,
    /// Header lines, in order.
    pub header: Vec<HeaderItem>,
    /// Grid columns, in order.
    pub columns: Vec<Column>,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            width: Width::Auto,
            max_rows: 8,
            icons: Icons::Ascii,
            color: ColorMode::Auto,
            charset: Charset::Auto,
            header: vec![
                HeaderItem::Host,
                HeaderItem::Net,
                HeaderItem::Users,
                HeaderItem::Load,
                HeaderItem::Temp,
                HeaderItem::Pressure,
                HeaderItem::Alerts,
            ],
            columns: vec![Column::Disks, Column::Memory, Column::Services],
        }
    }
}

/// Smallest fixed width: below it not even one column fits.
pub const MIN_WIDTH: u16 = 20;

/// `width = "auto"` or a number of columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    /// Terminal width, 100 when stdout is not a terminal.
    Auto,
    /// Fixed number of columns.
    Fixed(u16),
}

impl<'de> Deserialize<'de> for Width {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct WidthVisitor;

        impl Visitor<'_> for WidthVisitor {
            type Value = Width;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("\"auto\" or a number of columns")
            }

            fn visit_str<E: de::Error>(self, value: &str) -> Result<Width, E> {
                if value == "auto" {
                    Ok(Width::Auto)
                } else {
                    Err(E::invalid_value(de::Unexpected::Str(value), &self))
                }
            }

            fn visit_i64<E: de::Error>(self, value: i64) -> Result<Width, E> {
                u16::try_from(value)
                    .map(Width::Fixed)
                    .map_err(|_| E::invalid_value(de::Unexpected::Signed(value), &self))
            }
        }

        deserializer.deserialize_any(WidthVisitor)
    }
}

/// `icons`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Icons {
    /// 7-bit decoration only.
    Ascii,
    /// Nerd Font glyphs; same layout as `ascii`.
    Nerd,
}

/// `color`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ColorMode {
    /// Colour when stdout is a terminal (`NO_COLOR` / `CLICOLOR_FORCE` honoured).
    Auto,
    /// Always colour.
    Always,
    /// Never colour.
    Never,
}

/// `charset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Charset {
    /// From the locale variables.
    Auto,
    /// UTF-8 output.
    Utf8,
    /// 7-bit output: system strings are transliterated.
    Ascii,
}

/// One entry of `header`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeaderItem {
    /// Hostname, OS, kernel, uptime, date.
    Host,
    /// Default route interface and addresses.
    Net,
    /// Users and sessions.
    Users,
    /// Processes and load average.
    Load,
    /// Temperature.
    Temp,
    /// PSI pressure.
    Pressure,
    /// The `!!` alert line.
    Alerts,
}

/// One entry of `columns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Column {
    /// Filesystems.
    Disks,
    /// RAM and swap.
    Memory,
    /// Watched services and ports.
    Services,
}

/// A `[warn, crit]` pair.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(try_from = "Vec<T>")]
pub struct Levels<T> {
    /// From this value on: warning.
    pub warn: T,
    /// From this value on: critical.
    pub crit: T,
}

// Through a `Vec` rather than a `(T, T)` tuple: the tuple silently ignores a
// third element, and `[80, 90, 95]` is more likely a typo than intended.
impl<T> TryFrom<Vec<T>> for Levels<T> {
    type Error = String;

    fn try_from(values: Vec<T>) -> Result<Self, String> {
        let count = values.len();
        let mut values = values.into_iter();
        match (values.next(), values.next(), values.next()) {
            (Some(warn), Some(crit), None) => Ok(Self { warn, crit }),
            _ => Err(format!("expected [warn, crit], found {count} values")),
        }
    }
}

const fn levels<T>(warn: T, crit: T) -> Levels<T> {
    Levels { warn, crit }
}

/// `[thresholds]`. Percentages and degrees are whole numbers.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Thresholds {
    /// % of filesystem space used.
    pub disk: Levels<u8>,
    /// % of filesystem inodes used.
    pub inodes: Levels<u8>,
    /// % of RAM used, from `MemAvailable`.
    pub memory: Levels<u8>,
    /// % of swap used.
    pub swap: Levels<u8>,
    /// Load average per online CPU.
    pub load: Levels<f64>,
    /// PSI `some avg10`, %.
    pub pressure: Levels<u8>,
    /// Degrees Celsius.
    pub temperature: Levels<u8>,
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            disk: levels(80, 90),
            inodes: levels(80, 90),
            memory: levels(80, 90),
            swap: levels(10, 20),
            load: levels(1.0, 2.0),
            pressure: levels(10, 25),
            temperature: levels(70, 85),
        }
    }
}

/// `[disks]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Disks {
    /// Mount points to hide (globs with `*` and `?`).
    pub exclude: Vec<String>,
    /// Show network filesystems. Off by default: `statvfs` on a dead server
    /// can hang in the kernel.
    pub include_network: bool,
}

impl Default for Disks {
    fn default() -> Self {
        Self {
            exclude: ["/boot", "/boot/efi", "/snap/*"].map(String::from).to_vec(),
            include_network: false,
        }
    }
}

/// `[services]`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Services {
    /// Watched entries, in display order.
    pub watch: Vec<Watched>,
    /// Alert on every failed unit, watched or not.
    pub failed_units: bool,
}

impl Default for Services {
    fn default() -> Self {
        Self {
            watch: Vec::new(),
            failed_units: true,
        }
    }
}

/// One `services.watch` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub enum Watched {
    /// A systemd unit, full name (`sshd` is stored as `sshd.service`).
    Unit(String),
    /// `process:<name>`: a process whose `comm` is `<name>`.
    Process(String),
}

impl TryFrom<String> for Watched {
    type Error = String;

    fn try_from(entry: String) -> Result<Self, String> {
        if let Some(name) = entry.strip_prefix("process:") {
            // The kernel keeps 15 bytes of a process name in `comm`.
            return match name.len() {
                0 => Err("\"process:\" needs a process name".into()),
                1..=15 => Ok(Self::Process(name.to_owned())),
                _ => Err(format!(
                    "process name {name:?} is longer than 15 bytes, the kernel truncates it"
                )),
            };
        }
        if entry.is_empty() || entry.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return Err(format!("{entry:?} is not a unit name"));
        }
        // systemd unit names always carry a type suffix; a bare name means a service.
        Ok(if entry.contains('.') {
            Self::Unit(entry)
        } else {
            Self::Unit(format!("{entry}.service"))
        })
    }
}

/// `[ports]`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Ports {
    /// Watched ports, in display order.
    pub watch: Vec<WatchedPort>,
}

/// One `ports.watch` entry: `"22/tcp"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct WatchedPort {
    /// Port number.
    pub port: u16,
    /// Socket family and protocol.
    pub proto: Proto,
}

/// Protocol of a watched port, as in `/proc/net/<proto>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Proto {
    /// TCP over IPv4.
    Tcp,
    /// TCP over IPv6.
    Tcp6,
    /// UDP over IPv4.
    Udp,
    /// UDP over IPv6.
    Udp6,
}

impl TryFrom<String> for WatchedPort {
    type Error = String;

    fn try_from(entry: String) -> Result<Self, String> {
        let bad = || format!("{entry:?} is not \"<port>/<tcp|tcp6|udp|udp6>\"");
        let (port, proto) = entry.split_once('/').ok_or_else(bad)?;
        let proto = match proto {
            "tcp" => Proto::Tcp,
            "tcp6" => Proto::Tcp6,
            "udp" => Proto::Udp,
            "udp6" => Proto::Udp6,
            _ => return Err(bad()),
        };
        // Plain decimal only: no sign, no leading zero, no port 0.
        if port.starts_with('0') || !port.bytes().all(|b| b.is_ascii_digit()) {
            return Err(bad());
        }
        let port = port.parse::<u16>().map_err(|_| bad())?;
        Ok(Self { port, proto })
    }
}

/// Why the config file was not used.
#[derive(Debug)]
pub enum ConfigError {
    /// The file exists (or was named explicitly) but cannot be read.
    Io {
        /// The file.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// The file is larger than any sane config.
    TooLarge {
        /// The file.
        path: PathBuf,
    },
    /// Not valid TOML, or a key or value serde rejected.
    Syntax {
        /// The file.
        path: PathBuf,
        /// The TOML error, with line, column and key.
        error: Box<toml::de::Error>,
    },
    /// Valid TOML, but the values do not make sense together.
    Invalid {
        /// The file.
        path: PathBuf,
        /// Dotted key (`thresholds.disk`).
        key: &'static str,
        /// What is wrong.
        reason: String,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::TooLarge { path } => write!(
                f,
                "{}: larger than {MAX_FILE_BYTES} bytes, not a config file",
                path.display()
            ),
            // toml's message ends with a newline; the caller adds its own.
            Self::Syntax { path, error } => {
                write!(f, "{}: {}", path.display(), error.to_string().trim_end())
            }
            Self::Invalid { path, key, reason } => {
                write!(f, "{}: {key}: {reason}", path.display())
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Syntax { error, .. } => Some(error.as_ref()),
            Self::TooLarge { .. } | Self::Invalid { .. } => None,
        }
    }
}

/// A place where a config file may be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The file.
    pub path: PathBuf,
    /// Named explicitly (`--config`, `$MOTD_RS_CONFIG`): a missing file is an
    /// error instead of a reason to look further.
    pub required: bool,
}

/// Where to look, first found wins.
///
/// An explicit file (`--config`, else `$MOTD_RS_CONFIG`) is the only
/// candidate. Otherwise: the user's own file (not for root, whose view is the
/// system one), then `/etc/motd-rs/config.toml` under `sysroot`.
/// `env` reads an environment variable; it is a parameter so tests do not
/// depend on the environment they run in.
pub fn candidates(
    explicit: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
    euid: u32,
    sysroot: &Sysroot,
) -> Vec<Candidate> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    if let Some(path) = explicit
        .map(Path::to_path_buf)
        .or_else(|| set("MOTD_RS_CONFIG").map(PathBuf::from))
    {
        return vec![Candidate {
            path,
            required: true,
        }];
    }
    let mut found = Vec::new();
    if euid != 0 {
        // XDG base directory spec: a relative XDG_CONFIG_HOME is invalid and ignored.
        let config_home = set("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                set("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|home| home.join(".config"))
            });
        if let Some(dir) = config_home {
            found.push(Candidate {
                path: dir.join("motd-rs/config.toml"),
                required: false,
            });
        }
    }
    found.push(Candidate {
        path: sysroot.path("/etc/motd-rs/config.toml"),
        required: false,
    });
    found
}

/// The configuration a run uses.
#[derive(Debug, Default)]
pub struct Loaded {
    /// The file's settings, or the defaults if there is no usable file.
    pub config: Config,
    /// The file that was found, if any (even when it was rejected).
    pub path: Option<PathBuf>,
    /// Why the file was rejected. The defaults are used instead.
    pub error: Option<ConfigError>,
}

/// Reads the first existing candidate. Never fails: an error is returned
/// next to the defaults.
#[must_use]
pub fn load(candidates: &[Candidate]) -> Loaded {
    for candidate in candidates {
        let path = &candidate.path;
        let text = match read_limited(path) {
            Ok(text) => text,
            Err(source) if source.kind() == io::ErrorKind::NotFound && !candidate.required => {
                continue;
            }
            Err(source) if source.kind() == io::ErrorKind::FileTooLarge => {
                return rejected(path, ConfigError::TooLarge { path: path.clone() });
            }
            Err(source) => {
                let error = ConfigError::Io {
                    path: path.clone(),
                    source,
                };
                return rejected(path, error);
            }
        };
        return match parse(&text, path) {
            Ok(config) => Loaded {
                config,
                path: Some(path.clone()),
                error: None,
            },
            Err(error) => rejected(path, error),
        };
    }
    Loaded::default()
}

fn rejected(path: &Path, error: ConfigError) -> Loaded {
    Loaded {
        config: Config::default(),
        path: Some(path.to_path_buf()),
        error: Some(error),
    }
}

/// Reads at most [`MAX_FILE_BYTES`]; more is `FileTooLarge`.
fn read_limited(path: &Path) -> io::Result<String> {
    let mut text = String::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)?;
    if u64::try_from(text.len()).map_or(true, |len| len > MAX_FILE_BYTES) {
        return Err(io::Error::from(io::ErrorKind::FileTooLarge));
    }
    Ok(text)
}

/// Parses and validates a config file's text. `path` is only used in errors.
///
/// # Errors
///
/// [`ConfigError::Syntax`] or [`ConfigError::Invalid`].
pub fn parse(text: &str, path: &Path) -> Result<Config, ConfigError> {
    let config: Config = toml::from_str(text).map_err(|error| ConfigError::Syntax {
        path: path.to_path_buf(),
        error: Box::new(error),
    })?;
    validate(&config).map_err(|(key, reason)| ConfigError::Invalid {
        path: path.to_path_buf(),
        key,
        reason,
    })?;
    Ok(config)
}

/// Checks what serde cannot: ranges and consistency between values.
fn validate(config: &Config) -> Result<(), (&'static str, String)> {
    let display = &config.display;
    if let Width::Fixed(width) = display.width
        && width < MIN_WIDTH
    {
        return Err(("display.width", format!("must be at least {MIN_WIDTH}")));
    }
    if display.max_rows == 0 {
        return Err(("display.max_rows", "must be at least 1".into()));
    }
    if has_duplicates(&display.header) {
        return Err(("display.header", "lists an entry twice".into()));
    }
    if has_duplicates(&display.columns) {
        return Err(("display.columns", "lists an entry twice".into()));
    }

    let t = &config.thresholds;
    for (key, levels) in [
        ("thresholds.disk", t.disk),
        ("thresholds.inodes", t.inodes),
        ("thresholds.memory", t.memory),
        ("thresholds.swap", t.swap),
        ("thresholds.pressure", t.pressure),
    ] {
        if levels.crit > 100 {
            return Err((key, "a percentage cannot exceed 100".into()));
        }
        check_order(key, &levels)?;
    }
    check_order("thresholds.temperature", &t.temperature)?;
    if !(t.load.warn.is_finite() && t.load.crit.is_finite() && t.load.warn >= 0.0) {
        return Err(("thresholds.load", "must be finite and not negative".into()));
    }
    check_order("thresholds.load", &t.load)?;

    if config.disks.exclude.iter().any(String::is_empty) {
        return Err(("disks.exclude", "contains an empty pattern".into()));
    }
    if has_duplicates(&config.services.watch) {
        return Err(("services.watch", "lists an entry twice".into()));
    }
    if has_duplicates(&config.ports.watch) {
        return Err(("ports.watch", "lists an entry twice".into()));
    }
    Ok(())
}

fn check_order<T: PartialOrd + fmt::Display>(
    key: &'static str,
    levels: &Levels<T>,
) -> Result<(), (&'static str, String)> {
    if levels.warn <= levels.crit {
        Ok(())
    } else {
        Err((
            key,
            format!(
                "warn ({}) is above crit ({}): expected [warn, crit]",
                levels.warn, levels.crit
            ),
        ))
    }
}

fn has_duplicates<T: PartialEq>(items: &[T]) -> bool {
    items
        .iter()
        .enumerate()
        .any(|(i, item)| items.iter().skip(i + 1).any(|other| other == item))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(text: &str) -> Result<Config, ConfigError> {
        parse(text, Path::new("test.toml"))
    }

    fn error(text: &str) -> String {
        parse_str(text).unwrap_err().to_string()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("motd-rs-config-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn default_file_is_the_defaults() {
        assert_eq!(parse_str(DEFAULT_CONFIG).unwrap(), Config::default());
    }

    #[test]
    fn empty_file_is_the_defaults() {
        assert_eq!(parse_str("").unwrap(), Config::default());
        assert_eq!(parse_str("[display]\n").unwrap(), Config::default());
    }

    #[test]
    fn missing_keys_keep_their_default() {
        let config = parse_str("[thresholds]\ndisk = [70, 95]\n").unwrap();
        assert_eq!(config.thresholds.disk, levels(70, 95));
        assert_eq!(config.thresholds.memory, levels(80, 90));
        assert_eq!(config.display, Display::default());
    }

    #[test]
    fn full_file() {
        let config = parse_str(
            r#"
            [display]
            width = 120
            max_rows = 3
            icons = "nerd"
            color = "never"
            charset = "utf8"
            header = ["host", "alerts"]
            columns = ["memory"]
            [thresholds]
            load = [0.5, 4]
            temperature = [0, 0]
            [disks]
            exclude = []
            include_network = true
            [services]
            watch = ["sshd", "ssh.socket", "process:myapp"]
            failed_units = false
            [ports]
            watch = ["22/tcp", "53/udp6"]
            "#,
        )
        .unwrap();
        let display = &config.display;
        assert_eq!(display.width, Width::Fixed(120));
        assert_eq!(display.max_rows, 3);
        assert_eq!(display.icons, Icons::Nerd);
        assert_eq!(display.color, ColorMode::Never);
        assert_eq!(display.charset, Charset::Utf8);
        assert_eq!(display.header, [HeaderItem::Host, HeaderItem::Alerts]);
        assert_eq!(display.columns, [Column::Memory]);
        assert_eq!(config.thresholds.load, levels(0.5, 4.0));
        assert_eq!(config.thresholds.temperature, levels(0, 0));
        assert_eq!(config.disks.exclude.len(), 0);
        assert!(config.disks.include_network);
        assert_eq!(
            config.services.watch,
            [
                Watched::Unit("sshd.service".into()),
                Watched::Unit("ssh.socket".into()),
                Watched::Process("myapp".into()),
            ]
        );
        assert!(!config.services.failed_units);
        assert_eq!(
            config.ports.watch,
            [
                WatchedPort {
                    port: 22,
                    proto: Proto::Tcp
                },
                WatchedPort {
                    port: 53,
                    proto: Proto::Udp6
                },
            ]
        );
    }

    #[test]
    fn zero_thresholds_are_valid() {
        let config = parse_str("[thresholds]\ndisk = [0, 0]\nswap = [0, 100]\n").unwrap();
        assert_eq!(config.thresholds.disk, levels(0, 0));
        assert_eq!(config.thresholds.swap, levels(0, 100));
    }

    #[test]
    fn syntax_errors_name_the_place() {
        let message = error("[display]\nwidth = \n");
        assert!(message.starts_with("test.toml: "), "{message}");
        assert!(message.contains("line 2"), "{message}");

        let message = error("[display]\nicons = \"emoji\"\n");
        assert!(message.contains("emoji"), "{message}");
        assert!(!message.ends_with('\n'), "{message:?}");
        assert!(message.contains("line 2"), "{message}");
    }

    #[test]
    fn unknown_keys_are_rejected() {
        assert!(error("[display]\ncolour = \"never\"\n").contains("colour"));
        assert!(error("[nope]\n").contains("nope"));
        assert!(error("verbose = true\n").contains("verbose"));
    }

    #[test]
    fn bad_values_are_rejected() {
        for text in [
            "[display]\nwidth = \"wide\"\n",
            "[display]\nwidth = -1\n",
            "[display]\nwidth = 70000\n",
            "[display]\nheader = [\"disks\"]\n",
            "[display]\ncolumns = [\"host\"]\n",
            "[display]\nmax_rows = -1\n",
            "[thresholds]\ndisk = [80]\n",
            "[thresholds]\ndisk = [80, 90, 95]\n",
            "[thresholds]\ndisk = [80, 300]\n",
            "[thresholds]\ndisk = [80.5, 90]\n",
            "[thresholds]\nload = [\"1\", 2]\n",
            "[services]\nwatch = [\"\"]\n",
            "[services]\nwatch = [\"my unit\"]\n",
            "[services]\nwatch = [\"process:\"]\n",
            "[services]\nwatch = [\"process:a-very-long-process-name\"]\n",
            "[ports]\nwatch = [\"22\"]\n",
            "[ports]\nwatch = [\"22/sctp\"]\n",
            "[ports]\nwatch = [\"0/tcp\"]\n",
            "[ports]\nwatch = [\"65536/tcp\"]\n",
            "[ports]\nwatch = [\"+22/tcp\"]\n",
            "[ports]\nwatch = [\"022/tcp\"]\n",
            "[ports]\nwatch = [22]\n",
        ] {
            assert!(
                matches!(parse_str(text), Err(ConfigError::Syntax { .. })),
                "accepted: {text}"
            );
        }
    }

    #[test]
    fn inconsistent_values_are_rejected() {
        for (text, key) in [
            ("[display]\nwidth = 19\n", "display.width"),
            ("[display]\nmax_rows = 0\n", "display.max_rows"),
            (
                "[display]\nheader = [\"host\", \"host\"]\n",
                "display.header",
            ),
            (
                "[display]\ncolumns = [\"disks\", \"disks\"]\n",
                "display.columns",
            ),
            ("[thresholds]\ndisk = [90, 80]\n", "thresholds.disk"),
            ("[thresholds]\ninodes = [101, 101]\n", "thresholds.inodes"),
            (
                "[thresholds]\ntemperature = [90, 80]\n",
                "thresholds.temperature",
            ),
            ("[thresholds]\nload = [3.0, 2.0]\n", "thresholds.load"),
            ("[thresholds]\nload = [-1.0, 2.0]\n", "thresholds.load"),
            ("[thresholds]\nload = [1.0, inf]\n", "thresholds.load"),
            ("[thresholds]\nload = [nan, 2.0]\n", "thresholds.load"),
            ("[disks]\nexclude = [\"\"]\n", "disks.exclude"),
            (
                "[services]\nwatch = [\"sshd\", \"sshd.service\"]\n",
                "services.watch",
            ),
            ("[ports]\nwatch = [\"22/tcp\", \"22/tcp\"]\n", "ports.watch"),
        ] {
            match parse_str(text) {
                Err(ConfigError::Invalid { key: got, .. }) => assert_eq!(got, key, "{text}"),
                other => panic!("{text}: {other:?}"),
            }
        }
        assert_eq!(
            error("[thresholds]\ndisk = [90, 80]\n"),
            "test.toml: thresholds.disk: warn (90) is above crit (80): expected [warn, crit]"
        );
    }

    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    fn paths(found: &[Candidate]) -> Vec<(&str, bool)> {
        found
            .iter()
            .map(|c| (c.path.to_str().unwrap(), c.required))
            .collect()
    }

    #[test]
    fn search_order() {
        let root = Sysroot::default();
        let vars = [
            ("MOTD_RS_CONFIG", "/env.toml"),
            ("XDG_CONFIG_HOME", "/xdg"),
            ("HOME", "/home/u"),
        ];
        let explicit = candidates(Some(Path::new("/cli.toml")), env(&vars), 1000, &root);
        assert_eq!(paths(&explicit), [("/cli.toml", true)]);
        assert_eq!(
            paths(&candidates(None, env(&vars), 1000, &root)),
            [("/env.toml", true)]
        );
        assert_eq!(
            paths(&candidates(None, env(&vars[1..]), 1000, &root)),
            [
                ("/xdg/motd-rs/config.toml", false),
                ("/etc/motd-rs/config.toml", false)
            ]
        );
        assert_eq!(
            paths(&candidates(None, env(&vars[2..]), 1000, &root)),
            [
                ("/home/u/.config/motd-rs/config.toml", false),
                ("/etc/motd-rs/config.toml", false)
            ]
        );
    }

    #[test]
    fn root_and_odd_environments_only_use_the_system_file() {
        let root = Sysroot::new("/fixture");
        let system = [("/fixture/etc/motd-rs/config.toml", false)];
        let user = [("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/root")];
        assert_eq!(paths(&candidates(None, env(&user), 0, &root)), system);
        let relative = [("XDG_CONFIG_HOME", "rel"), ("HOME", "")];
        assert_eq!(
            paths(&candidates(None, env(&relative), 1000, &root)),
            system
        );
        let empty = [("MOTD_RS_CONFIG", "")];
        assert_eq!(paths(&candidates(None, env(&empty), 0, &root)), system);
    }

    #[test]
    fn load_takes_the_first_existing_file() {
        let dir = scratch("first");
        let user = dir.join("user.toml");
        let system = dir.join("system.toml");
        std::fs::write(&user, "[display]\nmax_rows = 2\n").unwrap();
        std::fs::write(&system, "[display]\nmax_rows = 5\n").unwrap();
        let optional = |path: &Path| Candidate {
            path: path.to_path_buf(),
            required: false,
        };

        let loaded = load(&[
            optional(&dir.join("absent.toml")),
            optional(&user),
            optional(&system),
        ]);
        assert_eq!(loaded.config.display.max_rows, 2);
        assert_eq!(loaded.path.as_deref(), Some(user.as_path()));
        assert!(loaded.error.is_none());

        let none = load(&[optional(&dir.join("absent.toml"))]);
        assert_eq!(none.config, Config::default());
        assert!(none.path.is_none() && none.error.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_file_gives_the_defaults_and_an_error() {
        let dir = scratch("broken");
        let broken = dir.join("broken.toml");
        let fallback = dir.join("fallback.toml");
        std::fs::write(&broken, "[display]\nmax_rows = 2\nbogus = 1\n").unwrap();
        std::fs::write(&fallback, "").unwrap();
        let loaded = load(&[
            Candidate {
                path: broken.clone(),
                required: false,
            },
            Candidate {
                path: fallback,
                required: false,
            },
        ]);
        // A rejected file is not skipped: falling back to another file would
        // hide the mistake.
        assert_eq!(loaded.config, Config::default());
        assert_eq!(loaded.path.as_deref(), Some(broken.as_path()));
        assert!(matches!(loaded.error, Some(ConfigError::Syntax { .. })));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_explicit_file_is_an_error() {
        let path = PathBuf::from("/nonexistent-motd-rs/config.toml");
        let loaded = load(&[Candidate {
            path: path.clone(),
            required: true,
        }]);
        assert_eq!(loaded.config, Config::default());
        assert!(matches!(
            loaded.error,
            Some(ConfigError::Io { ref source, .. }) if source.kind() == io::ErrorKind::NotFound
        ));
        assert!(
            loaded
                .error
                .unwrap()
                .to_string()
                .starts_with("/nonexistent-motd-rs/config.toml: ")
        );
    }

    #[test]
    fn an_endless_file_is_cut_short() {
        let loaded = load(&[Candidate {
            path: PathBuf::from("/dev/zero"),
            required: true,
        }]);
        assert!(matches!(loaded.error, Some(ConfigError::TooLarge { .. })));
        assert_eq!(loaded.config, Config::default());
    }

    #[test]
    fn a_directory_is_an_error_not_a_skip() {
        let dir = scratch("dir");
        let loaded = load(&[Candidate {
            path: dir.clone(),
            required: false,
        }]);
        assert!(matches!(loaded.error, Some(ConfigError::Io { .. })));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
