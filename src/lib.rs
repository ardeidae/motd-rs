//! motd-rs: a fast, compact, read-only system status dashboard for the Linux
//! message of the day.
//!
//! Layers: `collect` (I/O, raw facts) → `render` (pure, facts → text).
//! All file access goes through [`sysroot::Sysroot`] so the whole pipeline can
//! run against a fake root filesystem in tests.

pub mod cli;
pub mod collect;
pub mod parse;
pub mod render;
pub mod sysroot;

use cli::Options;

/// Collects everything and renders the report as text.
#[must_use]
pub fn run(options: &Options) -> String {
    let host = collect::host::collect(&options.sysroot);
    let mut out = render::host_line(&host);
    out.push('\n');
    out
}
