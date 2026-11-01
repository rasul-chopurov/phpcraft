//! The Services tool window: Docker containers (this project's Compose ones first), images and
//! volumes. Containers start, stop and restart from here; Logs and Shell open Run / terminal
//! tabs; Compose Up / Down and Sail work on the project.

use gpui::{Context, div, prelude::*, px};

use super::Workspace;
use crate::docker::{self, Container, Image};
use crate::run_configs::{Kind, RunConfig};
use crate::theme;

#[derive(Default, Clone, Copy, PartialEq)]
pub(super) enum ServicesTab {
    #[default]
    Containers,
    Images,
    Volumes,
}

#[derive(Default)]
pub(super) struct ServicesState {
    tab: ServicesTab,
    containers: Vec<Container>,
    images: Vec<Image>,
    volumes: Vec<String>,
    error: Option<String>,
    loading: bool,
    loaded: bool,
}

impl Workspace {
    /// Loads the lists in the background.
    pub(super) fn refresh_services(&mut self, cx: &mut Context<Self>) {
        self.services.loading = true;
        self.services.loaded = true;
        cx.spawn(async move |this, cx| {
            let (containers, images, volumes) = cx.background_executor().spawn(async move { (docker::containers(), docker::images(), docker::volumes()) }).await;
            this.update(cx, |this, cx| {
                let s = &mut this.services;
                s.loading = false;
                s.error = containers.as_ref().err().cloned();
                s.containers = containers.unwrap_or_default();
                s.images = images.unwrap_or_default();
                s.volumes = volumes.unwrap_or_default();
                cx.notify();
            })
            .ok();
        })
        .detach();
        cx.notify();
    }

    pub(super) fn ensure_services(&mut self, cx: &mut Context<Self>) {
        if !self.services.loaded {
            self.refresh_services(cx);
        }
    }

    fn container_action(&mut self, action: &'static str, id: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_executor().spawn(async move { docker::container_action(action, &id) }).await;
            this.update(cx, |this, cx| {
                if let Err(e) = result {
                    this.services.error = Some(e);
                }
                this.refresh_services(cx);
            })
            .ok();
        })
        .detach();
    }

    fn run_shell(&mut self, name: &str, command: String, cx: &mut Context<Self>) {
        let config = RunConfig { name: name.to_string(), kind: Kind::Shell { command }, env: Default::default() };
        self.start_run(config, false, cx);
    }

    pub(super) fn render_services(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let root = self.project.root().to_path_buf();
        let project = docker::compose_project(&root);
        let has_compose = docker::has_compose_file(&root);
        let has_sail = root.join("vendor/bin/sail").is_file();
        let s = &self.services;
        let link = |id: gpui::SharedString, label: &'static str| {
            div().id(id).px_1p5().rounded_sm().text_color(theme::accent()).cursor_pointer().hover(|d| d.bg(theme::hover())).child(label)
        };
        let tab = |id: &'static str, label: &'static str, t: ServicesTab| {
            div()
                .id(id)
                .px_2()
                .py_0p5()
                .rounded_sm()
                .cursor_pointer()
                .when(s.tab == t, |d| d.bg(theme::selection()))
                .hover(|d| d.bg(theme::hover()))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.services.tab = t;
                    cx.notify();
                }))
        };
        let toolbar = div()
            .flex()
            .items_center()
            .gap_1()
            .px_2()
            .h(px(30.))
            .border_b_1()
            .border_color(theme::border())
            .child(tab("svc-containers", "Containers", ServicesTab::Containers))
            .child(tab("svc-images", "Images", ServicesTab::Images))
            .child(tab("svc-volumes", "Volumes", ServicesTab::Volumes))
            .child(div().flex_1())
            .when(has_compose, |d| {
                d.child(link("svc-up".into(), "Compose Up").on_click(cx.listener(|this, _, _, cx| this.run_shell("compose up", "docker compose up -d".into(), cx))))
                    .child(link("svc-down".into(), "Compose Down").on_click(cx.listener(|this, _, _, cx| this.run_shell("compose down", "docker compose down".into(), cx))))
            })
            .when(has_sail, |d| {
                d.child(link("svc-sail-up".into(), "Sail Up").on_click(cx.listener(|this, _, _, cx| this.run_shell("sail up", "./vendor/bin/sail up -d".into(), cx))))
                    .child(link("svc-sail-down".into(), "Sail Stop").on_click(cx.listener(|this, _, _, cx| this.run_shell("sail stop", "./vendor/bin/sail stop".into(), cx))))
            })
            .child(link("svc-refresh".into(), if s.loading { "Refreshing…" } else { "Refresh" }).on_click(cx.listener(|this, _, _, cx| this.refresh_services(cx))));

        let body: Vec<gpui::AnyElement> = match s.tab {
            ServicesTab::Containers => {
                let mut list = s.containers.clone();
                // This project's Compose containers first, running before stopped.
                list.sort_by_key(|c| (c.project.as_deref() != Some(project.as_str()), c.state != "running", c.name.clone()));
                list.into_iter()
                    .enumerate()
                    .map(|(ix, c)| {
                        let running = c.state == "running";
                        let (id1, id2, id3, name1, name2) = (c.id.clone(), c.id.clone(), c.id.clone(), c.name.clone(), c.name.clone());
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_0p5()
                            .child(div().w(px(10.)).h(px(10.)).rounded_full().bg(if running { theme::success() } else { theme::muted() }))
                            .child(div().min_w(px(180.)).text_color(theme::ui_text()).child(c.name.clone()))
                            .child(div().min_w(px(160.)).text_color(theme::muted()).child(c.image.clone()))
                            .child(div().flex_1().text_color(theme::muted()).overflow_hidden().child(format!("{}  {}", c.status, c.ports)))
                            .child(if running {
                                link(format!("svc-stop-{ix}").into(), "Stop").on_click(cx.listener(move |this, _, _, cx| this.container_action("stop", id1.clone(), cx))).into_any_element()
                            } else {
                                link(format!("svc-start-{ix}").into(), "Start").on_click(cx.listener(move |this, _, _, cx| this.container_action("start", id1.clone(), cx))).into_any_element()
                            })
                            .child(link(format!("svc-restart-{ix}").into(), "Restart").on_click(cx.listener(move |this, _, _, cx| this.container_action("restart", id2.clone(), cx))))
                            .child(link(format!("svc-logs-{ix}").into(), "Logs").on_click(cx.listener(move |this, _, _, cx| this.run_shell(&format!("logs {name1}"), format!("docker logs -f --tail 300 {id3}"), cx))))
                            .when(running, |d| {
                                d.child(link(format!("svc-shell-{ix}").into(), "Shell").on_click(cx.listener(move |this, _, _, cx| {
                                    this.run_shell(&format!("shell {name2}"), format!("docker exec -it {name2} sh -c 'bash || sh'"), cx)
                                })))
                            })
                            .into_any_element()
                    })
                    .collect()
            }
            ServicesTab::Images => s
                .images
                .iter()
                .map(|i| {
                    div()
                        .flex()
                        .gap_3()
                        .px_2()
                        .py_0p5()
                        .child(div().min_w(px(260.)).text_color(theme::ui_text()).child(format!("{}:{}", i.repository, i.tag)))
                        .child(div().min_w(px(120.)).text_color(theme::muted()).child(i.id.clone()))
                        .child(div().text_color(theme::muted()).child(i.size.clone()))
                        .into_any_element()
                })
                .collect(),
            ServicesTab::Volumes => s.volumes.iter().map(|v| div().px_2().py_0p5().child(v.clone()).into_any_element()).collect(),
        };
        div()
            .size_full()
            .flex()
            .flex_col()
            .text_size(px(12.5))
            .child(toolbar)
            .children(s.error.clone().map(|e| div().px_2().py_1().text_color(theme::error()).child(e)))
            .child(div().id("svc-body").flex_1().min_h(px(0.)).overflow_y_scroll().children(body))
    }
}
