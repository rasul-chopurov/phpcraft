//! Updates and What's New: releases come from GitHub. At every start PhpCraft looks for a newer
//! release (alpha builds included) and offers it — Update and Restart (in place, see
//! `self_update`), Download, Remind Me Later or Skip This Version. The first run after an update shows the changelog; Help ▸ What's New shows it any
//! time.

use std::time::Duration;

use gpui::{
    App, AppContext, Context, FontWeight, SharedString, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    prelude::*, px,
};

use crate::config::Settings;
use crate::theme;

actions!(phpcraft, [CheckForUpdates, ShowChangelog]);

const RELEASES_API: &str = "https://api.github.com/repos/rasul-chopurov/phpcraft/releases?per_page=20";
pub const RELEASES_PAGE: &str = "https://github.com/rasul-chopurov/phpcraft/releases";

#[derive(Debug, Clone)]
pub struct Release {
    /// `0.0.3-alpha` (the tag without `v`).
    pub version: String,
    pub date: String,
    pub notes: String,
    pub page: String,
    pub assets: Vec<(String, String)>,
}

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// `1.2.3-beta.4` → comparable key; releases sort before their pre-releases' successors.
fn version_key(version: &str) -> (u64, u64, u64, u8, u64) {
    let version = version.trim().trim_start_matches('v');
    let (core, pre) = version.split_once('-').unwrap_or((version, ""));
    let mut nums = core.split('.').map(|n| n.parse::<u64>().unwrap_or(0));
    let (a, b, c) = (nums.next().unwrap_or(0), nums.next().unwrap_or(0), nums.next().unwrap_or(0));
    let pre = pre.to_ascii_lowercase();
    let rank = if pre.is_empty() {
        3
    } else if pre.starts_with("rc") {
        2
    } else if pre.starts_with("beta") {
        1
    } else {
        0
    };
    let pre_num = pre.rsplit(|ch: char| !ch.is_ascii_digit()).next().and_then(|n| n.parse().ok()).unwrap_or(0);
    (a, b, c, rank, pre_num)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    version_key(candidate) > version_key(current)
}

/// Releases, newest first (blocking).
pub fn fetch() -> Result<Vec<Release>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).build().into();
    let text = agent
        .get(RELEASES_API)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "PhpCraft")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut releases: Vec<Release> = json
        .as_array()
        .ok_or("unexpected response")?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .map(|r| Release {
            version: r["tag_name"].as_str().unwrap_or("").trim_start_matches('v').to_string(),
            date: r["published_at"].as_str().unwrap_or("").chars().take(10).collect(),
            notes: r["body"].as_str().unwrap_or("").to_string(),
            page: r["html_url"].as_str().unwrap_or(RELEASES_PAGE).to_string(),
            assets: r["assets"]
                .as_array()
                .map(|a| a.iter().map(|x| (x["name"].as_str().unwrap_or("").to_string(), x["browser_download_url"].as_str().unwrap_or("").to_string())).collect())
                .unwrap_or_default(),
        })
        .filter(|r| !r.version.is_empty())
        .collect();
    releases.sort_by(|a, b| version_key(&b.version).cmp(&version_key(&a.version)));
    Ok(releases)
}

/// The download for this computer: the installer for the OS and CPU, else the release page.
pub fn download_url(release: &Release) -> String {
    let arch_mac = if cfg!(target_arch = "aarch64") { "macos-arm64.dmg" } else { "macos-x64.dmg" };
    let arch_win = if cfg!(target_pointer_width = "64") { "windows-x64-Setup.exe" } else { "windows-x86-Setup.exe" };
    let arch_linux = if cfg!(target_arch = "aarch64") { "linux-arm64.tar.gz" } else { "linux-x86_64.tar.gz" };
    let wanted = if cfg!(target_os = "macos") {
        arch_mac
    } else if cfg!(windows) {
        arch_win
    } else {
        arch_linux
    };
    release.assets.iter().find(|(name, _)| name.ends_with(wanted)).map(|(_, url)| url.clone()).unwrap_or_else(|| release.page.clone())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A newer release is available.
    Update,
    /// What's New (after an update, or from the Help menu).
    Changelog,
}

pub struct UpdateWindow {
    mode: Mode,
    releases: Vec<Release>,
    error: Option<String>,
    /// Update and Restart in progress (download bytes), and its failure if any.
    installing: Option<std::sync::Arc<crate::self_update::Progress>>,
    install_error: Option<String>,
    _install: Option<gpui::Task<()>>,
    _tick: Option<gpui::Task<()>>,
}

fn open_window(mode: Mode, releases: Vec<Release>, error: Option<String>, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(crate::window_state::centered(680., 560., cx))),
        window_min_size: Some(crate::window_state::min_size()),
        titlebar: Some(TitlebarOptions {
            title: Some(if mode == Mode::Update { "Update available" } else { "What's New in PhpCraft" }.into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    let _ = cx.open_window(options, |_, cx| cx.new(|_| UpdateWindow { mode, releases, error, installing: None, install_error: None, _install: None, _tick: None }));
}

/// The start-up check: offer a newer release, or show What's New once after an update.
pub fn check_at_startup(cx: &mut App) {
    let settings = Settings::load();
    let first_run_of_version = !settings.last_seen_version.is_empty() && settings.last_seen_version != current_version();
    let mut seen = settings.clone();
    seen.last_seen_version = current_version().to_string();
    seen.save();
    if !settings.check_for_updates && !first_run_of_version {
        return;
    }
    cx.spawn(async move |cx| {
        let Ok(releases) = cx.background_spawn(async { fetch() }).await else { return };
        let _ = cx.update(|cx| {
            let latest = releases.first().cloned();
            if settings.check_for_updates
                && let Some(latest) = latest
                && is_newer(&latest.version, current_version())
                && settings.skipped_version != latest.version
            {
                let mut settings = Settings::load();
                settings.last_update_check = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                settings.save();
                open_window(Mode::Update, releases, None, cx);
            } else if first_run_of_version {
                open_window(Mode::Changelog, releases, None, cx);
            }
        });
    })
    .detach();
}

/// Help ▸ Check for Updates… / What's New: always shows a window, even when up to date.
fn check_now(mode: Mode, cx: &mut App) {
    cx.spawn(async move |cx| {
        let result = cx.background_spawn(async { fetch() }).await;
        let _ = cx.update(|cx| match result {
            Ok(releases) => {
                let newer = releases.first().is_some_and(|r| is_newer(&r.version, current_version()));
                open_window(if mode == Mode::Update && newer { Mode::Update } else { Mode::Changelog }, releases, None, cx)
            }
            Err(err) => open_window(Mode::Changelog, Vec::new(), Some(err), cx),
        });
    })
    .detach();
}

/// Help ▸ What's New.
pub fn show_changelog(cx: &mut App) {
    check_now(Mode::Changelog, cx);
}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &CheckForUpdates, cx| check_now(Mode::Update, cx));
    cx.on_action(|_: &ShowChangelog, cx| show_changelog(cx));
}

impl UpdateWindow {
    fn latest(&self) -> Option<&Release> {
        self.releases.first()
    }

    /// Downloads the latest release, puts it in place and restarts into it.
    fn update_and_restart(&mut self, cx: &mut Context<Self>) {
        let Some(release) = self.latest().cloned() else { return };
        let progress = std::sync::Arc::new(crate::self_update::Progress::default());
        self.installing = Some(progress.clone());
        self.install_error = None;
        self._install = Some(cx.spawn(async move |this, cx| {
            let task = cx.background_spawn(async move { crate::self_update::install(&release, progress) });
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(restart) => crate::self_update::restart(restart, cx),
                Err(err) => {
                    this.installing = None;
                    this.install_error = Some(err);
                    cx.notify();
                }
            });
        }));
        // Repaint the progress line while the download runs.
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(200)).await;
                let running = this.update(cx, |this, cx| {
                    cx.notify();
                    this.installing.is_some()
                });
                if !running.unwrap_or(false) {
                    break;
                }
            }
        }));
        cx.notify();
    }

    fn progress_text(&self) -> Option<String> {
        let progress = self.installing.as_ref()?;
        let done = progress.done.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1_048_576.;
        let total = progress.total.load(std::sync::atomic::Ordering::Relaxed) as f64 / 1_048_576.;
        Some(if total > 0. && done >= total {
            "Installing…".to_string()
        } else if total > 0. {
            format!("Downloading… {done:.1} of {total:.1} MB")
        } else {
            format!("Downloading… {done:.1} MB")
        })
    }
}


impl Render for UpdateWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current = current_version();
        let latest = self.latest().cloned();
        let newer = latest.as_ref().is_some_and(|r| is_newer(&r.version, current));
        let header = match (self.mode, &latest) {
            (Mode::Update, Some(latest)) if newer => format!("PhpCraft {} is available — you have {current}", latest.version),
            _ if self.error.is_some() => "Couldn't reach GitHub".to_string(),
            _ if newer => format!("What's New · {} is available", latest.as_ref().map(|r| r.version.as_str()).unwrap_or("")),
            _ => format!("What's New · you're up to date ({current})"),
        };
        let button = |id: &'static str, label: &'static str, primary: bool| crate::dialogs::button(id, label, primary);
        let in_place = crate::self_update::supported();
        let status = self.progress_text().or_else(|| self.install_error.clone());
        let failed = self.install_error.is_some() && self.installing.is_none();
        let actions = div()
            .flex()
            .items_center()
            .justify_end()
            .gap_2()
            .px_5()
            .py_3()
            .border_t_1()
            .border_color(theme::border())
            .child(div().flex_1().overflow_hidden().text_size(px(12.)).text_color(if failed { theme::error() } else { theme::muted() }).children(status))
            .when(newer, |d| {
                let release = latest.clone();
                let skip = latest.as_ref().map(|r| r.version.clone()).unwrap_or_default();
                d.child(button("update-skip", "Skip This Version", false).on_click(cx.listener(move |_, _, window, _| {
                    let mut settings = Settings::load();
                    settings.skipped_version = skip.clone();
                    settings.save();
                    window.remove_window();
                })))
                .child(button("update-later", "Remind Me Later", false).on_click(cx.listener(|_, _, window, _| window.remove_window())))
                .child(button("update-download", "Download", !in_place).on_click(cx.listener(move |_, _, window, cx| {
                    if let Some(release) = &release {
                        cx.open_url(&download_url(release));
                    }
                    window.remove_window();
                })))
                .when(in_place, |d| {
                    d.child(button("update-install", "Update and Restart", true).on_click(cx.listener(|this, _, _, cx| {
                        if this.installing.is_none() {
                            this.update_and_restart(cx);
                        }
                    })))
                })
            })
            .when(!newer, |d| {
                d.child(button("changelog-releases", "All Releases", false).on_click(cx.listener(|_, _, _, cx| cx.open_url(RELEASES_PAGE))))
                    .child(button("changelog-close", "Close", true).on_click(cx.listener(|_, _, window, _| window.remove_window())))
            });
        let entries = self.releases.iter().take(10).map(|release| {
            let is_current = release.version == current;
            let notes = if release.notes.trim().is_empty() { "No notes for this release.".to_string() } else { release.notes.clone() };
            div()
                .w_full()
                .flex()
                .flex_col()
                .gap_2()
                .pb_4()
                .mb_4()
                .border_b_1()
                .border_color(theme::border())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().text_size(px(16.)).font_weight(FontWeight::BOLD).text_color(theme::ui_text()).child(release.version.clone()))
                        .when(is_current, |d| d.child(div().px_1p5().rounded_sm().bg(theme::selection()).text_size(px(11.)).child("installed")))
                        .child(div().text_size(px(12.)).text_color(theme::muted()).child(release.date.clone())),
                )
                .child(crate::markdown_preview::render_inline(&crate::markdown_preview::parse(&notes)))
        });
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::editor_bg())
            .text_size(px(13.))
            .text_color(theme::text())
            .child(
                div()
                    .px_5()
                    .py_4()
                    .border_b_1()
                    .border_color(theme::border())
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(crate::dialogs::avatar("PhpCraft", 32.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::ui_text()).child(SharedString::from(header)))
                            .child(div().text_size(px(12.)).text_color(theme::muted()).child("Releases from github.com/rasul-chopurov/phpcraft")),
                    ),
            )
            .child(
                div()
                    .id("changelog")
                    .flex_1()
                    .w_full()
                    .min_h(px(0.))
                    .overflow_y_scroll()
                    .px_5()
                    .py_4()
                    .children(self.error.clone().map(|e| div().text_color(theme::error()).child(e)))
                    .children(entries),
            )
            .child(actions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(is_newer("0.0.3-alpha", "0.0.2-alpha"));
        assert!(is_newer("v0.1.0", "0.0.9"));
        assert!(is_newer("0.1.0", "0.1.0-rc.1"));
        assert!(is_newer("0.1.0-beta", "0.1.0-alpha"));
        assert!(is_newer("0.1.0-alpha.2", "0.1.0-alpha.1"));
        assert!(!is_newer("0.0.2-alpha", "0.0.2-alpha"));
        assert!(!is_newer("0.0.1-alpha", "0.0.2-alpha"));
    }
}
