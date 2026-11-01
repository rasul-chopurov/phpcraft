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

use crate::config::{AutoSave, Settings};

/// Workspace settings: auto save, project tree behaviour, theme and editor font.
pub struct SettingsDialog {
    settings: Settings,
    delay: Entity<TextField>,
    font: Entity<TextField>,
    themes: Vec<String>,
    focus_handle: FocusHandle,
}

impl EventEmitter<DialogEvent> for SettingsDialog {}

impl SettingsDialog {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = Settings::load();
        let delay_text = settings.auto_save_delay_ms.to_string();
        let font_text = settings.editor_font.clone();
        let delay = cx.new(|cx| {
            let mut f = TextField::new("1000", cx);
            f.set_text(delay_text, cx);
            f
        });
        let font = cx.new(|cx| {
            let mut f = TextField::new("JetBrains Mono", cx);
            f.set_text(font_text, cx);
            f
        });
        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle);
        Self {
            settings,
            delay,
            font,
            themes: theme::available().into_iter().map(|t| t.name).collect(),
            focus_handle,
        }
    }

    fn confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        let mut settings = self.settings.clone();
        settings.auto_save_delay_ms = self.delay.read(cx).text().trim().parse().unwrap_or(1000);
        settings.editor_font = self.font.read(cx).text().trim().to_string();
        settings.save();
        // Re-picks the editor font and applies the theme.
        theme::init(cx);
        cx.emit(DialogEvent::Dismiss);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DialogEvent::Dismiss);
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
        let s = &self.settings;
        let label = |text: &'static str| div().w(px(170.)).flex_none().text_color(theme::muted()).child(text);
        let modes = [(AutoSave::Off, "Off"), (AutoSave::OnFocusChange, "On focus change"), (AutoSave::AfterDelay, "After delay")];
        let auto_save = modes.into_iter().map(|(mode, text)| {
            option_chip(SharedString::from(format!("autosave-{text}")), text, s.auto_save == mode).on_click(cx.listener(move |this, _, _, cx| {
                this.settings.auto_save = mode;
                cx.notify();
            }))
        });
        let themes = self.themes.iter().map(|name| {
            let name = name.clone();
            let active = s.theme == name;
            option_chip(SharedString::from(format!("theme-{name}")), &name, active).on_click(cx.listener(move |this, _, _, cx| {
                this.settings.theme = name.clone();
                theme::set(&name, cx);
                cx.notify();
            }))
        });
        let toggle = |id: &'static str, text: &'static str, on: bool, f: fn(&mut Settings)| {
            super::checkbox(id, text, on).on_click(cx.listener(move |this, _, _, cx| {
                f(&mut this.settings);
                cx.notify();
            }))
        };
        card(620.)
            .key_context("Dialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .child(div().px_3().py_2().border_b_1().border_color(theme::border()).child("Settings"))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .p_3()
                    .child(div().flex().items_center().gap_2().child(label("Auto save")).child(div().flex().gap_1().children(auto_save)))
                    .when(s.auto_save == AutoSave::AfterDelay, |d| {
                        d.child(div().flex().items_center().gap_2().child(label("Delay (ms)")).child(div().w(px(120.)).child(self.delay.clone())))
                    })
                    .child(div().flex().items_center().gap_2().child(label("Project tree")).child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_2()
                            .child(toggle("single-click", "Open files with a single click", s.open_on_single_click, |s| s.open_on_single_click ^= true))
                            .child(toggle("select-opened", "Always select opened file", s.select_opened_file, |s| s.select_opened_file ^= true)),
                    ))
                    .child(div().flex().items_start().gap_2().child(label("Theme")).child(div().flex().flex_wrap().gap_1().children(themes)))
                    .child(div().flex().items_center().gap_2().child(label("Editor font")).child(div().flex_1().child(self.font.clone())))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(label("Settings Sync"))
                            .child(
                                div()
                                    .flex_1()
                                    .text_size(px(12.))
                                    .text_color(theme::muted())
                                    .child(match crate::sync::gist_id() {
                                        Some(id) => format!("GitHub gist {id}"),
                                        None => "Secret GitHub gist via the gh CLI".to_string(),
                                    }),
                            )
                            .child(button("sync-pull", "Pull", false).on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(crate::workspace::SyncPull), cx);
                                cx.emit(DialogEvent::Dismiss);
                            })))
                            .child(button("sync-push", "Push", false).on_click(cx.listener(|_, _, window, cx| {
                                window.dispatch_action(Box::new(crate::workspace::SyncPush), cx);
                                cx.emit(DialogEvent::Dismiss);
                            }))),
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
                    .child(div().flex_1().text_size(px(11.)).text_color(theme::muted()).child(format!("Saved to {}", crate::config::dir().join("settings.json").display())))
                    .child(button("settings-cancel", "Cancel", false).on_click(cx.listener(|this, _, window, cx| this.cancel(&Cancel, window, cx))))
                    .child(button("settings-ok", "OK", true).on_click(cx.listener(|this, _, window, cx| this.confirm(&Confirm, window, cx)))),
            )
    }
}
