//! Preferences: Keymap editor (⌘K ⌘S) and theme selector (⌘K ⌘T).

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, SharedString, Subscription, Window,
    div, prelude::*, px,
};

use super::{Cancel, Confirm, DialogEvent, SelectNext, SelectPrev, button, card};
use crate::keymap::{self, Entry};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

const VISIBLE_ROWS: usize = 18;

pub struct KeymapDialog {
    query: Entity<TextField>,
    entries: Vec<Entry>,
    selected: usize,
    /// Waiting for the user to press the new shortcut.
    recording: bool,
    /// Keystroke captured while recording, before it is saved.
    captured: Option<String>,
    status: Option<String>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DialogEvent> for KeymapDialog {}

impl KeymapDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new("Search actions or shortcuts", cx));
        let field_sub = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            cx.notify();
        });
        // Runs before key bindings, so the pressed shortcut is recorded instead of executed.
        let weak = cx.weak_entity();
        let interceptor = cx.intercept_keystrokes(move |event, _window, cx| {
            let Some(dialog) = weak.upgrade() else { return };
            if !dialog.read(cx).recording {
                return;
            }
            cx.stop_propagation();
            let keys = event.keystroke.unparse();
            dialog.update(cx, |dialog, cx| {
                if keys == "escape" {
                    dialog.recording = false;
                } else {
                    dialog.captured = Some(keys);
                    dialog.recording = false;
                }
                cx.notify();
            });
        });
        window.focus(&query.focus_handle(cx));
        Self {
            query,
            entries: keymap::effective(&keymap::load_user()),
            selected: 0,
            recording: false,
            captured: None,
            status: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: vec![field_sub, interceptor],
        }
    }

    fn matches(&self, cx: &App) -> Vec<Entry> {
        let query = self.query.read(cx).text().to_ascii_lowercase();
        let words: Vec<&str> = query.split_whitespace().collect();
        let mut seen = std::collections::HashSet::new();
        self.entries
            .iter()
            .filter(|e| {
                let haystack = format!(
                    "{} {} {} {} {}",
                    keymap::humanize(&e.action),
                    e.action,
                    e.keys,
                    keymap::display(&e.keys),
                    e.context.as_deref().unwrap_or("")
                )
                .to_ascii_lowercase();
                words.iter().all(|w| haystack.contains(w))
            })
            // Menu/popup duplicates are noise in the list.
            .filter(|e| seen.insert((e.action.clone(), e.keys.clone(), e.context.clone())))
            .cloned()
            .collect()
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.selected = self.selected.saturating_sub(1);
        self.captured = None;
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let count = self.matches(cx).len();
        self.selected = (self.selected + 1).min(count.saturating_sub(1));
        self.captured = None;
        cx.notify();
    }

    fn selected_entry(&self, cx: &App) -> Option<Entry> {
        self.matches(cx).into_iter().nth(self.selected)
    }

    fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.selected_entry(cx).is_some() {
            self.recording = true;
            self.captured = None;
            self.status = None;
            cx.notify();
        }
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let errors = keymap::apply(cx);
        self.entries = keymap::effective(&keymap::load_user());
        self.status = errors.first().cloned();
        cx.notify();
    }

    fn save_captured(&mut self, cx: &mut Context<Self>) {
        let (Some(entry), Some(keys)) = (self.selected_entry(cx), self.captured.take()) else {
            return;
        };
        match keymap::set_binding(&entry.action, entry.context.as_deref(), Some(&keys)) {
            Ok(()) => self.reload(cx),
            Err(err) => self.status = Some(format!("Failed to save keymap.json: {err}")),
        }
        cx.notify();
    }

    fn reset_selected(&mut self, cx: &mut Context<Self>) {
        let Some(entry) = self.selected_entry(cx) else {
            return;
        };
        if let Err(err) = keymap::set_binding(&entry.action, entry.context.as_deref(), None) {
            self.status = Some(format!("Failed to save keymap.json: {err}"));
        }
        self.reload(cx);
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        if self.captured.is_some() {
            self.save_captured(cx);
        } else {
            self.start_recording(cx);
        }
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.captured.take().is_some() {
            cx.notify();
        } else {
            cx.emit(DialogEvent::Dismiss);
        }
    }
}

impl Focusable for KeymapDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn key_pill(text: String, highlight: bool) -> gpui::Div {
    div()
        .px_1p5()
        .py(px(1.))
        .rounded_sm()
        .border_1()
        .border_color(if highlight {
            theme::accent()
        } else {
            theme::border_strong()
        })
        .bg(theme::editor_bg())
        .text_size(px(12.))
        .child(text)
}

impl Render for KeymapDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let matches = self.matches(cx);
        let selected = self.selected.min(matches.len().saturating_sub(1));
        let first = selected.saturating_sub(VISIBLE_ROWS - 1);
        let rows: Vec<_> = matches
            .iter()
            .enumerate()
            .skip(first)
            .take(VISIBLE_ROWS)
            .map(|(ix, entry)| {
                let is_selected = ix == selected;
                let namespace = entry.action.split("::").next().unwrap_or("").to_string();
                let keys = if is_selected && self.recording {
                    "Press a shortcut…".to_string()
                } else if let (true, Some(captured)) = (is_selected, &self.captured) {
                    keymap::display(captured)
                } else {
                    keymap::display(&entry.keys)
                };
                div()
                    .id(("keymap-row", ix))
                    .flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .h(px(28.))
                    .cursor_pointer()
                    .when(is_selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(
                        div()
                            .w(px(260.))
                            .flex_none()
                            .text_color(theme::ui_text())
                            .child(keymap::humanize(&entry.action)),
                    )
                    .child(
                        div()
                            .w(px(90.))
                            .flex_none()
                            .text_color(theme::muted())
                            .text_size(px(12.))
                            .child(namespace),
                    )
                    .child(div().w(px(170.)).flex_none().child(key_pill(
                        keys,
                        is_selected && (self.recording || self.captured.is_some()),
                    )))
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child(entry.context.clone().unwrap_or_default()),
                    )
                    .when(entry.user, |d| {
                        d.child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme::accent())
                                .child("custom"),
                        )
                    })
                    .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                        this.selected = ix;
                        this.captured = None;
                        if event.click_count() >= 2 {
                            this.start_recording(cx);
                        }
                        cx.notify();
                    }))
            })
            .collect();
        let hint = if self.recording {
            "Press the new shortcut, or Esc to cancel."
        } else if self.captured.is_some() {
            "↵ save · Esc discard"
        } else {
            "↵ or double-click to change · Saved to keymap.json in the config folder"
        };
        card(820.)
            .key_context("ListDialog KeymapDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child("Keymap"),
            )
            .child(div().p_2().child(self.query.clone()))
            .child(
                div()
                    .pb_1()
                    .min_h(px(VISIBLE_ROWS as f32 * 28.))
                    .text_size(px(13.))
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child(self.status.clone().unwrap_or_else(|| hint.to_string())),
                    )
                    .child(
                        button("keymap-reset", "Reset", false)
                            .on_click(cx.listener(|this, _, _, cx| this.reset_selected(cx))),
                    )
                    .child(button("keymap-change", "Change", true).on_click(
                        cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)),
                    )),
            )
    }
}

// --- Themes -------------------------------------------------------------------------------

pub struct ThemeDialog {
    themes: Vec<(String, String)>,
    /// Theme active when the dialog opened, restored on cancel.
    original: String,
    selected: usize,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for ThemeDialog {}

impl ThemeDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let themes: Vec<(String, String)> = theme::available()
            .into_iter()
            .map(|t| (t.name, t.appearance))
            .collect();
        let original = theme::current_name();
        let selected = themes
            .iter()
            .position(|(name, _)| *name == original)
            .unwrap_or(0);
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self {
            themes,
            original,
            selected,
            focus_handle,
        }
    }

    fn preview(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = ix;
        if let Some((name, _)) = self.themes.get(ix) {
            theme::set(&name.clone(), cx);
        }
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        self.preview(self.selected.saturating_sub(1), cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.preview(
            (self.selected + 1).min(self.themes.len().saturating_sub(1)),
            cx,
        );
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        let original = self.original.clone();
        theme::set(&original, cx);
        cx.emit(DialogEvent::Dismiss);
    }
}

impl Focusable for ThemeDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for ThemeDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows: Vec<_> = self
            .themes
            .iter()
            .enumerate()
            .map(|(ix, (name, appearance))| {
                div()
                    .id(("theme", ix))
                    .flex()
                    .justify_between()
                    .px_3()
                    .h(px(28.))
                    .items_center()
                    .cursor_pointer()
                    .when(ix == self.selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(SharedString::from(name.clone()))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme::muted())
                            .child(SharedString::from(appearance.clone())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| this.preview(ix, cx)))
            })
            .collect();
        let folder = crate::config::dir().join("themes");
        card(420.)
            .key_context("ListDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme::border())
                    .child("Select Theme"),
            )
            .child(div().py_1().children(rows))
            .child(
                div()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme::border())
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child(format!("Add your own JSON themes to {}", folder.display())),
            )
    }
}

// --- Settings ----------------------------------------------------------------------------

use crate::config::{OpenProjectIn, AutoSave, Settings};

/// Workspace settings: auto save, project tree behaviour, theme and editor font.
#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    Appearance,
    System,
    Editor,
    Keymap,
    VersionControl,
    Terminal,
    Laravel,
    Import,
    AiProviders,
}

/// `(page, group, title, search keywords)` in tree order.
const PAGES: &[(SettingsPage, &str, &str, &str)] = &[
    (SettingsPage::Appearance, "Appearance & Behavior", "Appearance", "theme color dark light"),
    (SettingsPage::System, "Appearance & Behavior", "System Settings", "auto save autosave delay project tree single click select opened updates"),
    (SettingsPage::Keymap, "", "Keymap", "shortcuts keys bindings"),
    (SettingsPage::Editor, "Editor", "Font & AI Completion", "font size family editor ai inline completion ghost"),
    (SettingsPage::VersionControl, "Version Control", "Git & Local History", "git history versions sync gist backup"),
    (SettingsPage::Laravel, "Languages & Frameworks", "Laravel", "laravel php artisan models"),
    (SettingsPage::Terminal, "Tools", "Terminal", "terminal shell zsh bash"),
    (SettingsPage::Import, "Tools", "Import Settings", "import vscode phpstorm zed migrate"),
    (SettingsPage::AiProviders, "Tools", "AI Providers", "ai claude anthropic opencode gemini google antigravity codex agent api key acp"),
];

pub struct SettingsDialog {
    settings: Settings,
    page: SettingsPage,
    search: Entity<TextField>,
    delay: Entity<TextField>,
    font: Entity<TextField>,
    font_size: Entity<TextField>,
    shell: Entity<TextField>,
    api_key: Entity<TextField>,
    model: Entity<TextField>,
    agent: agent::AgentConfig,
    themes: Vec<String>,
    status: Option<String>,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for SettingsDialog {}

/// A password / key field: shown as dots.
fn secret_field(placeholder: &'static str, text: String, cx: &mut Context<SettingsDialog>) -> Entity<TextField> {
    cx.new(|cx| {
        let mut f = TextField::new(placeholder, cx).masked();
        f.set_text(text, cx);
        f
    })
}

fn field(placeholder: &'static str, text: String, cx: &mut Context<SettingsDialog>) -> Entity<TextField> {
    cx.new(|cx| {
        let mut f = TextField::new(placeholder, cx);
        f.set_text(text, cx);
        f
    })
}

impl SettingsDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let delay = field("1000", settings.auto_save_delay_ms.to_string(), cx);
        let font = field("JetBrains Mono", settings.editor_font.clone(), cx);
        let font_size = field("13.5", settings.editor_font_size.to_string(), cx);
        let shell = field("Login shell", settings.terminal_shell.clone(), cx);
        let agent = crate::workspace::load_agent_config();
        let api_key = secret_field("sk-ant-… (or ANTHROPIC_API_KEY)", agent.api_key.clone(), cx);
        let model = field(agent::DEFAULT_MODEL, agent.model.clone(), cx);
        let search = cx.new(|cx| TextField::new("Search settings", cx));
        let subscription = cx.subscribe(&search, |this: &mut Self, _, _: &FieldEvent, cx| {
            if let Some(first) = this.visible_pages(cx).first() {
                this.page = first.0;
            }
            cx.notify();
        });
        let focus_handle = cx.focus_handle();
        window.focus(&search.focus_handle(cx));
        Self {
            settings,
            page: SettingsPage::Appearance,
            search,
            delay,
            font,
            font_size,
            shell,
            api_key,
            model,
            agent,
            themes: theme::available().into_iter().map(|t| t.name).collect(),
            status: None,
            focus_handle,
            _subscription: subscription,
        }
    }

    fn visible_pages(&self, cx: &App) -> Vec<(SettingsPage, &'static str, &'static str)> {
        let query = self.search.read(cx).text().to_ascii_lowercase();
        PAGES
            .iter()
            .filter(|(_, group, title, keywords)| {
                query.is_empty() || format!("{group} {title} {keywords}").to_ascii_lowercase().contains(&query)
            })
            .map(|(page, group, title, _)| (*page, *group, *title))
            .collect()
    }

    fn apply(&mut self, cx: &mut Context<Self>) {
        let settings = &mut self.settings;
        settings.auto_save_delay_ms = self.delay.read(cx).text().trim().parse().unwrap_or(1000);
        settings.editor_font = self.font.read(cx).text().trim().to_string();
        settings.editor_font_size = self.font_size.read(cx).text().trim().parse::<f32>().unwrap_or(13.5).clamp(8., 32.);
        settings.terminal_shell = self.shell.read(cx).text().trim().to_string();
        settings.save();
        self.agent.api_key = self.api_key.read(cx).text().trim().to_string();
        let model = self.model.read(cx).text().trim().to_string();
        self.agent.model = if model.is_empty() { agent::DEFAULT_MODEL.into() } else { model };
        crate::workspace::save_agent_config(&self.agent);
        // Re-picks fonts and applies the theme.
        theme::init(cx);
        self.status = Some("Applied".into());
        cx.notify();
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.apply(cx);
        cx.emit(DialogEvent::Dismiss);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        // Undo a live theme preview.
        theme::init(cx);
        cx.emit(DialogEvent::Dismiss);
    }

    fn render_page(&mut self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let s = &self.settings;
        let row = |label: &'static str| div().flex().items_center().gap_3().child(div().w(px(150.)).flex_none().text_color(theme::muted()).child(label));
        let toggle = |id: &'static str, text: &'static str, on: bool, f: fn(&mut Settings)| {
            super::checkbox(id, text, on).on_click(cx.listener(move |this, _, _, cx| {
                f(&mut this.settings);
                cx.notify();
            }))
        };
        let body = div().flex().flex_col().gap_4();
        match self.page {
            SettingsPage::Appearance => {
                let themes = self.themes.iter().map(|name| {
                    let name = name.clone();
                    option_chip(SharedString::from(format!("theme-{name}")), &name, s.theme == name).on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.theme = name.clone();
                        theme::set(&name, cx);
                        cx.notify();
                    }))
                });
                body.child(row("Theme").child(div().flex().flex_wrap().gap_1().children(themes))).into_any_element()
            }
            SettingsPage::System => {
                let modes = [(AutoSave::Off, "Off"), (AutoSave::OnFocusChange, "On focus change"), (AutoSave::AfterDelay, "After delay")];
                let auto_save = modes.into_iter().map(|(mode, text)| {
                    option_chip(SharedString::from(format!("autosave-{text}")), text, s.auto_save == mode).on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.auto_save = mode;
                        cx.notify();
                    }))
                });
                let places = [(OpenProjectIn::ThisWindow, "This window"), (OpenProjectIn::NewWindow, "New window"), (OpenProjectIn::NewTab, "New tab")];
                let open_in = places.into_iter().map(|(place, text)| {
                    option_chip(SharedString::from(format!("open-in-{text}")), text, s.open_project_in == place).on_click(cx.listener(move |this, _, _, cx| {
                        this.settings.open_project_in = place;
                        cx.notify();
                    }))
                });
                body.child(row("Open projects in").child(div().flex().gap_1().children(open_in)))
                    .child(row("Updates").child(toggle("check-updates", "Check for updates at startup", s.check_for_updates, |s| s.check_for_updates ^= true)))
                    .child(row("Auto save").child(div().flex().gap_1().children(auto_save)))
                    .child(
                        row("Actions on save").child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(toggle("reformat-on-save", "Reformat code (Pint / PHP CS Fixer)", s.reformat_on_save, |s| s.reformat_on_save ^= true))
                                .child(toggle("optimize-on-save", "Optimize imports", s.optimize_imports_on_save, |s| s.optimize_imports_on_save ^= true)),
                        ),
                    )
                    .when(s.auto_save == AutoSave::AfterDelay, |d| d.child(row("Delay (ms)").child(div().w(px(120.)).child(self.delay.clone()))))
                    .child(
                        row("Project tree").child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_2()
                                .child(toggle("single-click", "Open files with a single click", s.open_on_single_click, |s| s.open_on_single_click ^= true))
                                .child(toggle("select-opened", "Always select opened file", s.select_opened_file, |s| s.select_opened_file ^= true)),
                        ),
                    )
                    .into_any_element()
            }
            SettingsPage::Editor => body
                .child(row("Font").child(div().flex_1().child(self.font.clone())))
                .child(row("Size").child(div().w(px(90.)).child(self.font_size.clone())))
                .child(div().text_size(px(12.)).text_color(theme::muted()).child("Used by the editor and the terminal."))
                .child(row("AI completion").child(toggle(
                    "ai-inline",
                    "Suggest code inline as you type (Tab accepts, Esc dismisses)",
                    s.ai_inline_completion,
                    |s| s.ai_inline_completion ^= true,
                )))
                .child(div().text_size(px(12.)).text_color(theme::muted()).child("Uses the Anthropic API key from the AI Assistant panel; the model is completion_model in agent.json."))
                .into_any_element(),
            SettingsPage::Keymap => body
                .child(div().text_color(theme::muted()).child(format!("Custom shortcuts are stored in {}", keymap::user_path().display())))
                .child(div().flex().child(button("settings-keymap", "Configure Keymap…", false).on_click(cx.listener(|_, _, window, cx| {
                    cx.emit(DialogEvent::Dismiss);
                    window.dispatch_action(Box::new(crate::workspace::OpenKeymap), cx);
                }))))
                .into_any_element(),
            SettingsPage::VersionControl => body
                .child(row("Local History").child(div().flex().gap_1().children([5usize, 10, 20, 50].into_iter().map(|n| {
                    option_chip(SharedString::from(format!("history-{n}")), &format!("{n} versions"), s.local_history_versions == n).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.settings.local_history_versions = n;
                            cx.notify();
                        },
                    ))
                }))))
                .child(
                    row("Settings Sync")
                        .child(div().flex_1().text_size(px(12.)).text_color(theme::muted()).child(match crate::sync::gist_id() {
                            Some(id) => format!("GitHub gist {id}"),
                            None => "Secret GitHub gist via the gh CLI".to_string(),
                        }))
                        .child(button("sync-pull", "Pull", false).on_click(cx.listener(|_, _, window, cx| {
                            window.dispatch_action(Box::new(crate::workspace::SyncPull), cx);
                            cx.emit(DialogEvent::Dismiss);
                        })))
                        .child(button("sync-push", "Push", false).on_click(cx.listener(|_, _, window, cx| {
                            window.dispatch_action(Box::new(crate::workspace::SyncPush), cx);
                            cx.emit(DialogEvent::Dismiss);
                        }))),
                )
                .into_any_element(),
            SettingsPage::Terminal => body
                .child(row("Shell path").child(div().flex_1().child(self.shell.clone())))
                .child(div().text_size(px(12.)).text_color(theme::muted()).child("Empty starts your login shell. Arguments may follow the path, e.g. /bin/zsh -l"))
                .into_any_element(),
            SettingsPage::Laravel => body
                .child(div().text_color(theme::muted()).child("Laravel options are stored per project in .phpcraft/laravel.json."))
                .child(div().flex().child(button("settings-laravel", "Laravel Settings…", false).on_click(cx.listener(|_, _, window, cx| {
                    cx.emit(DialogEvent::Dismiss);
                    window.dispatch_action(Box::new(crate::workspace::OpenLaravelSettings), cx);
                }))))
                .into_any_element(),
            SettingsPage::AiProviders => {
                let current = self.agent.provider.clone();
                let provider_row = |id: &'static str, title: &'static str, detail: String, installed: bool, install: &'static str, login: Option<String>| {
                    let active = current == id;
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .p_2()
                        .rounded_md()
                        .border_1()
                        .border_color(if active { theme::accent() } else { theme::border() })
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_color(theme::ui_text()).child(title))
                                .child(div().text_size(px(11.)).text_color(if installed { theme::success() } else { theme::muted() }).child(if installed { "✓ available" } else { "not installed" }))
                                .child(div().flex_1())
                                .when(installed && !active, |d| {
                                    d.child(option_chip(SharedString::from(format!("ai-use-{id}")), "Use by default", false).on_click(cx.listener(move |this, _, _, cx| {
                                        this.agent.provider = id.to_string();
                                        cx.notify();
                                    })))
                                })
                                .when(active, |d| d.child(div().text_size(px(12.)).text_color(theme::accent()).child("Default")))
                                .when_some(login.filter(|_| installed), |d, login| {
                                    d.child(option_chip(SharedString::from(format!("ai-login-{id}")), "Sign in…", false).on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(DialogEvent::RunInTerminal(login.clone()));
                                    })))
                                }),
                        )
                        .child(div().text_size(px(12.)).text_color(theme::muted()).child(detail))
                        .when(!installed && !install.is_empty(), |d| {
                            d.child(div().text_size(px(12.)).font_family(theme::editor_font()).text_color(theme::muted()).child(format!("Install: {install}")))
                        })
                };
                let installed = |program: &str| agent::acp::which(program).is_some();
                let key_set = !self.api_key.read(cx).text().trim().is_empty() || std::env::var("ANTHROPIC_API_KEY").is_ok_and(|k| !k.is_empty());
                body.child(div().text_color(theme::muted()).child("Agents for the AI Assistant (⌘⌥I). ACP agents sign in with their own account; Claude API uses your key."))
                    .child(
                        provider_row("anthropic", "Claude API", "Anthropic Messages API with your key. Also used for inline AI completion.".into(), key_set, "", None)
                            .child(row("API key").child(div().flex_1().child(self.api_key.clone())))
                            .child(row("Model").child(div().flex_1().child(self.model.clone()))),
                    )
                    .child(provider_row("Claude Code", "Claude Code", "Your Claude subscription through the Claude Code ACP adapter.".into(), installed("claude-code-acp"), "npm install -g @zed-industries/claude-code-acp", Some("claude /login".into())))
                    .child(provider_row("OpenCode", "OpenCode", "OpenCode (opencode acp), any model or the OpenCode subscription.".into(), installed("opencode"), "curl -fsSL https://opencode.ai/install | bash", Some("opencode auth login".into())))
                    .child(provider_row("Gemini", "Gemini · Google AI (Antigravity)", "Your Google AI subscription through Gemini CLI (gemini --experimental-acp).".into(), installed("gemini"), "npm install -g @google/gemini-cli", Some("gemini".into())))
                    .child(provider_row("Codex", "Codex", "OpenAI Codex through the codex-acp adapter (ChatGPT sign-in).".into(), installed("codex-acp"), "npm install -g @zed-industries/codex-acp", Some("codex login".into())))
                    .into_any_element()
            }
            SettingsPage::Import => {
                let sources = crate::settings_import::Source::ALL.into_iter().map(|source| {
                    let location = source.locate();
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().w(px(100.)).text_color(theme::ui_text()).child(source.name()))
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.))
                                .text_color(theme::muted())
                                .child(location.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "Not found".into())),
                        )
                        .when(location.is_some(), |d| {
                            d.child(button(source.name(), "Import", false).on_click(cx.listener(move |this, _, _, cx| {
                                match crate::settings_import::import(source) {
                                    Ok(report) => {
                                        theme::init(cx);
                                        keymap::apply(cx);
                                        this.settings = Settings::load();
                                        this.status = Some(format!(
                                            "{}: {} setting(s), {} shortcut(s) imported",
                                            source.name(),
                                            report.settings.len(),
                                            report.bindings.len()
                                        ));
                                    }
                                    Err(err) => this.status = Some(err),
                                }
                                cx.notify();
                            })))
                        })
                });
                body.child(div().text_color(theme::muted()).child("Fonts, theme, auto save, terminal shell and keyboard shortcuts."))
                    .children(sources)
                    .into_any_element()
            }
        }
    }
}

impl Focusable for SettingsDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn option_chip(id: SharedString, label: &str, active: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(gpui::ElementId::Name(id))
        .px_2()
        .py(px(3.))
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .border_color(if active { theme::accent() } else { theme::border_strong() })
        .when(active, |d| d.bg(theme::active_row()))
        .hover(|d| d.bg(theme::hover()))
        .child(label.to_string())
}

impl Render for SettingsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tree: Vec<gpui::AnyElement> = Vec::new();
        let mut last_group = "";
        for (page, group, title) in self.visible_pages(cx) {
            if group != last_group && !group.is_empty() {
                tree.push(div().px_2().pt_2().pb_1().text_size(px(12.)).text_color(theme::muted()).child(group).into_any_element());
            }
            last_group = group;
            let active = page == self.page;
            tree.push(
                div()
                    .id(SharedString::from(format!("settings-page-{title}")))
                    .pl(px(if group.is_empty() { 8. } else { 18. }))
                    .pr_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .text_color(theme::ui_text())
                    .when(active, |d| d.bg(theme::selection()))
                    .when(!active, |d| d.hover(|d| d.bg(theme::hover())))
                    .child(title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.page = page;
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let (group, title) = PAGES.iter().find(|p| p.0 == self.page).map(|p| (p.1, p.2)).unwrap_or(("", ""));
        let page = self.render_page(cx);
        card(820.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(
                div()
                    .flex()
                    .h(px(460.))
                    .child(
                        div()
                            .id("settings-tree")
                            .w(px(230.))
                            .flex_none()
                            .p_2()
                            .border_r_1()
                            .border_color(theme::border())
                            .overflow_y_scroll()
                            .child(div().pb_1().child(self.search.clone()))
                            .children(tree),
                    )
                    .child(
                        div()
                            .id("settings-page")
                            .flex_1()
                            .p_4()
                            .overflow_y_scroll()
                            .child(
                                div()
                                    .pb_3()
                                    .text_size(px(15.))
                                    .text_color(theme::ui_text())
                                    .child(if group.is_empty() { title.to_string() } else { format!("{group}  ›  {title}") }),
                            )
                            .child(page),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .border_t_1()
                    .border_color(theme::border())
                    .child(div().flex_1().text_size(px(12.)).text_color(theme::muted()).child(self.status.clone().unwrap_or_else(|| {
                        format!("Saved to {}", crate::config::dir().join("settings.json").display())
                    })))
                    .child(button("settings-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("settings-apply", "Apply", false).on_click(cx.listener(|this, _, _, cx| this.apply(cx))))
                    .child(button("settings-ok", "OK", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
