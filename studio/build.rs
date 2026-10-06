//! Build information for « À propos de Laser Studio » (about.rs), with no
//! dependency and no network:
//! - `LASER_STUDIO_BUILD_DATE`: the UTC day of the build, `YYYY-MM-DD`,
//!   from `SOURCE_DATE_EPOCH` when it is set (reproducible builds).
//! - `LASER_STUDIO_GIT_COMMIT`: the short commit of the checkout, empty
//!   when git or the repository is not there (a source archive).

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    // Rerun when the code changes (the date follows the last build of new
    // code) and when the commit moves.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=Cargo.toml");

    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or_else(|| SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64));
    let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
    println!("cargo:rustc-env=LASER_STUDIO_BUILD_DATE={y:04}-{m:02}-{d:02}");

    let commit = git(&["rev-parse", "--short=10", "HEAD"]).filter(|c| c.chars().all(|c| c.is_ascii_hexdigit()));
    if let Some(dir) = git(&["rev-parse", "--absolute-git-dir"]) {
        let dir = PathBuf::from(dir);
        for f in ["HEAD", "logs/HEAD"] {
            if dir.join(f).exists() {
                println!("cargo:rerun-if-changed={}", dir.join(f).display());
            }
        }
    }
    println!("cargo:rustc-env=LASER_STUDIO_GIT_COMMIT={}", commit.unwrap_or_default());
}

/// A git command's trimmed output, `None` on any failure. Local only.
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).env("GIT_TERMINAL_PROMPT", "0").output().ok()?;
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (out.status.success() && !s.is_empty()).then_some(s)
}

/// Days since 1970-01-01 → (year, month, day), proleptic Gregorian
/// (Howard Hinnant's `civil_from_days`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}
