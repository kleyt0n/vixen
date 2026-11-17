//! Updates: check GitHub for a newer release every few hours, tell the user once per version,
//! and install it in place from the same assets and checksums `scripts/install.sh` uses, then
//! relaunch.

use crate::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::process::Command;

const REPO: &str = "kleyt0n/vixen";
/// How long after launch the first check runs, how often the checker wakes, and how old the
/// last check must be for it to look again. Wall-clock based, so time asleep counts.
const FIRST_CHECK: Duration = Duration::from_secs(20);
const TICK: Duration = Duration::from_secs(10 * 60);
const CHECK_EVERY_S: i64 = 6 * 3600;

type Res<T> = Result<T, String>;

/// What Settings → About and the update banner show.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Status {
    /// This build's version, e.g. `0.4.1`.
    pub current: String,
    /// Newest published release, once checked.
    pub latest: Option<String>,
    /// `latest` is newer than `current`.
    pub available: bool,
    /// Release page (notes and downloads).
    pub url: Option<String>,
    /// Unix seconds of the last successful check.
    pub checked_at: Option<i64>,
    /// Why the last check failed.
    pub error: Option<String>,
    /// Vixen can replace itself: it runs from a Vixen.app in a folder it can write to.
    /// Otherwise the UI links to the release page instead.
    pub can_install: bool,
    pub checking: bool,
    pub installing: bool,
}

/// Fill in what's known before any check, and start the background checker.
pub fn init(app: &AppHandle) {
    {
        let state = app.state::<AppState>();
        let mut s = state.update.lock().unwrap();
        s.current = app.package_info().version.to_string();
        s.can_install = bundle().is_some_and(|b| b.parent().is_some_and(writable));
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        let mut tried_at = 0;
        loop {
            let auto = app.state::<AppState>().prefs.lock().unwrap().check_updates;
            let t = now();
            if auto && t - tried_at >= CHECK_EVERY_S {
                tried_at = t;
                let s = check(&app).await;
                if s.available {
                    announce(&app, &s);
                }
            }
            tokio::time::sleep(TICK).await;
        }
    });
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

fn set(app: &AppHandle, f: impl FnOnce(&mut Status)) -> Status {
    let s = {
        let state = app.state::<AppState>();
        let mut s = state.update.lock().unwrap();
        f(&mut s);
        s.clone()
    };
    let _ = app.emit("update", &s);
    s
}

/// Ask GitHub for the latest release and record the result.
pub async fn check(app: &AppHandle) -> Status {
    set(app, |s| s.checking = true);
    let res = latest().await;
    set(app, |s| {
        s.checking = false;
        match res {
            Ok((version, url)) => {
                s.available = newer(&version, &s.current);
                s.latest = Some(version);
                s.url = Some(url);
                s.checked_at = Some(now());
                s.error = None;
            }
            Err(e) => s.error = Some(e),
        }
    })
}

/// A macOS notification, once per new version.
fn announce(app: &AppHandle, s: &Status) {
    let Some(v) = &s.latest else { return };
    let state = app.state::<AppState>();
    let prefs = {
        let mut p = state.prefs.lock().unwrap();
        if p.update_notified.as_deref() == Some(v) {
            return;
        }
        p.update_notified = Some(v.clone());
        p.clone()
    };
    crate::prefs::save(app, &prefs);
    let _ = app
        .notification()
        .builder()
        .title(format!("Vixen {v} is available"))
        .body(format!("You have {}. Update from Settings → About.", s.current))
        .show();
}

/// `(version, release page)` of the newest published release.
async fn latest() -> Res<(String, String)> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = curl(&["-m", "15", "-H", "Accept: application/vnd.github+json", &url]).await?;
    let v: serde_json::Value = serde_json::from_slice(&body).map_err(|_| "GitHub sent an unexpected reply")?;
    let tag = v.get("tag_name").and_then(|t| t.as_str()).ok_or("GitHub sent an unexpected reply")?;
    let page = v.get("html_url").and_then(|t| t.as_str()).unwrap_or_default();
    let page = if page.starts_with("https://") {
        page.to_string()
    } else {
        format!("https://github.com/{REPO}/releases/tag/{tag}")
    };
    Ok((tag.trim_start_matches('v').to_string(), page))
}

/// `major.minor.patch`, ignoring a leading `v` and any pre-release or build suffix.
fn semver(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.').map(|p| p.parse::<u64>());
    let major = it.next()?.ok()?;
    let minor = it.next().unwrap_or(Ok(0)).ok()?;
    let patch = it.next().unwrap_or(Ok(0)).ok()?;
    Some((major, minor, patch))
}

fn newer(latest: &str, current: &str) -> bool {
    matches!((semver(latest), semver(current)), (Some(a), Some(b)) if a > b)
}

/// The Vixen.app this process runs from, if any (not when run with `cargo run`).
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".vixen-write-test-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

async fn curl(args: &[&str]) -> Res<Vec<u8>> {
    let out = Command::new("curl")
        .args(["-fsSL", "--retry", "2"])
        .args(args)
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(match out.status.code() {
            Some(6 | 7) => "couldn't reach GitHub (offline?)".into(),
            Some(28) => "GitHub took too long to answer".into(),
            _ => format!("download failed: {}", err.trim().trim_start_matches("curl: ")),
        });
    }
    Ok(out.stdout)
}

async fn run(cmd: &str, args: &[&std::ffi::OsStr]) -> Res<Vec<u8>> {
    let out = Command::new(cmd).args(args).output().await.map_err(|e| format!("couldn't run {cmd}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

/// Download `<base>/<asset>` into `dir` and check it against `SHA256SUMS`.
async fn fetch(base: &str, asset: &str, sums: &str, dir: &Path) -> Res<PathBuf> {
    let want = sums
        .lines()
        .filter_map(|l| l.split_once(char::is_whitespace))
        .find(|(_, f)| f.trim().trim_start_matches('*') == asset)
        .map(|(h, _)| h.to_ascii_lowercase())
        .ok_or_else(|| format!("{asset} isn't in the release's checksums"))?;
    let path = dir.join(asset);
    curl(&["-m", "600", "-o", &path.to_string_lossy(), &format!("{base}/{asset}")]).await?;
    let out = run("shasum", &["-a".as_ref(), "256".as_ref(), path.as_os_str()]).await?;
    let got = String::from_utf8_lossy(&out).split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    if got != want {
        return Err(format!("checksum mismatch for {asset}"));
    }
    Ok(path)
}

/// Download, verify and install the latest release over this Vixen.app, then relaunch.
pub async fn install(app: &AppHandle) -> Res<()> {
    let version = {
        let state = app.state::<AppState>();
        let s = state.update.lock().unwrap();
        if s.installing {
            return Err("already updating".into());
        }
        s.latest.clone().filter(|_| s.available).ok_or("no update available")?
    };
    set(app, |s| s.installing = true);
    match replace(&version).await {
        Ok(bundle) => {
            relaunch(&bundle);
            app.exit(0);
            Ok(())
        }
        Err(e) => {
            set(app, |s| s.installing = false);
            Err(e)
        }
    }
}

async fn replace(version: &str) -> Res<PathBuf> {
    let bundle = bundle().ok_or("Vixen isn't running from Vixen.app; download the release instead")?;
    let parent = bundle.parent().ok_or("Vixen.app has no parent folder")?.to_path_buf();
    if !writable(&parent) {
        return Err(format!("can't write to {}; download the release instead", parent.display()));
    }
    let arch = match std::env::consts::ARCH {
        a @ ("aarch64" | "x86_64") => a,
        a => return Err(format!("no Vixen build for {a}")),
    };
    // Stage beside the app so swapping it in is a rename on the same volume.
    let stage = parent.join(format!(".vixen-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).map_err(|e| format!("couldn't create {}: {e}", stage.display()))?;
    let res = stage_and_swap(version, arch, &bundle, &stage).await;
    let _ = std::fs::remove_dir_all(&stage);
    res.map(|()| bundle)
}

async fn stage_and_swap(version: &str, arch: &str, bundle: &Path, stage: &Path) -> Res<()> {
    let base = format!("https://github.com/{REPO}/releases/download/v{version}");
    let sums = curl(&["-m", "30", &format!("{base}/SHA256SUMS")]).await?;
    let sums = String::from_utf8_lossy(&sums).into_owned();

    let tarball = fetch(&base, &format!("Vixen-{arch}.app.tar.gz"), &sums, stage).await?;
    run("tar", &["-xzf".as_ref(), tarball.as_os_str(), "-C".as_ref(), stage.as_os_str()]).await?;
    let new = stage.join("Vixen.app");
    if !new.join("Contents/MacOS").is_dir() {
        return Err("the download didn't contain Vixen.app".into());
    }
    let _ = run("xattr", &["-dr".as_ref(), "com.apple.quarantine".as_ref(), new.as_os_str()]).await;

    // The CLI too, when it's where install.sh puts it. Best effort: the app is what was asked for.
    if let Err(e) = update_cli(&base, arch, &sums, stage).await {
        eprintln!("vixen: CLI not updated: {e}");
    }

    let old = stage.join("old.app");
    std::fs::rename(bundle, &old).map_err(|e| format!("couldn't move the old app aside: {e}"))?;
    if let Err(e) = std::fs::rename(&new, bundle) {
        let _ = std::fs::rename(&old, bundle);
        return Err(format!("couldn't put the new app in place: {e}"));
    }
    Ok(())
}

async fn update_cli(base: &str, arch: &str, sums: &str, stage: &Path) -> Res<()> {
    let Some(home) = std::env::var_os("HOME") else { return Ok(()) };
    let dest = Path::new(&home).join(".local/bin/vixen");
    if !dest.is_file() {
        return Ok(());
    }
    let dir = stage.join("cli");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let tarball = fetch(base, &format!("vixen-{arch}-apple-darwin.tar.gz"), sums, &dir).await?;
    run("tar", &["-xzf".as_ref(), tarball.as_os_str(), "-C".as_ref(), dir.as_os_str()]).await?;
    // Copy next to the old binary, then rename over it, so a half-written file is never `vixen`.
    let tmp = dest.with_file_name(".vixen.new");
    std::fs::copy(dir.join("vixen"), &tmp).map_err(|e| e.to_string())?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    std::fs::rename(&tmp, &dest).map_err(|e| e.to_string())
}

/// Open the (new) app once this process has exited.
fn relaunch(bundle: &Path) {
    let _ = std::process::Command::new("/bin/sh")
        .args(["-c", r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; open "$2""#, "sh"])
        .arg(std::process::id().to_string())
        .arg(bundle)
        .spawn();
}

#[tauri::command]
pub fn update_status(app: AppHandle) -> Status {
    app.state::<AppState>().update.lock().unwrap().clone()
}

#[tauri::command]
pub async fn check_update(app: AppHandle) -> Status {
    check(&app).await
}

#[tauri::command]
pub async fn install_update(app: AppHandle) -> Res<()> {
    install(&app).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions() {
        assert!(newer("0.4.2", "0.4.1"));
        assert!(newer("v0.5.0", "0.4.9"));
        assert!(newer("1.0", "0.9.9"));
        assert!(!newer("0.4.1", "0.4.1"));
        assert!(!newer("0.4.0", "0.4.1"));
        assert!(!newer("0.4.2-beta.1", "0.4.2"));
        assert!(!newer("garbage", "0.4.1"));
    }

    /// Talks to GitHub: `cargo test -p vixen-app -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn downloads_and_verifies_latest_release() {
        let (version, page) = latest().await.unwrap();
        assert!(semver(&version).is_some(), "{version}");
        assert!(page.starts_with("https://github.com/"), "{page}");
        let base = format!("https://github.com/{REPO}/releases/download/v{version}");
        let sums = String::from_utf8(curl(&["-m", "30", &format!("{base}/SHA256SUMS")]).await.unwrap()).unwrap();
        let dir = std::env::temp_dir().join(format!("vixen-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tarball = fetch(&base, "Vixen-aarch64.app.tar.gz", &sums, &dir).await.unwrap();
        run("tar", &["-xzf".as_ref(), tarball.as_os_str(), "-C".as_ref(), dir.as_os_str()]).await.unwrap();
        assert!(dir.join("Vixen.app/Contents/MacOS").is_dir());
        assert!(fetch(&base, "nope.tar.gz", &sums, &dir).await.is_err());

        // Swap a stand-in app and CLI (under a scratch HOME) for the release's.
        let old_app = dir.join("apps/Vixen.app");
        std::fs::create_dir_all(&old_app).unwrap();
        std::fs::write(old_app.join("old-marker"), b"").unwrap();
        let bin = dir.join("home/.local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("vixen"), b"old").unwrap();
        // SAFETY: the only test that reads HOME, and it runs alone (--ignored).
        unsafe { std::env::set_var("HOME", dir.join("home")) };
        let stage = dir.join("apps/.stage");
        std::fs::create_dir_all(&stage).unwrap();
        stage_and_swap(&version, "aarch64", &old_app, &stage).await.unwrap();
        assert!(old_app.join("Contents/MacOS/vixen-app").is_file());
        assert!(!old_app.join("old-marker").exists());
        assert!(std::fs::metadata(bin.join("vixen")).unwrap().len() > 1000);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
