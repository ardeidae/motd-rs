//! motd-rs: a fast, compact, read-only system status dashboard for the Linux
//! message of the day.
//!
//! Layers: `collect` (I/O, raw facts) → `render` (pure, facts → text).
//! All file access goes through [`sysroot::Sysroot`] and every syscall through
//! [`sys::Sys`], so the whole pipeline can run against a fake root filesystem
//! (plus its `fake.toml`) in tests.

pub mod cli;
pub mod collect;
pub mod parse;
pub mod render;
pub mod sys;
pub mod sysroot;

use cli::Options;

/// Collects everything and renders the report as text.
///
/// On the real root the syscalls go to libc. Under `--sysroot` they are
/// answered by the fixture's `fake.toml`, so a captured machine renders the
/// same everywhere.
///
/// # Errors
///
/// Only under `--sysroot`: the fixture's `fake.toml` is unreadable or invalid.
pub fn run(options: &Options) -> Result<String, sys::FakeError> {
    let sys: Box<dyn sys::Sys> = if options.sysroot == sysroot::Sysroot::default() {
        Box::new(sys::RealSys)
    } else {
        Box::new(sys::FakeSys::load(options.sysroot.root())?)
    };
    let host = collect::host::collect(&options.sysroot, sys.as_ref());
    let mut out = render::host_line(&host);
    out.push('\n');
    Ok(out)
}
