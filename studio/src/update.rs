//! Automatic update at startup (application mode only).
//!
//! A build published by the Release workflow carries its version in
//! `LASER_STUDIO_RELEASE` (the `v*` tag). At startup it asks GitHub for the
//! latest release, with a short timeout. Offline, or on any problem, the
//! studio simply starts: no question, no error dialog. When a newer
//! release exists, the matching download is fetched, checked against the
//! SHA-256 digest GitHub lists for it, installed in place of this copy and
//! started; this process then exits.
//!
//! - Windows: the one-click installer runs silently and starts the studio.
//! - macOS: the `.app` bundle is replaced, then opened.
//! - Linux: the AppImage file is replaced, then started (only when running
//!   as an AppImage; a bare binary from the `.tar.gz` is left alone).
//!
//! Local builds (`cargo build`, `build_app.sh`) have no release version
//! and never update. The download goes through the system's `curl`
//! (macOS, Windows 10+, nearly every Linux desktop): no HTTP client is
//! compiled in. Nothing here touches an output: the laser is not involved
//! and the studio, once started, starts disarmed as always.

use anyhow::{bail, ensure, Context, Result};
use std::path::Path;
use std::process::Command;

const LATEST: &str = "https://api.github.com/repos/mimi6060/ilda-laser/releases/latest";
/// Offline or a slow network must not delay the start for long.
const CHECK_SECONDS: &str = "5";
const DOWNLOAD_SECONDS: &str = "600";

/// This build's release version (`v0.1.3` → `(0, 1, 3)`), if it is one.
fn current() -> Option<(u32, u32, u32)> {
    option_env!("LASER_STUDIO_RELEASE").and_then(parse_version)
}

/// `v1.2.3` or `1.2.3` → `(1, 2, 3)`. Anything else (pre-releases
/// included) is not a version we update to or from.
pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().trim_start_matches('v').split('.').map(|p| p.parse::<u32>().ok());
    let v = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(v)
}

/// The release download for this system.
fn asset_name() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some("LaserStudio-macOS.zip")
    } else if cfg!(windows) {
        Some("LaserStudio-Windows-Setup.exe")
    } else if cfg!(target_os = "linux") {
        Some("LaserStudio-Linux.AppImage")
    } else {
        None
    }
}

/// What the latest release offers for us.
#[derive(Debug, PartialEq)]
pub struct Offer {
    pub tag: String,
    pub url: String,
    pub sha256: String,
}

/// Reads GitHub's `releases/latest` reply: the newer release's asset
/// `name`, if the release is newer than `current` and lists a SHA-256
/// digest for it (no digest, no update).
pub fn offer(json: &str, current: (u32, u32, u32), name: &str) -> Option<Offer> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag = v["tag_name"].as_str()?;
    if v["draft"].as_bool() == Some(true) || v["prerelease"].as_bool() == Some(true) || parse_version(tag)? <= current {
        return None;
    }
    let asset = v["assets"].as_array()?.iter().find(|a| a["name"].as_str() == Some(name))?;
    let url = asset["browser_download_url"].as_str()?;
    let sha256 = asset["digest"].as_str()?.strip_prefix("sha256:")?;
    let ok = url.starts_with("https://github.com/mimi6060/ilda-laser/releases/download/") && sha256.len() == 64 && sha256.bytes().all(|b| b.is_ascii_hexdigit());
    ok.then(|| Offer { tag: tag.to_string(), url: url.to_string(), sha256: sha256.to_ascii_lowercase() })
}

/// The file's SHA-256, lowercase hex.
fn sha256_file(path: &Path) -> Result<String> {
    use sha2::Digest;
    let bytes = std::fs::read(path).with_context(|| format!("lecture {}", path.display()))?;
    Ok(sha2::Sha256::digest(&bytes).iter().map(|b| format!("{b:02x}")).collect())
}

fn curl(args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("curl").args(["-fsSL", "--proto", "=https"]).args(args).output().context("curl")?;
    ensure!(out.status.success(), "curl : {}", out.status);
    Ok(out.stdout)
}

/// Checks for a newer release and installs it. Returns only when the
/// studio should start as it is (up to date, offline, local build, or
/// the update failed); after a successful update this process exits.
pub fn at_startup() {
    let (Some(current), Some(name)) = (current(), asset_name()) else { return };
    if std::env::var_os("LASER_STUDIO_NO_UPDATE").is_some() {
        return;
    }
    let Ok(json) = curl(&["--max-time", CHECK_SECONDS, "-H", "Accept: application/vnd.github+json", LATEST]) else {
        return; // offline: start without a word
    };
    let Some(offer) = offer(&String::from_utf8_lossy(&json), current, name) else { return };
    eprintln!("Laser Studio : mise à jour vers {}…", offer.tag);
    match install(&offer, name) {
        Ok(()) => std::process::exit(0),
        Err(e) => eprintln!("Laser Studio : mise à jour impossible ({e:#}), démarrage de la version actuelle"),
    }
}

/// Downloads `offer` next to `dest` (same disk, so the final rename is
/// atomic) and checks its digest.
fn download(offer: &Offer, dest: &Path) -> Result<()> {
    let _ = std::fs::remove_file(dest);
    let path = dest.to_str().context("chemin de téléchargement")?;
    let got = curl(&["--max-time", DOWNLOAD_SECONDS, "-o", path, &offer.url]);
    let check = got.and_then(|_| sha256_file(dest)).and_then(|sum| {
        ensure!(sum == offer.sha256, "empreinte SHA-256 différente");
        Ok(())
    });
    if check.is_err() {
        let _ = std::fs::remove_file(dest);
    }
    check
}

#[cfg(windows)]
fn install(offer: &Offer, name: &str) -> Result<()> {
    let setup = std::env::temp_dir().join(format!("{}-{name}", offer.tag));
    download(offer, &setup)?;
    // The installer waits for nothing: this process exits right after,
    // and the installer closes any copy still running before it replaces
    // the files, then starts the new one.
    Command::new(&setup).args(["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"]).spawn().context("installateur")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn install(offer: &Offer, _name: &str) -> Result<()> {
    let exe = std::env::current_exe()?;
    let bundle = exe.ancestors().nth(3).filter(|_| crate::in_app_bundle(&exe)).context("pas lancé depuis Laser Studio.app")?.to_path_buf();
    let dir = bundle.parent().context("dossier de l'application")?;
    let work = dir.join(".Laser Studio update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).with_context(|| format!("{} n'est pas modifiable", dir.display()))?;
    let result = (|| -> Result<()> {
        let zip = work.join("update.zip");
        download(offer, &zip)?;
        let st = Command::new("/usr/bin/ditto").args(["-x", "-k"]).arg(&zip).arg(&work).status()?;
        ensure!(st.success(), "décompression : {st}");
        let new = work.join("Laser Studio.app");
        ensure!(new.join("Contents/MacOS/laser-studio").is_file(), "archive sans Laser Studio.app");
        let old = work.join("old.app");
        std::fs::rename(&bundle, &old).context("mise de côté de l'ancienne version")?;
        if let Err(e) = std::fs::rename(&new, &bundle) {
            let _ = std::fs::rename(&old, &bundle);
            bail!("remplacement : {e}");
        }
        Ok(())
    })();
    // LASER_STUDIO_UPDATE_NO_RESTART: replace only (manual test of the
    // update, without starting the studio on the real data).
    if result.is_ok() && std::env::var_os("LASER_STUDIO_UPDATE_NO_RESTART").is_none() {
        // -n: a new instance, even while this one is still exiting.
        Command::new("/usr/bin/open").arg("-n").arg(&bundle).spawn().context("ouverture de la nouvelle version")?;
    }
    let _ = std::fs::remove_dir_all(&work);
    result
}

#[cfg(target_os = "linux")]
fn install(offer: &Offer, _name: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::process::CommandExt;
    let image = std::path::PathBuf::from(std::env::var_os("APPIMAGE").context("pas lancé depuis l'AppImage")?);
    let tmp = image.with_extension("AppImage.download");
    download(offer, &tmp)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, &image).context("remplacement de l'AppImage")?;
    // Same terminal, same arguments: the new version takes over.
    let err = Command::new(&image).args(std::env::args_os().skip(1)).exec();
    bail!("démarrage de la nouvelle version : {err}")
}

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
fn install(_offer: &Offer, _name: &str) -> Result<()> {
    bail!("système non pris en charge")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUM: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn release(tag: &str, name: &str, digest: Option<&str>) -> String {
        let mut asset = serde_json::json!({ "name": name, "browser_download_url": format!("https://github.com/mimi6060/ilda-laser/releases/download/{tag}/{name}") });
        if let Some(d) = digest {
            asset["digest"] = d.into();
        }
        serde_json::json!({ "tag_name": tag, "draft": false, "prerelease": false, "assets": [asset] }).to_string()
    }

    #[test]
    fn versions_parse_with_or_without_v() {
        assert_eq!(parse_version("v0.1.2"), Some((0, 1, 2)));
        assert_eq!(parse_version("1.10.0"), Some((1, 10, 0)));
        assert_eq!(parse_version("v1.2"), None);
        assert_eq!(parse_version("v1.2.3.4"), None);
        assert_eq!(parse_version("v1.2.3-beta"), None);
        assert_eq!(parse_version(""), None);
    }

    #[test]
    fn only_a_newer_release_with_a_digest_is_offered() {
        let name = "LaserStudio-macOS.zip";
        let digest = format!("sha256:{SUM}");
        let got = offer(&release("v0.2.0", name, Some(&digest)), (0, 1, 9), name).unwrap();
        assert_eq!((got.tag.as_str(), got.sha256.as_str()), ("v0.2.0", SUM));
        assert!(got.url.ends_with("/v0.2.0/LaserStudio-macOS.zip"));
        assert_eq!(offer(&release("v0.2.0", name, Some(&digest)), (0, 2, 0), name), None, "same version");
        assert_eq!(offer(&release("v0.1.0", name, Some(&digest)), (0, 2, 0), name), None, "older");
        assert_eq!(offer(&release("v0.3.0", name, None), (0, 2, 0), name), None, "no digest");
        assert_eq!(offer(&release("v0.3.0", name, Some("md5:abc")), (0, 2, 0), name), None, "not sha256");
        assert_eq!(offer(&release("v0.3.0", "other.zip", Some(&digest)), (0, 2, 0), name), None, "no asset for us");
        assert_eq!(offer("not json", (0, 2, 0), name), None);
    }

    #[test]
    fn a_download_from_elsewhere_is_refused() {
        let name = "LaserStudio-macOS.zip";
        let json = serde_json::json!({ "tag_name": "v9.0.0", "assets": [{ "name": name, "browser_download_url": "https://example.com/x.zip", "digest": format!("sha256:{SUM}") }] }).to_string();
        assert_eq!(offer(&json, (0, 1, 0), name), None);
    }

    #[test]
    fn drafts_and_prereleases_are_ignored() {
        let name = "LaserStudio-macOS.zip";
        let mut v: serde_json::Value = serde_json::from_str(&release("v1.0.0", name, Some(&format!("sha256:{SUM}")))).unwrap();
        v["prerelease"] = true.into();
        assert_eq!(offer(&v.to_string(), (0, 1, 0), name), None);
    }

    #[test]
    fn a_local_build_never_updates() {
        // Tests are local builds: no release version is compiled in.
        if option_env!("LASER_STUDIO_RELEASE").is_none_or(|v| v.is_empty()) {
            assert_eq!(current(), None);
        }
    }
}
