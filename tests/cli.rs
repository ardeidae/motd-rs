//! End-to-end tests: run the real binary against fixture roots.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_motd-rs");

fn run(args: &[&str]) -> Output {
    Command::new(BIN).args(args).output().expect("binary runs")
}

fn fixture(name: &str) -> String {
    format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn renders_minimal_fixture() {
    let out = run(&["--sysroot", &fixture("minimal")]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "raspberrypi  Debian GNU/Linux 13 (trixie)  6.12.20-v8+ aarch64  up 19d 0h06m\n"
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

#[test]
fn missing_root_still_succeeds() {
    let out = run(&["--sysroot", "/nonexistent-motd-rs-root"]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8(out.stdout).unwrap(), "?\n");
}

#[test]
fn invalid_fake_toml_exits_1() {
    let dir = std::env::temp_dir().join(format!("motd-rs-bad-fake-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("fake.toml"), "bogus = 1\n").unwrap();
    let out = run(&["--sysroot", dir.to_str().unwrap()]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("invalid fake.toml")
    );
}

#[test]
fn version_and_help() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("motd-rs {}\n", env!("CARGO_PKG_VERSION"))
    );
    let out = run(&["--help"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .starts_with("motd-rs - ")
    );
}

#[test]
fn bad_argument_exits_1_with_message() {
    let out = run(&["--bogus"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("unknown argument '--bogus'")
    );
}

#[test]
fn real_machine_does_not_crash() {
    let out = run(&[]);
    assert!(out.status.success());
    assert_ne!(String::from_utf8_lossy(&out.stdout), "");
}
