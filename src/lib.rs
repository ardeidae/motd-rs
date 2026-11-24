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
pub mod fake;
pub mod model;
pub mod parse;
pub mod render;
pub mod sys;
pub mod sysroot;
pub mod systemd;

use cli::Options;

/// Collects everything and renders the report as text.
///
/// On the real root the syscalls go to libc and systemd is asked over D-Bus.
/// Under `--sysroot` both are answered by the fixture's `fake.toml`, so a
/// captured machine renders the same everywhere.
///
/// # Errors
///
/// Only under `--sysroot`: the fixture's `fake.toml` is unreadable or invalid.
pub fn run(options: &Options) -> Result<String, fake::FakeError> {
    let (sys, systemd): (Box<dyn sys::Sys>, Box<dyn systemd::Systemd>) =
        if options.sysroot == sysroot::Sysroot::default() {
            (Box::new(sys::RealSys), Box::new(systemd::DbusSystemd))
        } else {
            fake::Fake::load(options.sysroot.root())?.into_backends()
        };
    let ctx = std::sync::Arc::new(collect::Ctx {
        sysroot: options.sysroot.clone(),
        sys,
        systemd,
    });
    // Section collectors are registered here as they are written.
    let collected = collect::run(Vec::new(), &ctx, collect::DEFAULT_DEADLINE);
    let report = model::Report {
        generated_at: ctx.sys.now(),
        host: collect::host::collect(&ctx.sysroot, ctx.sys.as_ref()),
        sections: collected.sections,
        meta: model::Meta {
            timings: collected.timings,
        },
    };
    let mut out = render::host_line(&report.host);
    out.push('\n');
    Ok(out)
}
