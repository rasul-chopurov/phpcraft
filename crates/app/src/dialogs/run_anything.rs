//! Run Anything (⌃⌃): one field for saved and detected configurations, Artisan commands,
//! recently typed lines and any shell line.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Window, div,
    prelude::*, px,
};
use laravel::artisan::{self, ArtisanCommand};

use super::{Cancel, Complete, Confirm, DialogEvent, SelectNext, SelectPrev, card};
use crate::i18n::{tr, trf};
use crate::run_configs::{Kind, RunConfig};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

/// Rows kept for the recent lines and for Artisan matches; the rest is reachable by typing.
const HISTORY_LIMIT: usize = 6;
const ARTISAN_LIMIT: usize = 40;

#[derive(Clone)]
enum Row {
    Header(&'static str),
    Config(RunConfig),
    History(String),
    Artisan(ArtisanCommand),
    /// The typed line itself, run through the shell.
    Shell(String),
    Edit,
}

pub struct RunAnythingDialog {
    root: PathBuf,
    saved: Vec<RunConfig>,
    detected: Vec<RunConfig>,
    history: Vec<String>,
    commands: Option<Arc<Vec<ArtisanCommand>>>,
    query: Entity<TextField>,
    selected: usize,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for RunAnythingDialog {}

fn icon(config: &RunConfig) -> &'static str {
    match config.kind {
        Kind::Test { .. } => "◉",
        Kind::Npm { .. } => "⬢",
        Kind::Composer { .. } => "♪",
        Kind::Artisan { .. } => "◈",
        Kind::Shell { .. } => "$",
        Kind::Php { .. } => "▶",
    }
}

impl RunAnythingDialog {
    pub fn new(
        root: &std::path::Path,
        saved: Vec<RunConfig>,
        detected: Vec<RunConfig>,
        history: Vec<String>,
        cached: Option<Arc<Vec<ArtisanCommand>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let query = cx.new(|cx| TextField::new(tr("Type a command, a configuration name or a shell line"), cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = 0;
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        if cached.is_none() {
            let root = root.to_path_buf();
            cx.spawn(async move |this, cx| {
                let result = cx
                    .background_executor()
                    .spawn(async move { artisan::list_commands(&root) })
                    .await;
                this.update(cx, |this, cx| {
                    if let Ok(list) = result {
                        let list = Arc::new(list);
                        this.commands = Some(list.clone());
                        cx.emit(DialogEvent::CommandsLoaded(list));
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        Self {
            root: root.to_path_buf(),
            saved,
            detected,
            history,
            commands: cached,
            query,
            selected: 0,
            focus_handle: cx.focus_handle(),
            _subscription: subscription,
        }
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let typed = self.query.read(cx).text().trim().to_string();
        let needle = typed.to_ascii_lowercase();
        let hit = |text: &str| needle.is_empty() || text.to_ascii_lowercase().contains(&needle);
        let mut rows = Vec::new();

        let history: Vec<String> = self
            .history
            .iter()
            .filter(|line| hit(line))
            .take(HISTORY_LIMIT)
            .cloned()
            .collect();
        if !history.is_empty() {
            rows.push(Row::Header("Recent"));
            rows.extend(history.into_iter().map(Row::History));
        }
        let saved: Vec<RunConfig> = self.saved.iter().filter(|c| hit(&c.name)).cloned().collect();
        if !saved.is_empty() {
            rows.push(Row::Header("Saved"));
            rows.extend(saved.into_iter().map(Row::Config));
        }
        let detected: Vec<RunConfig> = self
            .detected
            .iter()
            .filter(|c| !self.saved.iter().any(|s| s.name == c.name))
            .filter(|c| hit(&c.name))
            .cloned()
            .collect();
        if !detected.is_empty() {
            rows.push(Row::Header("Detected"));
            rows.extend(detected.into_iter().map(Row::Config));
        }
        if let Some(commands) = &self.commands {
            let found: Vec<ArtisanCommand> = commands
                .iter()
                .filter(|c| hit(&c.name) || hit(&c.description))
                .take(ARTISAN_LIMIT)
                .cloned()
                .collect();
            if !found.is_empty() {
                rows.push(Row::Header("Artisan"));
                rows.extend(found.into_iter().map(Row::Artisan));
            }
        }
        if !typed.is_empty() {
            rows.push(Row::Shell(typed));
        }
        rows.push(Row::Edit);
        rows
    }

    fn activate(&mut self, ix: usize, save: bool, cx: &mut Context<Self>) {
        match self.rows(cx).into_iter().nth(ix) {
            Some(Row::Config(config)) => cx.emit(DialogEvent::Run { config, save }),
            Some(Row::History(line)) | Some(Row::Shell(line)) => cx.emit(DialogEvent::RunAnything(line)),
            Some(Row::Artisan(command)) => cx.emit(DialogEvent::RunAnything(command.name)),
            Some(Row::Edit) => cx.emit(DialogEvent::EditRunConfigurations),
            _ => {}
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let rows = self.rows(cx);
        let mut ix = self.selected as isize;
        loop {
            ix += delta;
            if ix < 0 || ix as usize >= rows.len() {
                return;
            }
            if !matches!(rows[ix as usize], Row::Header(_)) {
                self.selected = ix as usize;
                cx.notify();
                return;
            }
        }
    }

    /// Tab: put the selected Artisan command into the field so its arguments can be typed.
    fn complete(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(Row::Artisan(command)) = self.rows(cx).into_iter().nth(self.selected) {
            let text = format!("{} ", command.name);
            self.query.update(cx, |q, cx| q.set_text(text, cx));
        }
    }

    fn detail(&self, config: &RunConfig) -> String {
        let command = crate::run_configs::command(config, &self.root);
        format!("{} {}", command.program, command.args.join(" "))
    }
}

impl Focusable for RunAnythingDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RunAnythingDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        let rows: Vec<gpui::AnyElement> = self
            .rows(cx)
            .into_iter()
            .enumerate()
            .map(|(ix, row)| {
                let item = div()
                    .id(("run-anything-row", ix))
                    .mx_1()
                    .px_2()
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .cursor_pointer()
                    .when(ix == selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = ix;
                        this.activate(ix, false, cx);
                    }));
                match row {
                    Row::Header(title) => div()
                        .px_3()
                        .pt_1p5()
                        .pb_1()
                        .text_size(px(12.))
                        .text_color(theme::muted())
                        .child(tr(title))
                        .into_any_element(),
                    Row::Config(config) => {
                        let detail = self.detail(&config);
                        let icon = icon(&config);
                        item.child(div().w(px(16.)).text_color(theme::success()).child(icon))
                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(config.name.clone()))
                            .child(div().max_w(px(180.)).overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(theme::muted()).child(detail))
                            .into_any_element()
                    }
                    Row::History(line) => item
                        .child(div().w(px(16.)).text_color(theme::muted()).child("↻"))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(line))
                        .child(div().text_size(px(11.)).text_color(theme::muted()).child(tr("recent")))
                        .into_any_element(),
                    Row::Artisan(command) => {
                        let description = command.description.clone();
                        item.child(div().w(px(16.)).text_color(theme::success()).child("◈"))
                            .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(command.name.clone()))
                            .child(div().max_w(px(220.)).overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(theme::muted()).child(description))
                            .into_any_element()
                    }
                    Row::Shell(line) => item
                        .child(div().w(px(16.)).text_color(theme::link()).child("$"))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(trf("Run '{}' in the terminal", &[&line])))
                        .into_any_element(),
                    Row::Edit => item
                        .mt_1()
                        .child(div().w(px(16.)).text_color(theme::muted()).child("✎"))
                        .child(tr("Edit Configurations…"))
                        .into_any_element(),
                }
            })
            .collect();
        card(560.)
            .key_context("Dialog RunAnythingDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Confirm, _, cx| this.activate(this.selected, false, cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Dismiss)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_action(cx.listener(|this, _: &Complete, window, cx| this.complete(window, cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "enter" && event.keystroke.modifiers.shift {
                    this.activate(this.selected, true, cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().px_3().pt_2().text_size(px(12.)).text_color(theme::muted()).child(tr("Run Anything")))
            .child(div().p_2().child(self.query.clone()))
            .child(
                div()
                    .px_3()
                    .pb_1()
                    .text_size(px(11.))
                    .text_color(theme::muted())
                    .child(tr("↵ run · ⇥ complete an Artisan command · ⇧↵ save a configuration")),
            )
            .child(div().id("run-anything-list").max_h(px(460.)).overflow_y_scroll().pb_1().text_size(px(13.)).children(rows))
    }
}
