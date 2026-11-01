//! The Git tool window (⌘9): branch tree | log with commit graph and filters | commit
//! details, plus a Console tab with the output of git commands.

use gpui::{AnyElement, Context, Hsla, PathBuilder, Pixels, SharedString, canvas, div, point, prelude::*, px};

use super::Workspace;
use super::git_panel::{GitTab, ToggleGitLog};
use crate::dialogs::GitCommand;
use crate::theme;

const ROW: f32 = 22.;
const LANE: f32 = 12.;

/// Lane colours, cycling.
fn lane_color(lane: usize) -> Hsla {
    use php_syntax::Highlight as H;
    let palette = [
        theme::accent(),
        theme::success(),
        theme::syntax(H::Keyword),
        theme::syntax(H::Number),
        theme::syntax(H::Function),
        theme::syntax(H::Tag),
        theme::syntax(H::Variable),
        theme::syntax(H::String),
    ];
    palette[lane % palette.len()]
}

/// Draws one row of the commit graph.
fn graph_cell(row: git::GraphRow, width: usize) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let x = |lane: usize| bounds.left() + px(LANE * lane as f32 + LANE / 2. + 4.);
            let top = bounds.top();
            let mid = bounds.top() + px(ROW / 2.);
            let bottom = bounds.bottom();
            let mut segment = |from: gpui::Point<Pixels>, to: gpui::Point<Pixels>, color: Hsla| {
                let mut path = PathBuilder::stroke(px(1.6));
                path.move_to(from);
                if from.x == to.x {
                    path.line_to(to);
                } else {
                    // A soft curve for lanes merging into / branching from the dot.
                    let ctrl = point(to.x, from.y);
                    path.curve_to(to, ctrl);
                }
                if let Ok(path) = path.build() {
                    window.paint_path(path, color);
                }
            };
            for (a, b) in &row.top {
                segment(point(x(*a), top), point(x(*b), mid), lane_color(*a));
            }
            for (a, b) in &row.bottom {
                segment(point(x(*a), mid), point(x(*b), bottom), lane_color(*b));
            }
            let dot = gpui::Bounds::new(point(x(row.column) - px(4.), mid - px(4.)), gpui::size(px(8.), px(8.)));
            window.paint_quad(gpui::fill(dot, lane_color(row.column)).corner_radii(px(4.)));
        },
    )
    .w(px(LANE * width as f32 + 8.))
    .h(px(ROW))
    .flex_none()
}

#[derive(Default)]
struct Folder {
    children: std::collections::BTreeMap<String, Folder>,
    /// Full branch name when this node is a branch.
    branch: Option<String>,
}

fn build_tree(branches: &[String], query: &str) -> Folder {
    let mut root = Folder::default();
    for branch in branches.iter().filter(|b| query.is_empty() || b.to_ascii_lowercase().contains(query)) {
        let mut node = &mut root;
        let parts: Vec<&str> = branch.split('/').collect();
        for part in &parts[..parts.len() - 1] {
            node = node.children.entry(format!("{part}/")).or_default();
        }
        node.children.entry(parts[parts.len() - 1].to_string()).or_default().branch = Some(branch.clone());
    }
    root
}

impl Workspace {
    fn select_commit(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.git.log_selected = Some(ix);
        let Some(hash) = self.git.log.get(ix).map(|c| c.hash.clone()) else { return };
        let repo = self.intel.repo();
        self.git.log_files = repo.and_then(|r| r.commit_files(&hash).ok()).unwrap_or_default();
        self.git.log_details = repo.and_then(|r| r.commit_details(&hash));
        cx.notify();
    }

    fn filter_branch(&mut self, branch: String, cx: &mut Context<Self>) {
        self.git.filter_branch = branch;
        self.refresh_log(cx);
    }

    fn render_tree_node(&self, key: &str, folder: &Folder, depth: usize, out: &mut Vec<AnyElement>, cx: &mut Context<Self>) {
        let current = self.git.branch.clone();
        for (name, node) in &folder.children {
            let id = format!("{key}/{name}");
            if let Some(branch) = &node.branch {
                let is_current = current.as_deref() == Some(branch.as_str());
                let filtered = self.git.filter_branch == *branch;
                let branch = branch.clone();
                let checkout = branch.clone();
                out.push(
                    div()
                        .id(SharedString::from(id.clone()))
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl(px(8. + 14. * depth as f32))
                        .h(px(22.))
                        .rounded_md()
                        .cursor_pointer()
                        .when(filtered, |d| d.bg(theme::selection()))
                        .hover(|d| d.bg(theme::hover()))
                        .child(div().text_color(if is_current { theme::syntax(php_syntax::Highlight::Warning) } else { theme::muted() }).child(if is_current { "★" } else { "⎇" }))
                        .child(div().text_color(theme::ui_text()).child(SharedString::from(name.clone())))
                        .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                            if event.click_count() >= 2 {
                                this.run_git(GitCommand::Checkout(checkout.clone()), cx);
                            } else {
                                this.filter_branch(branch.clone(), cx);
                            }
                        }))
                        .into_any_element(),
                );
            }
            if !node.children.is_empty() {
                let collapsed = self.git.tree_collapsed.contains(&id);
                let toggle = id.clone();
                out.push(
                    div()
                        .id(SharedString::from(format!("{id}#folder")))
                        .flex()
                        .items_center()
                        .gap_1()
                        .pl(px(8. + 14. * depth as f32))
                        .h(px(22.))
                        .rounded_md()
                        .cursor_pointer()
                        .hover(|d| d.bg(theme::hover()))
                        .child(div().w(px(10.)).text_size(px(9.)).text_color(theme::muted()).child(if collapsed { "▶" } else { "▼" }))
                        .child(div().text_color(theme::ui_text()).child(SharedString::from(name.trim_end_matches('/').to_string())))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if !this.git.tree_collapsed.remove(&toggle) {
                                this.git.tree_collapsed.insert(toggle.clone());
                            }
                            cx.notify();
                        }))
                        .into_any_element(),
                );
                if !collapsed {
                    self.render_tree_node(&id, node, depth + 1, out, cx);
                }
            }
        }
    }

    fn render_branch_tree(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let query = self.git.tree_query.read(cx).text().to_ascii_lowercase();
        let mut rows: Vec<AnyElement> = Vec::new();
        let head_selected = self.git.filter_branch.is_empty();
        rows.push(
            div()
                .id("tree-head")
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .when(head_selected, |d| d.bg(theme::selection()))
                .hover(|d| d.bg(theme::hover()))
                .child("All branches")
                .on_click(cx.listener(|this, _, _, cx| this.filter_branch(String::new(), cx)))
                .into_any_element(),
        );
        for (section, branches) in [("Local", &self.git.branches.0), ("Remote", &self.git.branches.1)] {
            let key = section.to_string();
            let collapsed = self.git.tree_collapsed.contains(&key);
            let toggle = key.clone();
            rows.push(
                div()
                    .id(SharedString::from(format!("section-{section}")))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .h(px(22.))
                    .cursor_pointer()
                    .child(div().w(px(10.)).text_size(px(9.)).text_color(theme::muted()).child(if collapsed { "▶" } else { "▼" }))
                    .child(div().text_color(theme::muted()).child(section))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.git.tree_collapsed.remove(&toggle) {
                            this.git.tree_collapsed.insert(toggle.clone());
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
            if !collapsed {
                let tree = build_tree(branches, &query);
                self.render_tree_node(&key, &tree, 1, &mut rows, cx);
            }
        }
        div()
            .w(px(250.))
            .flex_none()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(theme::border())
            .child(div().p_1().child(self.git.tree_query.clone()))
            .child(div().id("branch-tree").flex_1().overflow_y_scroll().px_1().children(rows))
    }

    fn render_log_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let width = self.git.log_graph.iter().map(|r| r.width).max().unwrap_or(1).min(12);
        let rows: Vec<_> = self
            .git
            .log
            .iter()
            .zip(self.git.log_graph.iter().cloned())
            .enumerate()
            .map(|(ix, (commit, graph))| {
                let selected = self.git.log_selected == Some(ix);
                let merge = commit.parents.len() > 1;
                div()
                    .id(("commit", ix))
                    .flex()
                    .items_center()
                    .gap_2()
                    .pr_2()
                    .h(px(ROW))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(theme::selection()))
                    .hover(|d| d.bg(theme::hover()))
                    .child(graph_cell(graph, width))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .gap_2()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(div().text_color(if merge { theme::muted() } else { theme::ui_text() }).child(commit.subject.clone()))
                            .when(!commit.refs.is_empty(), |d| {
                                d.child(div().text_size(px(11.)).text_color(theme::accent()).child(commit.refs.replace("HEAD -> ", "")))
                            }),
                    )
                    .child(div().w(px(150.)).flex_none().overflow_hidden().whitespace_nowrap().text_color(theme::muted()).child(commit.author.clone()))
                    .child(div().w(px(110.)).flex_none().text_color(theme::muted()).child(commit.date.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_commit(ix, cx)))
            })
            .collect();
        let branch_chip = (!self.git.filter_branch.is_empty()).then(|| {
            div()
                .id("filter-branch")
                .flex()
                .gap_1()
                .px_1p5()
                .rounded_md()
                .bg(theme::hover())
                .cursor_pointer()
                .child(format!("Branch: {}", self.git.filter_branch))
                .child(div().text_color(theme::muted()).child("×"))
                .on_click(cx.listener(|this, _, _, cx| this.filter_branch(String::new(), cx)))
        });
        div()
            .flex_1()
            .min_w(px(0.))
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_1()
                    .child(div().w(px(220.)).child(self.git.filter_text.clone()))
                    .children(branch_chip)
                    .child(div().w(px(130.)).child(self.git.filter_user.clone()))
                    .child(div().w(px(180.)).child(self.git.filter_path.clone()))
                    .child(div().flex_1())
                    .child(div().text_color(theme::muted()).child(format!("{} commits", self.git.log.len()))),
            )
            .child(div().id("log-rows").flex_1().overflow_y_scroll().children(rows))
    }

    fn render_commit_details(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let files: Vec<_> = self
            .git
            .log_files
            .iter()
            .enumerate()
            .map(|(ix, (change, path))| {
                let hash = self.git.log_details.as_ref().map(|d| d.hash.clone()).unwrap_or_default();
                let path = path.clone();
                let (dir, name) = path.rsplit_once('/').map_or((String::new(), path.clone()), |(d, n)| (d.to_string(), n.to_string()));
                div()
                    .id(("details-file", ix))
                    .flex()
                    .gap_1p5()
                    .px_2()
                    .h(px(22.))
                    .items_center()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme::hover()))
                    .child(div().w(px(12.)).text_color(theme::accent()).child(change.letter()))
                    .child(div().text_color(theme::ui_text()).child(name))
                    .child(div().flex_1().overflow_hidden().whitespace_nowrap().text_size(px(11.)).text_color(theme::muted()).child(dir))
                    .on_click(cx.listener(move |this, _, window, cx| this.show_commit_diff_public(&hash, &path, window, cx)))
            })
            .collect();
        let details = self.git.log_details.clone();
        div()
            .w(px(380.))
            .flex_none()
            .flex()
            .flex_col()
            .border_l_1()
            .border_color(theme::border())
            .when(details.is_none(), |d| {
                d.child(div().flex_1().flex().items_center().justify_center().text_color(theme::muted()).child("Select commit to view changes"))
            })
            .when_some(details, |d, details| {
                d.child(div().id("details-files").flex_1().overflow_y_scroll().py_1().children(files)).child(
                    div()
                        .id("details-message")
                        .max_h(px(140.))
                        .overflow_y_scroll()
                        .p_2()
                        .border_t_1()
                        .border_color(theme::border())
                        .child(div().text_color(theme::ui_text()).child(SharedString::from(details.message.clone())))
                        .child(
                            div()
                                .mt_1()
                                .text_size(px(11.))
                                .text_color(theme::muted())
                                .child(format!("{} {} <{}> {}", &details.hash[..details.hash.len().min(10)], details.author, details.email, details.date)),
                        ),
                )
            })
    }

    pub(super) fn render_git_log(&self, cx: &mut Context<Self>) -> Option<impl IntoElement + use<>> {
        let tab = |id: &'static str, label: &'static str, active: bool| {
            div()
                .id(id)
                .px_2()
                .h(px(22.))
                .flex()
                .items_center()
                .rounded_md()
                .cursor_pointer()
                .when(active, |d| d.bg(theme::active_row()).text_color(theme::ui_text()))
                .when(!active, |d| d.text_color(theme::muted()).hover(|d| d.bg(theme::hover())))
                .child(label)
        };
        let log_title: &'static str = "Log";
        let body: AnyElement = match self.git.tab {
            GitTab::Log => div()
                .flex()
                .flex_1()
                .min_h(px(0.))
                .child(self.render_branch_tree(cx))
                .child(self.render_log_list(cx))
                .child(self.render_commit_details(cx))
                .into_any_element(),
            GitTab::Console => div()
                .id("git-console")
                .flex_1()
                .overflow_y_scroll()
                .p_2()
                .font_family(theme::editor_font())
                .children(self.git.console.iter().map(|entry| div().mb_2().text_color(theme::text()).child(SharedString::from(entry.clone()))))
                .into_any_element(),
        };
        Some(
            div()
                .size_full()
                .flex()
                .flex_col()
                .bg(theme::editor_bg())
                .text_size(px(12.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_2()
                        .h(px(30.))
                        .bg(theme::panel_bg())
                        .child(div().pr_2().text_color(theme::ui_text()).child("Git"))
                        .child(tab("git-tab-log", log_title, self.git.tab == GitTab::Log).on_click(cx.listener(|this, _, _, cx| {
                            this.git.tab = GitTab::Log;
                            cx.notify();
                        })))
                        .child(tab("git-tab-console", "Console", self.git.tab == GitTab::Console).on_click(cx.listener(|this, _, _, cx| {
                            this.git.tab = GitTab::Console;
                            cx.notify();
                        })))
                        .child(div().flex_1())
                        .child(div().id("log-refresh").px_1().cursor_pointer().text_color(theme::muted()).child("⟳").on_click(cx.listener(|this, _, _, cx| this.refresh_log(cx))))
                        .child(div().id("log-close").px_1().cursor_pointer().text_color(theme::muted()).child("×").on_click(cx.listener(|this, _, w, cx| this.toggle_git_log(&ToggleGitLog, w, cx)))),
                )
                .child(body),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_tree_groups_folders() {
        let tree = build_tree(&["main".into(), "feature/archive/test-ocr".into(), "feature/lastupdates".into()], "");
        assert!(tree.children["main"].branch.is_some());
        let feature = &tree.children["feature/"];
        assert!(feature.children.contains_key("archive/"));
        assert_eq!(feature.children["lastupdates"].branch.as_deref(), Some("feature/lastupdates"));
        let filtered = build_tree(&["main".into(), "feature/x".into()], "feat");
        assert!(!filtered.children.contains_key("main"));
    }
}
