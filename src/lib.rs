//! motd-rs: a fast, compact, read-only system status dashboard for the Linux
//! message of the day.
//!
//! Layers: `collect` (I/O, raw facts) → `model` (pure data) → `render`
//! (pure, [`model::Report`] → text).
//! All file access goes through [`sysroot::Sysroot`] and every syscall through
//! [`sys::Sys`], so the whole pipeline can run against a fake root filesystem
//! (plus its `fake.toml`) in tests.

pub mod cli;
pub mod collect;
pub mod config;
pub mod eval;
pub mod fake;
pub mod glob;
pub mod model;
pub mod output;
pub mod parse;
pub mod render;
pub mod sys;
pub mod sysroot;
pub mod systemd;

use std::fmt;
use std::sync::Arc;

use cli::Options;

/// Why a command could not run.
#[derive(Debug)]
pub enum Error {
    /// Under `--sysroot`: the fixture's `fake.toml` is unreadable or invalid.
    Fake(fake::FakeError),
    /// `--check-config` found a problem in the config file.
    Config(config::ConfigError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fake(err) => err.fmt(f),
            Self::Config(err) => err.fmt(f),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Fake(err) => Some(err),
            Self::Config(err) => Some(err),
        }
    }
}

impl From<fake::FakeError> for Error {
    fn from(err: fake::FakeError) -> Self {
        Self::Fake(err)
    }
}

/// The syscall and systemd backends of a run.
type Backends = (Box<dyn sys::Sys>, Box<dyn systemd::Systemd>);

/// On the real root the syscalls go to libc and systemd is asked over D-Bus.
/// Under `--sysroot` both are answered by the fixture's `fake.toml`, so a
/// captured machine renders the same everywhere.
fn backends(options: &Options) -> Result<Backends, Error> {
    if options.sysroot == sysroot::Sysroot::default() {
        Ok((Box::new(sys::RealSys), Box::new(systemd::DbusSystemd)))
    } else {
        Ok(fake::Fake::load(options.sysroot.root())?.into_backends())
    }
}

fn load_config(options: &Options, sys: &dyn sys::Sys) -> config::Loaded {
    config::load(&config::candidates(
        options.config.as_deref(),
        |name| std::env::var_os(name),
        sys.euid(),
        &options.sysroot,
    ))
}

/// Collects everything and renders the report as text.
///
/// A broken config file does not stop the run: the defaults are used and the
/// report carries the error.
///
/// # Errors
///
/// Only under `--sysroot`: the fixture's `fake.toml` is unreadable or invalid.
pub fn run(options: &Options) -> Result<String, Error> {
    let (sys, systemd) = backends(options)?;
    let loaded = load_config(options, sys.as_ref());
    let mut config = loaded.config;
    options.display.apply(&mut config.display);
    let ctx = Arc::new(collect::Ctx {
        sysroot: options.sysroot.clone(),
        sys,
        systemd,
    });
    let collectors: Vec<Box<dyn collect::Collector>> = vec![
        Box::new(collect::load::Load),
        Box::new(collect::memory::Memory),
        Box::new(collect::disks::Disks::new(&config.disks)),
    ];
    let mut collected = collect::run(collectors, &ctx, options.deadline);
    eval::evaluate(&mut collected.sections, &config.thresholds);
    let now = ctx.sys.now();
    let report = model::Report {
        generated_at: now,
        host: collect::host::collect(&ctx.sysroot, ctx.sys.as_ref(), now),
        sections: collected.sections,
        meta: model::Meta {
            timings: collected.timings,
            config_error: loaded.error.map(|err| err.to_string()),
        },
    };
    let mut out = render::host_line(&report.host);
    out.push('\n');
    let lines = [
        render::load_line(&report.sections),
        render::memory_line(&report.sections),
        render::disks_line(&report.sections),
    ];
    for line in lines.into_iter().flatten() {
        out.push_str(&line);
        out.push('\n');
    }
    Ok(out)
}

/// `--check-config`: finds the config file a run would use and validates it.
/// Returns the line to print when it is fine.
///
/// # Errors
///
/// The config file is unreadable or invalid (or, under `--sysroot`, the
/// fixture's `fake.toml` is).
pub fn check_config(options: &Options) -> Result<String, Error> {
    let (sys, _) = backends(options)?;
    let loaded = load_config(options, sys.as_ref());
    match (loaded.error, loaded.path) {
        (Some(err), _) => Err(Error::Config(err)),
        (None, Some(path)) => Ok(format!("{}: OK\n", path.display())),
        (None, None) => Ok("no config file found, using the built-in defaults\n".to_owned()),
    }
}
