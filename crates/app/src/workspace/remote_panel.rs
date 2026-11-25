//! Remote Hosts tool window: SFTP servers from
//! `.phpcraft/deployment.json`, a remote file browser, upload/download of project files and an
//! SSH session in the terminal. Transfers go through the system OpenSSH `sftp`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui::{AnyElement, AppContext, Context, Entity, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use crate::deployment::{self, Deployment, RemoteEntry, Server};
use crate::icons::{Icon, icon};
use crate::run_configs::{Kind, RunConfig};
use crate::text_field::TextField;
use crate::theme;
use crate::i18n::{tr, trf};

actions!(workspace, [
    UploadToServer,
    UploadToServerPick,
    DownloadFromServer,
    DownloadFromServerPick,
    DownloadSelected,
    RenameSelected,
    DeleteSelected,
    ToggleAutoUpload
]);

/// A file dragged out of the project tree; the remote browser uploads what it accepts.
#[derive(Clone)]
pub(super) struct DraggedPaths(pub Vec<PathBuf>);

/// A file dragged out of the remote browser; the project tree downloads what it accepts.
#[derive(Clone)]
pub(super) struct DraggedRemote {
    pub server: String,
    pub remote: String,
    pub name: String,
}

struct RemoteDrag(String);

impl gpui::Render for RemoteDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::panel_bg())
            .border_1()
            .border_color(theme::accent())
            .text_size(px(12.))
            .text_color(theme::ui_text())
            .flex()
            .items_center()
            .gap_1()
            .child(icon(Icon::globe, 12., theme::accent()))
            .child(self.0.clone())
    }
}

/// What a project file looks like while it is dragged towards the server browser.
pub(super) struct PathDrag(pub String);

impl gpui::Render for PathDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme::panel_bg())
            .border_1()
            .border_color(theme::accent())
            .text_size(px(12.))
            .text_color(theme::ui_text())
            .flex()
            .items_center()
            .gap_1()
            .child(icon(Icon::upload, 12., theme::accent()))
            .child(self.0.clone())
    }
}

/// How the server is logged into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Auth {
    Password,
    /// A private key file, optionally with a passphrase.
    Key,
    /// ssh-agent and ~/.ssh/config (OpenSSH defaults).
    Agent,
}

impl Auth {
    fn of(server: &Server) -> Self {
        if server.use_password {
            Auth::Password
        } else if !server.key.trim().is_empty() {
            Auth::Key
        } else {
            Auth::Agent
        }
    }

    fn label(self) -> &'static str {
        match self {
            Auth::Password => "Password",
            Auth::Key => "Key pair",
            Auth::Agent => "OpenSSH agent / config",
        }
    }
}

/// The server editor shown at the top of the window.
struct ServerForm {
    id: Option<String>,
    name: Entity<TextField>,
    host: Entity<TextField>,
    port: Entity<TextField>,
    user: Entity<TextField>,
    root: Entity<TextField>,
    key: Entity<TextField>,
    password: Entity<TextField>,
    auth: Auth,
}

#[derive(Default)]
pub(super) struct RemoteState {
    loaded: bool,
    deployment: Deployment,
    selected: Option<String>,
    cwd: String,
    entries: Option<Result<Vec<RemoteEntry>, String>>,
    busy: bool,
    status: Option<(String, bool)>,
    form: Option<ServerForm>,
    selected_entry: Option<String>,
    /// Bumped by every listing request; only the latest one's result is shown.
    generation: u64,
    /// When each file last went up, so the editor could tell whether it is still current.
    uploaded: HashMap<PathBuf, SystemTime>,
    /// The file on its way up right now.
    uploading: Option<PathBuf>,
}

fn secret(cx: &mut Context<Workspace>, placeholder: &'static str, text: &str) -> Entity<TextField> {
    let text = text.to_string();
    cx.new(|cx| {
        let mut field = TextField::new(placeholder, cx).masked();
        field.set_text(text, cx);
        field
    })
}

fn field(cx: &mut Context<Workspace>, placeholder: &'static str, text: &str) -> Entity<TextField> {
    let text = text.to_string();
    cx.new(|cx| {
        let mut field = TextField::new(placeholder, cx);
        field.set_text(text, cx);
        field
    })
}

/// Quotes `s` for a POSIX shell.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') { format!("{dir}{name}") } else { format!("{dir}/{name}") }
}

fn parent(dir: &str) -> String {
    let trimmed = dir.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".into(),
        Some(ix) => trimmed[..ix].into(),
    }
}

impl Workspace {
    pub(super) fn ensure_remote(&mut self, cx: &mut Context<Self>) {
        if self.remote.loaded {
            return;
        }
        self.remote.loaded = true;
        self.remote.deployment = deployment::load(self.project.root());
        let default = self.remote.deployment.default_server().map(|s| s.id.clone());
        if let Some(id) = default {
            self.select_server(id, cx);
        }
    }

    fn server(&self) -> Option<Server> {
        let id = self.remote.selected.as_ref()?;
        self.remote.deployment.servers.iter().find(|s| &s.id == id).cloned()
    }

    fn select_server(&mut self, id: String, cx: &mut Context<Self>) {
        let root = self.remote.deployment.servers.iter().find(|s| s.id == id).map(|s| s.remote_root.clone()).unwrap_or_else(|| "/".into());
        self.remote.selected = Some(id);
        self.browse(root, cx);
    }

    fn set_status(&mut self, text: impl Into<String>, ok: bool, cx: &mut Context<Self>) {
        self.remote.status = Some((text.into(), ok));
        cx.notify();
    }

    fn browse(&mut self, dir: String, cx: &mut Context<Self>) {
        let Some(server) = self.server() else { return };
        let password = deployment::password(self.project.root(), &server.id);
        self.remote.cwd = dir.clone();
        self.remote.entries = None;
        self.remote.busy = true;
        self.remote.selected_entry = None;
        self.remote.generation += 1;
        let generation = self.remote.generation;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { deployment::list(&server, &password, &dir) }).await;
            let _ = this.update(cx, |this, cx| {
                if this.remote.generation == generation {
                    this.remote.entries = Some(result);
                    this.remote.busy = false;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Where a remote file lands locally: its mapped project path, or a temp folder.
    fn local_for(&self, server: &Server, remote: &str) -> PathBuf {
        let base = server.remote_root.trim_end_matches('/');
        // Only plain folder names: `..`, `.` and roots never leave the target folder.
        let safe = |path: &str| -> PathBuf {
            Path::new(path).components().filter_map(|c| match c {
                std::path::Component::Normal(part) => Some(part),
                _ => None,
            }).collect()
        };
        let rel = remote.strip_prefix(base).filter(|r| r.starts_with('/')).map(|r| r.trim_start_matches('/'));
        match rel {
            Some(rel) if !rel.is_empty() && !base.is_empty() => self.project.root().join(safe(rel)),
            _ => {
                let id: String = server.id.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
                std::env::temp_dir().join("phpcraft-remote").join(if id.is_empty() { "server".into() } else { id }).join(safe(remote))
            }
        }
    }

    fn download_entry(&mut self, name: String, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.server() else { return };
        let remote = join(&self.remote.cwd, &name);
        let local = self.local_for(&server, &remote);
        let password = deployment::password(self.project.root(), &server.id);
        self.set_status(format!("Downloading {remote}…"), true, cx);
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let target = local.clone();
            let result = cx.background_spawn(async move { deployment::download(&server, &password, &remote, &target).map(|_| remote) }).await;
            let _ = cx.update_window(window_handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(remote) => {
                        this.set_status(format!("Downloaded {remote}"), true, cx);
                        if open {
                            this.open_file(&local, window, cx);
                        }
                    }
                    Err(err) => this.set_status(err, false, cx),
                });
            });
        })
        .detach();
    }

    /// After a save: uploads the file when Automatic Upload is on for this project.
    pub(crate) fn file_saved(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let config = deployment::load(&root);
        if !config.auto_upload || !path.starts_with(&root) {
            return;
        }
        let Some(server) = config.default_server().cloned() else { return };
        let password = deployment::password(&root, &server.id);
        let local = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { deployment::upload(&server, &password, &root, &local) }).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(remote) => this.set_status(format!("Uploaded to {remote}"), true, cx),
                Err(err) => this.set_status(format!("Automatic upload failed: {err}"), false, cx),
            });
        })
        .detach();
    }

    pub(super) fn toggle_auto_upload(&mut self, _: &ToggleAutoUpload, _: &mut Window, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let mut config = deployment::load(&root);
        config.auto_upload = !config.auto_upload;
        let message = match (config.auto_upload, config.default_server()) {
            (true, Some(server)) => format!("Automatic upload to {} is on", server.name),
            (true, None) => "Automatic upload is on — add a server in Remote Hosts".into(),
            (false, _) => "Automatic upload is off".into(),
        };
        match deployment::save(&root, &config) {
            Ok(()) => self.set_status(message, true, cx),
            Err(e) => self.set_status(e.to_string(), false, cx),
        }
    }

    fn upload_path(&mut self, local: PathBuf, cx: &mut Context<Self>) {
        let Some(server) = self.server().or_else(|| self.remote.deployment.default_server().cloned()) else {
            self.set_status(tr("Add an SFTP server first"), false, cx);
            return;
        };
        self.upload_to(&server.id, vec![local], None, cx);
    }

    /// Uploads files (folders expand to what they hold) to one server over one connection.
    /// Files from outside the project land in the browser's current folder.
    pub(super) fn upload_to(&mut self, server_id: &str, paths: Vec<PathBuf>, dir: Option<String>, cx: &mut Context<Self>) {
        let Some(server) = self.remote.deployment.servers.iter().find(|s| s.id == server_id).cloned() else {
            self.set_status(tr("That server is no longer configured"), false, cx);
            return;
        };
        if paths.is_empty() {
            return;
        }
        let root = self.project.root().to_path_buf();
        let password = deployment::password(&root, &server.id);
        let dir = dir.unwrap_or_else(|| self.remote.cwd.clone());
        let pairs: Vec<(PathBuf, String)> =
            paths.iter().map(|path| (path.clone(), deployment::remote_for(&server, &root, path, &dir))).collect();
        let count = pairs.len();
        let server_name = server.name.clone();
        self.remote.uploading = paths.first().cloned();
        self.set_status(trf("Uploading {} file(s) to {}…", &[&count.to_string(), &server_name]), true, cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { deployment::upload_many(&server, &password, &pairs) }).await;
            let _ = this.update(cx, |this, cx| {
                this.remote.uploading = None;
                match result {
                    Ok(files) => {
                        for path in &paths {
                            if let Ok(stamp) = std::fs::metadata(path).and_then(|m| m.modified()) {
                                this.remote.uploaded.insert(path.clone(), stamp);
                            }
                        }
                        this.set_status(trf("Uploaded {} file(s) to {}", &[&files.to_string(), &server_name]), true, cx);
                        if !this.remote.cwd.is_empty() {
                            let cwd = this.remote.cwd.clone();
                            this.browse(cwd, cx);
                        }
                    }
                    Err(err) => this.set_status(err, false, cx),
                }
            });
        })
        .detach();
    }

    /// "Upload to Server…": asks which host, then uploads the open file to it.
    pub(super) fn open_server_picker(&mut self, upload: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_remote(cx);
        if self.remote.deployment.servers.is_empty() {
            self.set_status(tr("Add an SFTP server first"), false, cx);
            return;
        }
        let servers = self.remote.deployment.servers.clone();
        let default = self.remote.deployment.default.clone();
        let dialog = cx.new(|cx| crate::dialogs::ServersDialog::new(servers, upload, default, window, cx));
        self.show_modal(dialog, window, cx);
    }

    /// The server list chose a host for the open file.
    pub(super) fn server_picked(&mut self, id: String, upload: bool, window: &mut Window, cx: &mut Context<Self>) {
        if upload {
            self.save_all(cx);
            match self.active_file(cx) {
                Some(path) => self.upload_to(&id, vec![path], None, cx),
                None => self.set_status(tr("Open a file to upload"), false, cx),
            }
        } else {
            self.download_to(id, window, cx)
        }
    }

    /// Makes `id` the server automatic upload and the editor chip work with.
    pub(super) fn default_server_changed(&mut self, id: String, cx: &mut Context<Self>) {
        let root = self.project.root().to_path_buf();
        let mut config = deployment::load(&root);
        let name = config.servers.iter().find(|s| s.id == id).map(|s| s.name.clone()).unwrap_or_default();
        config.default = Some(id);
        self.remote.deployment = config.clone();
        self.remote.selected = config.default.clone();
        match deployment::save(&root, &config) {
            Ok(()) => self.set_status(trf("Automatic upload now goes to {}", &[&name]), true, cx),
            Err(e) => self.set_status(e.to_string(), false, cx),
        }
    }

    /// Downloads the open file from a chosen server.
    pub(super) fn download_to(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(local) = self.active_file(cx) else {
            self.set_status(tr("Open a file to download"), false, cx);
            return;
        };
        let Some(server) = self.remote.deployment.servers.iter().find(|s| s.id == id).cloned() else {
            self.set_status(tr("That server is no longer configured"), false, cx);
            return;
        };
        let Some(remote) = deployment::remote_path(&server, self.project.root(), &local) else {
            self.set_status(tr("Only files inside the project can be downloaded"), false, cx);
            return;
        };
        let password = deployment::password(self.project.root(), &server.id);
        let window_handle = window.window_handle();
        self.set_status(trf("Downloading {}…", &[&remote]), true, cx);
        cx.spawn(async move |this, cx| {
            let target = local.clone();
            let pairs = vec![(remote.clone(), target)];
            let result = cx.background_spawn(async move { deployment::download_many(&server, &password, &pairs).map(|_| remote) }).await;
            let _ = cx.update_window(window_handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(remote) => {
                        this.set_status(trf("Downloaded {}", &[&remote]), true, cx);
                        this.reload_clean_tabs(&[local.clone()], cx);
                        this.open_file(&local, window, cx);
                    }
                    Err(err) => this.set_status(err, false, cx),
                });
            });
        })
        .detach();
    }

    /// Files dropped on the remote browser: upload them to the server shown there.
    pub(super) fn upload_dropped(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.ensure_remote(cx);
        let Some(server) = self.server().or_else(|| self.remote.deployment.default_server().cloned()) else {
            self.set_status(tr("Add an SFTP server first"), false, cx);
            return;
        };
        let dir = self.remote.cwd.clone();
        self.upload_to(&server.id, paths, Some(dir), cx);
    }

    /// A remote file dropped on the project tree: download it to its mapped local path.
    pub(super) fn download_dropped(&mut self, dragged: DraggedRemote, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.remote.deployment.servers.iter().find(|s| s.id == dragged.server).cloned() else {
            self.set_status(tr("That server is no longer configured"), false, cx);
            return;
        };
        let local = self.local_for(&server, &dragged.remote);
        let password = deployment::password(self.project.root(), &server.id);
        let window_handle = window.window_handle();
        let remote = dragged.remote.clone();
        self.set_status(trf("Downloading {}…", &[&remote]), true, cx);
        cx.spawn(async move |this, cx| {
            let target = local.clone();
            let pairs = vec![(remote.clone(), target)];
            let result = cx.background_spawn(async move { deployment::download_many(&server, &password, &pairs).map(|_| remote) }).await;
            let _ = cx.update_window(window_handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(remote) => {
                        this.set_status(trf("Downloaded {}", &[&remote]), true, cx);
                        this.open_file(&local, window, cx);
                    }
                    Err(err) => this.set_status(err, false, cx),
                });
            });
        })
        .detach();
    }

    /// The SFTP chip in the editor's tab bar: the server the open file goes to and whether it
    /// is up to date there. Clicking uploads; ⌥-click, the caret or a right-click asks which
    /// server.
    pub(super) fn render_remote_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let path = self.active_file(cx)?;
        let server = self.server().or_else(|| self.remote.deployment.default_server().cloned())?;
        deployment::remote_path(&server, self.project.root(), &path)?;
        let uploading = self.remote.uploading.as_deref() == Some(path.as_path());
        let stamp = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        let changed = self.remote.uploaded.get(&path).is_some_and(|up| stamp.is_none_or(|s| s > *up));
        let (color, tip) = if uploading {
            (theme::accent(), trf("Uploading to {}…", &[&server.name]))
        } else if self.remote.uploaded.get(&path).is_none() {
            (theme::muted(), trf("Not uploaded to {} yet · ⌥⇧⌘X", &[&server.name]))
        } else if changed {
            (theme::accent(), trf("Changed since the last upload to {} · ⌥⇧⌘X", &[&server.name]))
        } else {
            (theme::success(), trf("Matches {} · ⌥⇧⌘X", &[&server.name]))
        };
        let id = server.id.clone();
        let local = path.clone();
        Some(
            div()
                .id("remote-chip")
                .flex()
                .items_center()
                .gap_1()
                .h(px(22.))
                .px_2()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(theme::border())
                .text_size(px(11.5))
                .text_color(theme::muted())
                .hover(|d| d.bg(theme::hover()))
                .tooltip(super::layout::tooltip(tip))
                .child(icon(Icon::upload, 12., color))
                .child(SharedString::from(server.name.clone()))
                .child(div().text_color(color).child(if uploading { "…" } else { "●" }))
                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                    if event.modifiers().alt {
                        this.open_server_picker(true, window, cx);
                    } else {
                        this.upload_to(&id, vec![local.clone()], None, cx);
                    }
                }))
                .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |this, _, window, cx| this.open_server_picker(true, window, cx)))
                .child(
                    div()
                        .id("remote-chip-more")
                        .px_1()
                        .rounded_sm()
                        .hover(|d| d.bg(theme::active_row()))
                        .child("⌄")
                        .on_click(cx.listener(move |this, _, window, cx| this.open_server_picker(true, window, cx))),
                )
                .into_any_element(),
        )
    }

    fn active_file(&self, cx: &gpui::App) -> Option<PathBuf> {
        self.tabs.get(self.active).and_then(|t| t.read(cx).buffer.path().map(Path::to_path_buf))
    }

    pub(super) fn upload_to_server(&mut self, _: &UploadToServer, _: &mut Window, cx: &mut Context<Self>) {
        self.ensure_remote(cx);
        self.save_all(cx);
        match self.active_file(cx) {
            Some(path) => self.upload_path(path, cx),
            None => self.set_status("Open a file to upload", false, cx),
        }
    }

    /// "Upload to…": pick the host for the open file.
    pub(super) fn upload_pick(&mut self, _: &UploadToServerPick, window: &mut Window, cx: &mut Context<Self>) {
        self.open_server_picker(true, window, cx);
    }

    /// "Download from…": pick the host to pull the open file from.
    pub(super) fn download_pick(&mut self, _: &DownloadFromServerPick, window: &mut Window, cx: &mut Context<Self>) {
        self.open_server_picker(false, window, cx);
    }

    pub(super) fn download_from_server(&mut self, _: &DownloadFromServer, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_remote(cx);
        let Some(local) = self.active_file(cx) else { return };
        let Some(server) = self.server().or_else(|| self.remote.deployment.default_server().cloned()) else { return };
        let Some(remote) = deployment::remote_path(&server, self.project.root(), &local) else { return };
        let password = deployment::password(self.project.root(), &server.id);
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let target = local.clone();
            let result = cx.background_spawn(async move { deployment::download(&server, &password, &remote, &target).map(|_| remote) }).await;
            let _ = cx.update_window(window_handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(remote) => {
                        this.set_status(format!("Downloaded {remote}"), true, cx);
                        this.open_file(&local, window, cx);
                    }
                    Err(err) => this.set_status(err, false, cx),
                });
            });
        })
        .detach();
    }

    /// The entry the list has selected, if it is still there.
    fn selected_remote(&self) -> Option<(String, bool)> {
        let name = self.remote.selected_entry.clone()?;
        let entries = self.remote.entries.as_ref()?.as_ref().ok()?;
        entries.iter().find(|e| e.name == name).map(|e| (join(&self.remote.cwd, &e.name), e.is_dir))
    }

    /// Downloads the selected file — or the whole selected folder — into its mapped place in
    /// the project.
    pub(super) fn download_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.server() else { return };
        let Some((remote, is_dir)) = self.selected_remote() else {
            self.set_status(tr("Select a file or a folder first"), false, cx);
            return;
        };
        let local = self.local_for(&server, &remote);
        let password = deployment::password(self.project.root(), &server.id);
        let window_handle = window.window_handle();
        self.set_status(trf("Downloading {}…", &[&remote]), true, cx);
        cx.spawn(async move |this, cx| {
            let target = local.clone();
            let result = cx.background_spawn(async move {
                if is_dir {
                    deployment::download_tree(&server, &password, &remote, &target).map(|files| files.len())
                } else {
                    deployment::download(&server, &password, &remote, &target).map(|_| 1)
                }
            }).await;
            let _ = cx.update_window(window_handle, |_, window, cx| {
                let _ = this.update(cx, |this, cx| match result {
                    Ok(count) => {
                        this.set_status(trf("Downloaded {} file(s)", &[&count.to_string()]), true, cx);
                        this.rows = this.project.rows();
                        this.files_cache = None;
                        this.rescan(cx);
                        if !is_dir {
                            this.open_file(&local, window, cx);
                        }
                        cx.notify();
                    }
                    Err(err) => this.set_status(err, false, cx),
                });
            });
        })
        .detach();
    }

    pub(super) fn download_selected_action(&mut self, _: &DownloadSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.download_selected(window, cx)
    }

    pub(super) fn rename_selected_action(&mut self, _: &RenameSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.rename_selected(window, cx)
    }

    pub(super) fn delete_selected_action(&mut self, _: &DeleteSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.delete_selected(window, cx)
    }

    /// Asks for a new name for the selected entry.
    pub(super) fn rename_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = self.server() else { return };
        let Some((remote, _)) = self.selected_remote() else {
            self.set_status(tr("Select a file or a folder first"), false, cx);
            return;
        };
        let op = crate::dialogs::FileOp::RenameRemote { server: server.id, from: remote };
        let dialog = cx.new(|cx| crate::dialogs::InputDialog::new(op, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn remote_renamed(&mut self, server_id: String, from: String, name: String, cx: &mut Context<Self>) {
        let Some(server) = self.remote.deployment.servers.iter().find(|s| s.id == server_id).cloned() else {
            self.set_status(tr("That server is no longer configured"), false, cx);
            return;
        };
        let dir = parent(&from);
        let to = if dir == "/" { format!("/{name}") } else { format!("{dir}/{name}") };
        let password = deployment::password(self.project.root(), &server.id);
        self.set_status(trf("Renaming {}…", &[&from]), true, cx);
        cx.spawn(async move |this, cx| {
            let done = cx.background_spawn(async move { deployment::rename_remote(&server, &password, &from, &to).map(|_| to) }).await;
            let _ = this.update(cx, |this, cx| match done {
                Ok(to) => {
                    this.set_status(trf("Renamed to {}", &[&to]), true, cx);
                    let cwd = this.remote.cwd.clone();
                    this.browse(cwd, cx);
                }
                Err(err) => this.set_status(err, false, cx),
            });
        })
        .detach();
    }

    /// Asks before deleting the selected file or folder from the server.
    pub(super) fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((remote, is_dir)) = self.selected_remote() else {
            self.set_status(tr("Select a file or a folder first"), false, cx);
            return;
        };
        let message = if is_dir {
            trf("Delete the folder {} and everything in it from the server?", &[&remote])
        } else {
            trf("Delete {} from the server?", &[&remote])
        };
        let dialog = cx.new(|cx| {
            crate::dialogs::ConfirmPrompt::new(format!("remote-delete:{remote}"), tr("Delete from Server"), message, "Delete", window, cx)
        });
        self.show_modal(dialog, window, cx);
    }

    /// The answer to [`Self::delete_selected`]: remove the path, then show the folder again.
    pub(super) fn remote_delete(&mut self, remote: String, cx: &mut Context<Self>) {
        let Some(server) = self.server() else { return };
        let is_dir = self
            .remote
            .entries
            .as_ref()
            .and_then(|entries| entries.as_ref().ok())
            .and_then(|entries| entries.iter().find(|e| join(&self.remote.cwd, &e.name) == remote))
            .is_some_and(|e| e.is_dir);
        let password = deployment::password(self.project.root(), &server.id);
        self.set_status(trf("Deleting {}…", &[&remote]), true, cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { deployment::delete_remote_path(&server, &password, &remote, is_dir) }).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(_) => {
                    this.set_status(tr("Deleted"), true, cx);
                    let cwd = this.remote.cwd.clone();
                    this.browse(cwd, cx);
                }
                Err(err) => this.set_status(err, false, cx),
            });
        })
        .detach();
    }

    fn open_ssh(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.server() else { return };
        // Every field is quoted for the local shell; the remote command is quoted again for the
        // remote shell, so folder names from the server can't run anything.
        let mut command = format!("ssh -p {} -o StrictHostKeyChecking=accept-new", server.port);
        if let Some(key) = deployment::key_path(&server) {
            command.push_str(&format!(" -i {}", sh_quote(&key)));
        }
        let remote = format!("cd {} 2>/dev/null; exec \"$SHELL\" -l", sh_quote(&self.remote.cwd));
        command.push_str(&format!(" -t -- {} {}", sh_quote(&format!("{}@{}", server.user, server.host)), sh_quote(&remote)));
        let config = RunConfig { name: format!("SSH {}", server.name), kind: Kind::Shell { command }, env: Default::default() };
        self.add_terminal(Some(config), cx);
    }

    fn test_server(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.server() else { return };
        let password = deployment::password(self.project.root(), &server.id);
        self.set_status("Testing connection…", true, cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { deployment::test(&server, &password) }).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(text) => this.set_status(format!("✓ {text}"), true, cx),
                Err(err) => this.set_status(err, false, cx),
            });
        })
        .detach();
    }

    fn edit_server(&mut self, existing: Option<Server>, cx: &mut Context<Self>) {
        let server = existing.clone().unwrap_or_default();
        let password = existing.as_ref().map(|s| deployment::password(self.project.root(), &s.id)).unwrap_or_default();
        self.remote.form = Some(ServerForm {
            id: existing.map(|s| s.id),
            name: field(cx, "Name", &server.name),
            host: field(cx, "Host (example.com)", &server.host),
            port: field(cx, "Port", &server.port.to_string()),
            user: field(cx, "User", &server.user),
            root: field(cx, "Remote root (/var/www/app)", &server.remote_root),
            key: field(cx, "~/.ssh/id_ed25519", &server.key),
            password: secret(cx, "Password or key passphrase", &password),
            auth: Auth::of(&server),
        });
        cx.notify();
    }

    /// Picks a private key file (starts in ~/.ssh when the system dialog allows).
    fn browse_key(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions { files: true, directories: false, multiple: false, prompt: Some("Choose Key".into()) });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let _ = this.update(cx, |this, cx| {
                if let Some(form) = &this.remote.form {
                    form.key.update(cx, |field, cx| field.set_text(path.display().to_string(), cx));
                }
            });
        })
        .detach();
    }

    fn save_form(&mut self, cx: &mut Context<Self>) {
        let Some(form) = self.remote.form.take() else { return };
        let text = |f: &Entity<TextField>| f.read(cx).text().trim().to_string();
        let host = text(&form.host);
        if host.is_empty() {
            self.remote.form = Some(form);
            self.set_status("Host is required", false, cx);
            return;
        }
        let id = form.id.clone().unwrap_or_else(|| format!("srv-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0)));
        let name = text(&form.name);
        let server = Server {
            id: id.clone(),
            name: if name.is_empty() { host.clone() } else { name },
            host,
            port: text(&form.port).parse().unwrap_or(22),
            user: text(&form.user),
            remote_root: text(&form.root),
            key: if form.auth == Auth::Key { text(&form.key) } else { String::new() },
            use_password: form.auth == Auth::Password,
        };
        if form.auth == Auth::Key && server.key.is_empty() {
            self.remote.form = Some(form);
            self.set_status("Choose the private key file", false, cx);
            return;
        }
        // The password, or the key's passphrase; nothing for the agent.
        let password = if form.auth == Auth::Agent { String::new() } else { form.password.read(cx).text().to_string() };
        deployment::set_password(self.project.root(), &id, &password);
        let servers = &mut self.remote.deployment.servers;
        match servers.iter_mut().find(|s| s.id == id) {
            Some(slot) => *slot = server,
            None => servers.push(server),
        }
        if self.remote.deployment.default.is_none() {
            self.remote.deployment.default = Some(id.clone());
        }
        let _ = deployment::save(self.project.root(), &self.remote.deployment);
        self.select_server(id, cx);
    }

    fn delete_server(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.remote.selected.take() else { return };
        self.remote.deployment.servers.retain(|s| s.id != id);
        deployment::set_password(self.project.root(), &id, "");
        if self.remote.deployment.default.as_deref() == Some(id.as_str()) {
            self.remote.deployment.default = self.remote.deployment.servers.first().map(|s| s.id.clone());
        }
        let _ = deployment::save(self.project.root(), &self.remote.deployment);
        self.remote.entries = None;
        self.remote.cwd.clear();
        if let Some(next) = self.remote.deployment.servers.first().map(|s| s.id.clone()) {
            self.select_server(next, cx);
        }
        cx.notify();
    }

    fn render_server_form(&self, form: &ServerForm, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let row = |label: &'static str, field: &Entity<TextField>| {
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(div().w(px(76.)).flex_none().text_color(theme::muted()).child(label))
                .child(div().flex_1().flex().child(field.clone()))
        };
        let auth = form.auth;
        let chips = [Auth::Password, Auth::Key, Auth::Agent].map(|kind| {
            let on = kind == auth;
            div()
                .id(SharedString::from(format!("remote-auth-{}", kind.label())))
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(if on { theme::accent() } else { theme::border() })
                .when(on, |d| d.bg(theme::selection()))
                .text_color(if on { theme::ui_text() } else { theme::muted() })
                .child(kind.label())
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(form) = this.remote.form.as_mut() {
                        form.auth = kind;
                    }
                    cx.notify();
                }))
        });
        div()
            .flex()
            .flex_col()
            .gap_1p5()
            .p_3()
            .border_b_1()
            .border_color(theme::border())
            .child(div().text_color(theme::ui_text()).child(if form.id.is_some() { "Edit server" } else { "New SFTP server" }))
            .child(row("Name", &form.name))
            .child(row("Host", &form.host))
            .child(row("Port", &form.port))
            .child(row("User", &form.user))
            .child(row("Root path", &form.root))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().w(px(76.)).flex_none().text_color(theme::muted()).child(tr("Auth")))
                    .child(div().flex().flex_wrap().gap_1().children(chips)),
            )
            .when(auth == Auth::Key, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().w(px(76.)).flex_none().text_color(theme::muted()).child(tr("Key file")))
                        .child(div().flex_1().flex().child(form.key.clone()))
                        .child(crate::dialogs::button("remote-browse-key", "Browse…", false).on_click(cx.listener(|this, _, _, cx| this.browse_key(cx)))),
                )
                .child(row("Passphrase", &form.password))
            })
            .when(auth == Auth::Password, |d| d.child(row("Password", &form.password)))
            .when(auth == Auth::Agent, |d| {
                d.child(div().pl(px(84.)).text_size(px(11.)).text_color(theme::muted()).child(tr("Uses ssh-agent and ~/.ssh/config, like `ssh user@host` in a terminal.")))
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(crate::dialogs::button("remote-cancel", "Cancel", false).on_click(cx.listener(|this, _, _, cx| {
                        this.remote.form = None;
                        cx.notify();
                    })))
                    .child(crate::dialogs::button("remote-save", "Save", true).on_click(cx.listener(|this, _, _, cx| this.save_form(cx)))),
            )
    }

    pub(super) fn render_remote(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tool = |id: &'static str, kind: Icon, tip: &'static str| {
            div()
                .id(id)
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .cursor_pointer()
                .hover(|d| d.bg(theme::hover()))
                .child(icon(kind, 15., theme::muted()))
                .tooltip(super::layout::tooltip(tip))
        };
        let has_server = self.remote.selected.is_some();
        let toolbar = div()
            .flex()
            .items_center()
            .gap_0p5()
            .px_2()
            .h(px(32.))
            .border_b_1()
            .border_color(theme::border())
            .child(tool("remote-add", Icon::plus, "Add SFTP server").on_click(cx.listener(|this, _, _, cx| this.edit_server(None, cx))))
            .when(has_server, |d| {
                d.child(tool("remote-edit", Icon::settings, "Edit server").on_click(cx.listener(|this, _, _, cx| {
                    let server = this.server();
                    this.edit_server(server, cx)
                })))
                .child(tool("remote-delete", Icon::minus, "Remove server").on_click(cx.listener(|this, _, _, cx| this.delete_server(cx))))
                .child(tool("remote-refresh", Icon::refresh, "Refresh").on_click(cx.listener(|this, _, _, cx| {
                    let cwd = this.remote.cwd.clone();
                    this.browse(cwd, cx)
                })))
                .child(tool("remote-test", Icon::check, "Test connection").on_click(cx.listener(|this, _, _, cx| this.test_server(cx))))
                .child(tool("remote-ssh", Icon::terminal, "Open SSH session in the terminal").on_click(cx.listener(|this, _, _, cx| this.open_ssh(cx))))
                .child(tool("remote-upload", Icon::upload, "Upload current file  ⌥⇧⌘X").on_click(cx.listener(|this, _, window, cx| {
                    this.upload_to_server(&UploadToServer, window, cx)
                })))
                .child(tool("remote-download", Icon::download, "Download current file from server").on_click(cx.listener(|this, _, window, cx| {
                    this.download_from_server(&DownloadFromServer, window, cx)
                })))
                .child(tool("remote-upload-pick", Icon::upload, "Upload current file to…  ⌥⇧⌘U").on_click(cx.listener(|this, _, window, cx| {
                    this.open_server_picker(true, window, cx)
                })))
                .child(tool("remote-download-pick", Icon::download, "Download current file from…  ⇧⌥⌘D").on_click(cx.listener(|this, _, window, cx| {
                    this.open_server_picker(false, window, cx)
                })))
                .when(self.remote.selected_entry.is_some(), |d| {
                    d.child(tool("remote-download-entry", Icon::download, "Download selected  ⇧⌥⌘G").on_click(cx.listener(|this, _, window, cx| {
                        this.download_selected(window, cx)
                    })))
                    .child(tool("remote-rename-entry", Icon::settings, "Rename on the server…  F2").on_click(cx.listener(|this, _, window, cx| {
                        this.rename_selected(window, cx)
                    })))
                    .child(tool("remote-delete-entry", Icon::minus, "Delete from the server…").on_click(cx.listener(|this, _, window, cx| {
                        this.delete_selected(window, cx)
                    })))
                })
            });

        let servers = self.remote.deployment.servers.iter().map(|server| {
            let selected = self.remote.selected.as_deref() == Some(server.id.as_str());
            let id = server.id.clone();
            div()
                .id(SharedString::from(format!("remote-server-{}", server.id)))
                .flex()
                .items_center()
                .gap_1p5()
                .px_2()
                .py_0p5()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_color(if selected { theme::accent() } else { theme::border() })
                .when(selected, |d| d.bg(theme::selection()))
                .child(icon(Icon::globe, 13., if selected { theme::accent() } else { theme::muted() }))
                .child(server.name.clone())
                .on_click(cx.listener(move |this, _, _, cx| this.select_server(id.clone(), cx)))
        });

        let body: gpui::AnyElement = if self.remote.deployment.servers.is_empty() && self.remote.form.is_none() {
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .p_6()
                .text_color(theme::muted())
                .child(icon(Icon::globe, 32., theme::muted()))
                .child(tr("No remote hosts yet"))
                .child(div().text_size(px(12.)).child(tr("Add an SFTP server to browse, upload and open an SSH session.")))
                .child(crate::dialogs::button("remote-first", "Add SFTP server", true).on_click(cx.listener(|this, _, _, cx| this.edit_server(None, cx))))
                .into_any_element()
        } else {
            let mut list = div()
                .id("remote-files")
                .flex_1()
                .overflow_y_scroll()
                .flex()
                .flex_col()
                .py_1()
                // Dropping here uploads: from the project tree or from the Finder.
                .drag_over::<DraggedPaths>(|d, _, _, _| d.bg(theme::selection()))
                .on_drop(cx.listener(|this, paths: &DraggedPaths, _, cx| this.upload_dropped(paths.0.clone(), cx)))
                .drag_over::<gpui::ExternalPaths>(|d, _, _, _| d.bg(theme::selection()))
                .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _, cx| this.upload_dropped(paths.paths().to_vec(), cx)));
            if self.remote.cwd != "/" && !self.remote.cwd.is_empty() {
                list = list.child(
                    div()
                        .id("remote-up")
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .px_3()
                        .h(px(24.))
                        .cursor_pointer()
                        .hover(|d| d.bg(theme::hover()))
                        .child(icon(Icon::folder, 15., theme::muted()))
                        .child("..")
                        .on_click(cx.listener(|this, _, _, cx| {
                            let up = parent(&this.remote.cwd);
                            this.browse(up, cx)
                        })),
                );
            }
            match &self.remote.entries {
                None if self.remote.busy => list = list.child(div().px_3().py_2().text_color(theme::muted()).child(tr("Loading…"))),
                None => {}
                Some(Err(err)) => list = list.child(div().px_3().py_2().text_color(theme::error()).child(err.clone())),
                Some(Ok(entries)) => {
                    for (ix, entry) in entries.iter().enumerate() {
                        let name = entry.name.clone();
                        let is_dir = entry.is_dir;
                        let selected = self.remote.selected_entry.as_deref() == Some(name.as_str());
                        let file_icon = super::file_icon(&entry.name, is_dir, false);
                        list = list.child(
                            div()
                                .id(("remote-entry", ix))
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .px_3()
                                .h(px(24.))
                                .cursor_pointer()
                                .when(selected, |d| d.bg(theme::active_row()))
                                .when(!selected, |d| d.hover(|d| d.bg(theme::hover())))
                                .child(file_icon)
                                .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(entry.name.clone()))
                                .when(!is_dir, |d| d.child(div().text_size(px(11.)).text_color(theme::muted()).child(entry.size.clone())))
                                .when(!is_dir, |d| {
                                    let server = self.remote.selected.clone().unwrap_or_default();
                                    let remote = join(&self.remote.cwd, &entry.name);
                                    d.on_drag(DraggedRemote { server, remote, name: entry.name.clone() }, |drag, _, _, cx| {
                                        cx.new(|_| RemoteDrag(drag.name.clone()))
                                    })
                                })
                                .on_click(cx.listener(move |this, event: &gpui::ClickEvent, window, cx| {
                                    if is_dir {
                                        let dir = join(&this.remote.cwd, &name);
                                        this.browse(dir, cx);
                                    } else if event.click_count() >= 2 {
                                        this.download_entry(name.clone(), true, window, cx);
                                    } else {
                                        this.remote.selected_entry = Some(name.clone());
                                        cx.notify();
                                    }
                                })),
                        );
                    }
                }
            }
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(div().flex().flex_wrap().gap_1().px_2().py_1p5().children(servers))
                .when(!self.remote.cwd.is_empty(), |d| {
                    d.child(
                        div()
                            .px_3()
                            .py_1()
                            .text_size(px(11.))
                            .text_color(theme::muted())
                            .border_b_1()
                            .border_color(theme::border())
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(self.remote.cwd.clone()),
                    )
                })
                .child(list)
                .into_any_element()
        };

        let status = self.remote.status.clone().map(|(text, ok)| {
            div()
                .px_3()
                .py_1p5()
                .text_size(px(11.))
                .border_t_1()
                .border_color(theme::border())
                .text_color(if ok { theme::muted() } else { theme::error() })
                .child(text)
        });

        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(13.))
            .text_color(theme::ui_text())
            .child(toolbar)
            .children(self.remote.form.as_ref().map(|form| self.render_server_form(form, cx)))
            .child(body)
            .children(status)
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child(tr("Double-click a file to download and open it")),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_paths() {
        assert_eq!(join("/var/www", "app"), "/var/www/app");
        assert_eq!(join("/", "etc"), "/etc");
        assert_eq!(parent("/var/www/app"), "/var/www");
        assert_eq!(parent("/var"), "/");
        assert_eq!(sh_quote("a'b $(x)"), "'a'\\''b $(x)'");
    }
}
