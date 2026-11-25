//! End-to-end tests: run the real binary against fixture roots.

use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_motd-rs");

fn run(args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .env_remove("MOTD_RS_CONFIG")
        .output()
        .expect("binary runs")
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("motd-rs-cli-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
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
        "raspberrypi  Debian GNU/Linux 13 (trixie)  6.12.20-v8+ aarch64  up 19d 0h06m\nnet ?\nprocs 0  load ?\nram ?\ndisks ?\n"
    );
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
}

#[test]
fn missing_root_still_succeeds() {
    let out = run(&["--sysroot", "/nonexistent-motd-rs-root"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "?\nnet ?\nprocs ?  load ?\nram ?\ndisks ?\n"
    );
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

#[test]
fn print_default_config() {
    let out = run(&["--print-default-config"]);
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(text.starts_with("# motd-rs configuration."));
    assert!(text.contains("[thresholds]"));
}

#[test]
fn check_config_without_a_file() {
    // Fixture roots have no /etc/motd-rs and fake root (euid 0) skips the
    // user's own file.
    let out = run(&["--check-config", "--sysroot", &fixture("minimal")]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "no config file found, using the built-in defaults\n"
    );
}

#[test]
fn check_config_good_and_bad_files() {
    let dir = scratch("check");
    let good = dir.join("good.toml");
    let bad = dir.join("bad.toml");
    std::fs::write(&good, "[display]\nmax_rows = 4\n").unwrap();
    std::fs::write(&bad, "[thresholds]\ndisk = [95, 90]\n").unwrap();

    let out = run(&["--check-config", "--config", good.to_str().unwrap()]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        format!("{}: OK\n", good.display())
    );

    let out = run(&["--check-config", "--config", bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.contains("thresholds.disk: warn (95) is above crit (90)"),
        "{err}"
    );

    // The environment variable is used when --config is absent.
    let out = Command::new(BIN)
        .args(["--check-config", "--sysroot", &fixture("minimal")])
        .env("MOTD_RS_CONFIG", &bad)
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn a_broken_config_does_not_stop_the_run() {
    let dir = scratch("broken");
    let bad = dir.join("bad.toml");
    std::fs::write(&bad, "this is not toml").unwrap();
    let out = run(&[
        "--sysroot",
        &fixture("minimal"),
        "--config",
        bad.to_str().unwrap(),
    ]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success());
    assert!(
        String::from_utf8(out.stdout)
            .unwrap()
            .starts_with("raspberrypi ")
    );
}

#[test]
fn output_writes_the_file_not_stdout() {
    let dir = scratch("output");
    let target = dir.join("50-motd-rs");
    let out = run(&[
        "--sysroot",
        &fixture("minimal"),
        "--output",
        target.to_str().unwrap(),
    ]);
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "raspberrypi  Debian GNU/Linux 13 (trixie)  6.12.20-v8+ aarch64  up 19d 0h06m\nnet ?\nprocs 0  load ?\nram ?\ndisks ?\n"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn output_error_exits_2_with_message() {
    let dir = scratch("output-missing");
    let target = dir.join("no-such-dir/motd");
    let out = run(&[
        "--sysroot",
        &fixture("minimal"),
        "--output",
        target.to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    let err = String::from_utf8(out.stderr).unwrap();
    assert!(
        err.starts_with(&format!("motd-rs: {}: ", target.display())),
        "{err}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn invalid_option_value_exits_1() {
    let out = run(&["--deadline", "0"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
    assert!(
        String::from_utf8(out.stderr)
            .unwrap()
            .contains("invalid value '0' for --deadline")
    );
}

#[test]
fn renders_captured_machines() {
    let out = run(&["--sysroot", &fixture("rpi4-ubuntu")]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "rpi4-ubuntu  Ubuntu 24.04.4 LTS  6.8.0-1057-raspi aarch64  up 158d 17h47m  Wed 25 Nov 14:01 CET\nnet eth0 192.0.2.1\nusers 1 (2 sessions)\nprocs 98  load 1.56 2.67 3.05 /4\nram 55% 4.2G/7.6G  swap none  cache 3.1G  buffers 244M\ndisks / 23% 47G/211G  /boot/firmware 38% 189M/504M\n"
    );
    let out = run(&["--sysroot", &fixture("container")]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "container  Debian GNU/Linux 11 (bullseye)  6.8.0-149-generic x86_64  up 18h24m  Wed 25 Nov 12:35 UTC\nnet eth0 192.0.2.5\nprocs 1  load 5.60 4.84 4.08 /20\nram 54% 12G/23G  swap 18% 6.9G/40G  cache 9.3G  buffers 1.8G\ndisks / 81% 16G/20G\n"
    );
}

#[test]
fn watched_ports_line() {
    let dir = scratch("ports");
    let config = dir.join("config.toml");
    std::fs::write(
        &config,
        "[ports]\nwatch = [\"22/tcp\", \"6443/tcp\", \"80/tcp\", \"53/udp\"]\n",
    )
    .unwrap();
    let out = run(&[
        "--sysroot",
        &fixture("rpi4-ubuntu"),
        "--config",
        config.to_str().unwrap(),
    ]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success());
    let text = String::from_utf8(out.stdout).unwrap();
    assert!(
        text.ends_with(
            "\nports 22/tcp listening  6443/tcp listening  80/tcp closed  53/udp listening\n"
        ),
        "{text}"
    );
    let ports_lines = text.lines().filter(|l| l.starts_with("ports ")).count();
    assert_eq!(ports_lines, 1, "{text}");
}

/// Runs `fixture` with `config` and returns the output lines that start
/// with `services`.
fn services_lines(fixture_name: &str, config_text: &str) -> Vec<String> {
    let dir = scratch(&format!("services-{fixture_name}"));
    let config = dir.join("config.toml");
    std::fs::write(&config, config_text).unwrap();
    let out = run(&[
        "--sysroot",
        &fixture(fixture_name),
        "--config",
        config.to_str().unwrap(),
    ]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stderr), "");
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("services"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn watched_services_line() {
    // `minimal` has a fake systemd that knows ssh.service, and no process.
    assert_eq!(
        services_lines(
            "minimal",
            "[services]\nwatch = [\"ssh\", \"nope.timer\", \"process:sshd\"]\n"
        ),
        ["services ssh running  nope.timer not-found  sshd inactive"]
    );
    // Unit states captured on `rpi4-ubuntu`: apport is masked, auditd has no
    // unit file, nginx was trimmed from the capture.
    assert_eq!(
        services_lines(
            "rpi4-ubuntu",
            "[services]\nwatch = [\"ssh\", \"k3s\", \"ufw\", \"apt-daily\", \"apport\", \"auditd\", \"process:sshd\", \"process:nginx\"]\n"
        ),
        [
            "services ssh running  k3s running  ufw exited  apt-daily inactive  apport inactive  auditd not-found  sshd running  nginx inactive"
        ]
    );
    // No systemd in the container: units are unknown, processes are still
    // found in /proc.
    assert_eq!(
        services_lines(
            "container",
            "[services]\nwatch = [\"ssh\", \"process:sleep\"]\n"
        ),
        ["services ssh ?  sleep running"]
    );
    // Nothing watched (the default): no line.
    assert_eq!(services_lines("minimal", ""), Vec::<String>::new());
}
