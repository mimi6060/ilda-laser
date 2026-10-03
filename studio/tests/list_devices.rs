//! T-303: `--list-devices` prints the DACs and exits with code 0. Runs the
//! real binary with the hidden `--test-devices` list, so no scan touches
//! the network or the USB bus, and no output is ever opened.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_laser-studio");

fn list(devices: &str) -> std::process::Output {
    let data_dir = std::env::temp_dir().join(format!("laser-studio-list-devices-{}", std::process::id()));
    let out = Command::new(BIN)
        .args(["--list-devices", "--test-devices", devices, "--no-midi", "--no-audio", "--port", "0", "--data-dir"])
        .arg(&data_dir)
        .output()
        .unwrap();
    assert!(!data_dir.exists(), "listing creates no data dir: it starts nothing");
    out
}

#[test]
fn prints_each_device_and_exits_0() {
    let out = list(r#"[{"kind":"helios","id":"helios:left","name":"Helios DAC Left (1:2.3)"},{"kind":"idn","id":"idn:heliospro.local","name":"HeliosPRO"}]"#);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let text = String::from_utf8(out.stdout).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "helios:left\thelios\tHelios DAC Left (1:2.3)");
    assert_eq!(lines[1], "idn:heliospro.local\tidn\tHeliosPRO");
    assert!(text.contains("--device <id>"));
}

#[test]
fn says_so_when_nothing_is_found() {
    let out = list("[]");
    assert_eq!(out.status.code(), Some(0));
    assert!(String::from_utf8(out.stdout).unwrap().starts_with("No DAC found"));
}

#[test]
fn never_goes_with_an_output() {
    for extra in [&["--device", "auto"][..], &["--ponk"][..]] {
        let out = Command::new(BIN).arg("--list-devices").args(extra).args(["--test-devices", "[]"]).output().unwrap();
        assert_ne!(out.status.code(), Some(0), "{extra:?}");
    }
}
