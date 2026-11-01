//! Run… popup (⌃⌥R): saved, detected and context run configurations.

use gpui::{App, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Window, div, prelude::*, px};

use super::{Cancel, Confirm, DialogEvent, SelectNext, SelectPrev, card};
use crate::run_configs::{Kind, RunConfig};
use crate::text_field::{FieldEvent, TextField};
use crate::theme;

#[derive(Clone)]
enum Row {
    Header(&'static str),
    Config(RunConfig, bool),
    Edit,
}

pub struct RunDialog {
    context: Option<RunConfig>,
    saved: Vec<RunConfig>,
    detected: Vec<RunConfig>,
    query: Entity<TextField>,
    selected: usize,
    focus_handle: FocusHandle,
    _subscription: Subscription,
}

impl EventEmitter<DialogEvent> for RunDialog {}

fn icon(config: &RunConfig) -> &'static str {
    match config.kind {
        Kind::Test { .. } => "◉",
        Kind::Npm { .. } => "⬢",
        Kind::Composer { .. } => "♪",
        Kind::Artisan { .. } => "◈",
        _ => "▶",
    }
}

impl RunDialog {
    pub fn new(context: Option<RunConfig>, saved: Vec<RunConfig>, detected: Vec<RunConfig>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| TextField::new("Run configuration", cx));
        let subscription = cx.subscribe(&query, |this: &mut Self, _, _: &FieldEvent, cx| {
            this.selected = this.rows(cx).iter().position(|r| !matches!(r, Row::Header(_))).unwrap_or(0);
            cx.notify();
        });
        window.focus(&query.focus_handle(cx));
        let mut dialog = Self { context, saved, detected, query, selected: 0, focus_handle: cx.focus_handle(), _subscription: subscription };
        dialog.selected = dialog.rows(cx).iter().position(|r| !matches!(r, Row::Header(_))).unwrap_or(0);
        dialog
    }

    fn rows(&self, cx: &App) -> Vec<Row> {
        let query = self.query.read(cx).text().to_ascii_lowercase();
        let matches = |c: &RunConfig| query.is_empty() || c.name.to_ascii_lowercase().contains(&query);
        let mut rows = Vec::new();
        let mut section = |title: &'static str, configs: Vec<&RunConfig>, saved: bool| {
            let configs: Vec<_> = configs.into_iter().filter(|c| matches(c)).collect();
            if !configs.is_empty() {
                rows.push(Row::Header(title));
                rows.extend(configs.into_iter().map(|c| Row::Config(c.clone(), saved)));
            }
        };
        section("Context", self.context.iter().collect(), false);
        section("Saved", self.saved.iter().collect(), true);
        section("Detected", self.detected.iter().filter(|d| !self.saved.iter().any(|s| s.name == d.name)).collect(), false);
        rows.push(Row::Edit);
        rows
    }

    fn activate(&mut self, ix: usize, save: bool, cx: &mut Context<Self>) {
        match self.rows(cx).into_iter().nth(ix) {
            Some(Row::Config(config, _)) => cx.emit(DialogEvent::Run { config, save }),
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
}

impl Focusable for RunDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for RunDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected;
        let rows: Vec<gpui::AnyElement> = self
            .rows(cx)
            .into_iter()
            .enumerate()
            .map(|(ix, row)| {
                let item = div()
                    .id(("run-row", ix))
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
                        .child(title)
                        .into_any_element(),
                    Row::Config(config, saved) => item
                        .child(div().w(px(16.)).text_color(theme::success()).child(icon(&config)))
                        .child(div().flex_1().overflow_hidden().whitespace_nowrap().child(config.name.clone()))
                        .when(!saved, |d| d.child(div().text_size(px(11.)).text_color(theme::muted()).child("⇧↵ save")))
                        .into_any_element(),
                    Row::Edit => item
                        .mt_1()
                        .child(div().w(px(16.)).text_color(theme::muted()).child("✎"))
                        .child("Edit Configurations…")
                        .into_any_element(),
                }
            })
            .collect();
        card(440.)
            .key_context("ListDialog")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|this, _: &Confirm, _, cx| this.activate(this.selected, false, cx)))
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DialogEvent::Dismiss)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.step(-1, cx)))
            .on_action(cx.listener(|this, _: &SelectNext, _, cx| this.step(1, cx)))
            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                if event.keystroke.key == "enter" && event.keystroke.modifiers.shift {
                    this.activate(this.selected, true, cx);
                    cx.stop_propagation();
                }
            }))
            .child(div().px_3().pt_2().text_size(px(12.)).text_color(theme::muted()).child("Run"))
            .child(div().p_2().child(self.query.clone()))
            .child(div().id("run-list").max_h(px(460.)).overflow_y_scroll().pb_1().text_size(px(13.)).children(rows))
    }
}
