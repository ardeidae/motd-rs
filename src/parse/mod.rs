//! Pure parsers: `&str` → typed value, no I/O. Each one is tested on real
//! captured input and on malformed input.

use std::fmt;

pub mod cpu_list;
pub mod loadavg;
pub mod meminfo;
pub mod mountinfo;
pub mod os_release;
pub mod pid_list;
pub mod pid_stat;
pub mod uptime;

/// A parser failed on its input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// What was being parsed (`"/proc/uptime"`).
    pub source: &'static str,
    /// Why it failed.
    pub reason: String,
}

impl ParseError {
    pub(crate) fn new(source: &'static str, reason: impl Into<String>) -> Self {
        Self {
            source,
            reason: reason.into(),
        }
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "cannot parse {}: {}", self.source, self.reason)
    }
}

impl std::error::Error for ParseError {}
