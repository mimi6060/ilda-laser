//! T-413: the macOS bundle's Info.plist, as `build_app.sh` writes it, has
//! the identifier, the crate's version and the microphone text. Only the
//! plist is rendered here (`--plist`): the full script builds in release
//! mode and is run by hand.

#![cfg(target_os = "macos")]

use std::path::{Path, PathBuf};
use std::process::Command;

fn packaging() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/macos")
}

fn extract(plist: &Path, key: &str) -> String {
    let out = Command::new("plutil").args(["-extract", key, "raw", "-o", "-"]).arg(plist).output().unwrap();
    assert!(out.status.success(), "no {key} in the plist");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

#[test]
fn the_bundle_plist_has_the_identifier_version_and_microphone_text() {
    let plist = std::env::temp_dir().join(format!("laser-studio-Info-{}.plist", std::process::id()));
    let status = Command::new(packaging().join("build_app.sh")).arg("--plist").arg(&plist).status().unwrap();
    assert!(status.success(), "build_app.sh --plist");
    assert_eq!(extract(&plist, "CFBundleIdentifier"), "org.ilda-laser.laser-studio");
    assert_eq!(extract(&plist, "CFBundleShortVersionString"), env!("CARGO_PKG_VERSION"));
    assert_eq!(extract(&plist, "CFBundleVersion"), env!("CARGO_PKG_VERSION"));
    assert_eq!(extract(&plist, "CFBundleExecutable"), env!("CARGO_PKG_NAME"), "the binary cargo builds");
    assert_eq!(extract(&plist, "LSUIElement"), "true", "a menu-bar app");
    let mic = extract(&plist, "NSMicrophoneUsageDescription");
    assert!(mic.starts_with("Laser Studio écoute l'entrée audio"), "{mic}");
    let _ = std::fs::remove_file(plist);
}

#[test]
fn the_icon_is_our_svg_and_no_generated_icon_is_in_the_tree() {
    let svg = std::fs::read_to_string(packaging().join("icon.svg")).unwrap();
    assert!(svg.contains("<svg") && svg.contains("Drawn for this project"));
    let generated: Vec<_> = std::fs::read_dir(packaging()).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "icns" || x == "png")).collect();
    assert!(generated.is_empty(), "generated icons are not committed: {generated:?}");
}
