//! Remote Hosts tool window (PhpStorm's Deployment ▸ Browse Remote Host): SFTP servers from
//! `.phpcraft/deployment.json`, a remote file browser, upload/download of project files and an
//! SSH session in the terminal. Transfers go through the system OpenSSH `sftp`.

use std::path::{Path, PathBuf};

use gpui::{AppContext, Context, Entity, SharedString, Window, actions, div, prelude::*, px};

use super::Workspace;
use crate::deployment::{self, Deployment, RemoteEntry, Server};
use crate::icons::{Icon, icon};
use crate::run_configs::{Kind, RunConfig};
use crate::text_field::TextField;
use crate::theme;

actions!(workspace, [UploadToServer, DownloadFromServer]);

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
    use_password: bool,
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

    fn upload_path(&mut self, local: PathBuf, cx: &mut Context<Self>) {
        let Some(server) = self.server().or_else(|| self.remote.deployment.default_server().cloned()) else {
            self.set_status("Add a server first", false, cx);
            return;
        };
        let root = self.project.root().to_path_buf();
        let password = deployment::password(&root, &server.id);
        self.set_status(format!("Uploading {}…", local.display()), true, cx);
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { deployment::upload(&server, &password, &root, &local) }).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(remote) => {
                    this.set_status(format!("Uploaded to {remote}"), true, cx);
                    if !this.remote.cwd.is_empty() {
                        let cwd = this.remote.cwd.clone();
                        this.browse(cwd, cx);
                    }
                }
                Err(err) => this.set_status(err, false, cx),
            });
        })
        .detach();
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

    fn open_ssh(&mut self, cx: &mut Context<Self>) {
        let Some(server) = self.server() else { return };
        // Every field is quoted for the local shell; the remote command is quoted again for the
        // remote shell, so folder names from the server can't run anything.
        let mut command = format!("ssh -p {} -o StrictHostKeyChecking=accept-new", server.port);
        if !server.key.trim().is_empty() {
            command.push_str(&format!(" -i {}", sh_quote(server.key.trim())));
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
            key: field(cx, "Private key (empty = ssh-agent)", &server.key),
            password: field(cx, "Password", &password),
            use_password: server.use_password,
        });
        cx.notify();
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
            key: text(&form.key),
            use_password: form.use_password,
        };
        let password = if form.use_password { form.password.read(cx).text().to_string() } else { String::new() };
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
        let use_password = form.use_password;
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
            .child(row("Key file", &form.key))
            .child(
                div()
                    .id("remote-use-password")
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .child(icon(if use_password { Icon::todo } else { Icon::stop }, 14., if use_password { theme::accent() } else { theme::muted() }))
                    .child("Password authentication")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(form) = this.remote.form.as_mut() {
                            form.use_password = !form.use_password;
                        }
                        cx.notify();
                    })),
            )
            .when(use_password, |d| d.child(row("Password", &form.password)))
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
                .child("No remote hosts yet")
                .child(div().text_size(px(12.)).child("Add an SFTP server to browse, upload and open an SSH session."))
                .child(crate::dialogs::button("remote-first", "Add SFTP server", true).on_click(cx.listener(|this, _, _, cx| this.edit_server(None, cx))))
                .into_any_element()
        } else {
            let mut list = div().id("remote-files").flex_1().overflow_y_scroll().flex().flex_col().py_1();
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
                None if self.remote.busy => list = list.child(div().px_3().py_2().text_color(theme::muted()).child("Loading…")),
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
                    .child("Double-click a file to download and open it"),
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
