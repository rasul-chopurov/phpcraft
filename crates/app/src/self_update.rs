//! Update in place: downloads the new release for this computer, swaps it in and restarts.
//! macOS replaces `PhpCraft.app` from the release's `.zip`; Windows replaces `phpcraft.exe`
//! from the portable `.zip` when its folder is writable (portable or per-user installs), and
//! otherwise runs the new `Setup.exe` silently (it asks for admin rights and starts PhpCraft
//! again when done).

#![cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code, unused_imports))]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::updates::Release;

/// Bytes downloaded so far and the total (0 while unknown), for the progress line.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
}

/// What to do once the new version is in place.
pub enum Restart {
    /// Start this program (the replaced `.app` or `.exe`) after PhpCraft quits.
    Relaunch(PathBuf),
    /// The installer is running; it starts PhpCraft itself.
    #[cfg_attr(not(windows), allow(dead_code))]
    Installer,
}

/// The `.app` bundle the running binary belongs to.
#[cfg(target_os = "macos")]
fn bundle() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().find(|p| p.extension().is_some_and(|e| e == "app")).map(Path::to_path_buf)
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(".phpcraft-write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Whether this build can update itself (a dev build run from `target/` can't).
pub fn supported() -> bool {
    #[cfg(target_os = "macos")]
    return bundle().is_some();
    #[cfg(windows)]
    return true;
    #[allow(unreachable_code)]
    false
}

fn asset<'a>(release: &'a Release, suffix: &str) -> Result<&'a str, String> {
    release.assets.iter().find(|(name, _)| name.ends_with(suffix)).map(|(_, url)| url.as_str()).ok_or_else(|| format!("This release has no {suffix} download."))
}

fn download(url: &str, to: &Path, progress: &Progress) -> Result<(), String> {
    use std::io::{Read, Write};
    let response = ureq::get(url).header("User-Agent", "PhpCraft").call().map_err(|e| format!("download: {e}"))?;
    let (_, body) = response.into_parts();
    progress.total.store(body.content_length().unwrap_or(0), Ordering::Relaxed);
    let mut reader = body.into_reader();
    let mut file = std::fs::File::create(to).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.flush().map_err(|e| e.to_string())
}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> Result<(), String> {
    let output = process::command(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if output.status.success() { Ok(()) } else { Err(format!("{program}: {}", String::from_utf8_lossy(&output.stderr).trim())) }
}

/// A fresh folder for this update's downloads.
fn work_dir(version: &str) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("phpcraft-update-{version}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir)
}

/// Downloads `release` and puts it in place of the running version (blocking).
pub fn install(release: &Release, progress: Arc<Progress>) -> Result<Restart, String> {
    let work = work_dir(&release.version)?;
    #[cfg(target_os = "macos")]
    {
        let bundle = bundle().ok_or("PhpCraft isn't running from PhpCraft.app.")?;
        let parent = bundle.parent().ok_or("no parent folder")?.to_path_buf();
        if !writable(&parent) {
            return Err(format!("{} isn't writable: download the update instead.", parent.display()));
        }
        let arch = if cfg!(target_arch = "aarch64") { "macos-arm64.zip" } else { "macos-x64.zip" };
        let zip = work.join("PhpCraft.zip");
        download(asset(release, arch)?, &zip, &progress)?;
        let unpacked = work.join("unpacked");
        run("ditto", &["-x".as_ref(), "-k".as_ref(), zip.as_os_str(), unpacked.as_os_str()])?;
        let new_app = unpacked.join("PhpCraft.app");
        if !new_app.join("Contents/MacOS").is_dir() {
            return Err("The download doesn't contain PhpCraft.app.".into());
        }
        // Copy next to the old bundle first (same volume), then two renames swap them: the
        // running process keeps its open files, and a failure leaves the old app untouched.
        let staged = parent.join(".PhpCraft-update.app");
        let old = parent.join(".PhpCraft-old.app");
        let _ = std::fs::remove_dir_all(&staged);
        let _ = std::fs::remove_dir_all(&old);
        run("ditto", &[new_app.as_os_str(), staged.as_os_str()])?;
        std::fs::rename(&bundle, &old).map_err(|e| format!("replace {}: {e}", bundle.display()))?;
        if let Err(e) = std::fs::rename(&staged, &bundle) {
            let _ = std::fs::rename(&old, &bundle);
            return Err(format!("replace {}: {e}", bundle.display()));
        }
        let _ = std::fs::remove_dir_all(&work);
        Ok(Restart::Relaunch(bundle))
    }
    #[cfg(windows)]
    {
        let exe = dunce::canonicalize(std::env::current_exe().map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let dir = exe.parent().ok_or("no parent folder")?.to_path_buf();
        let arch = if cfg!(target_pointer_width = "64") { "x64" } else { "x86" };
        if writable(&dir) {
            let zip = work.join("PhpCraft.zip");
            download(asset(release, &format!("windows-{arch}-portable.zip"))?, &zip, &progress)?;
            // Windows 10+ ships bsdtar, which reads zip files.
            run("tar", &["-xf".as_ref(), zip.as_os_str(), "-C".as_ref(), work.as_os_str()])?;
            let new_exe = work.join("phpcraft.exe");
            if !new_exe.is_file() {
                return Err("The download doesn't contain phpcraft.exe.".into());
            }
            // A running exe can't be overwritten but can be renamed; the old one is deleted at
            // the next start (`clean_up`).
            let old = exe.with_file_name("phpcraft.old.exe");
            let _ = std::fs::remove_file(&old);
            std::fs::rename(&exe, &old).map_err(|e| format!("replace {}: {e}", exe.display()))?;
            if let Err(e) = std::fs::copy(&new_exe, &exe) {
                let _ = std::fs::rename(&old, &exe);
                return Err(format!("replace {}: {e}", exe.display()));
            }
            let _ = std::fs::remove_dir_all(&work);
            Ok(Restart::Relaunch(exe))
        } else {
            let setup = work.join("PhpCraft-Setup.exe");
            download(asset(release, &format!("windows-{arch}-Setup.exe"))?, &setup, &progress)?;
            process::command(&setup)
                .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"])
                .spawn()
                .map_err(|e| format!("installer: {e}"))?;
            Ok(Restart::Installer)
        }
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (work, progress);
        Err("Updating in place isn't available on this system: download the update instead.".into())
    }
}

/// Starts the new version once this process has exited, then quits.
pub fn restart(restart: Restart, cx: &mut gpui::App) {
    if let Restart::Relaunch(target) = restart {
        #[cfg(target_os = "macos")]
        {
            let script = format!(
                "while kill -0 {} 2>/dev/null; do sleep 0.2; done; rm -rf \"$1\"; open \"$2\"",
                std::process::id()
            );
            let old = target.parent().map(|p| p.join(".PhpCraft-old.app")).unwrap_or_default();
            let _ = process::command("/bin/sh").arg("-c").arg(script).arg("sh").arg(old).arg(&target).spawn();
        }
        #[cfg(windows)]
        {
            // A second instance would start while this one still holds the windows: the new
            // exe waits for the old process itself (`wait_for_parent`).
            let _ = process::command(&target).env("PHPCRAFT_WAIT_PID", std::process::id().to_string()).spawn();
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        let _ = target;
    }
    cx.quit();
}

/// At start-up: wait for the version being replaced to exit, and remove its leftovers.
pub fn clean_up() {
    #[cfg(windows)]
    {
        if let Some(pid) = std::env::var("PHPCRAFT_WAIT_PID").ok().and_then(|p| p.parse::<u32>().ok()) {
            // SAFETY: called first in main, before any other thread starts.
            unsafe { std::env::remove_var("PHPCRAFT_WAIT_PID") };
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline && process_alive(pid) {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::fs::remove_file(exe.with_file_name("phpcraft.old.exe"));
        }
    }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    process::command("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
}
