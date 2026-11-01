//! Tools ▸ Composer: install, update, require / remove a package, dump the autoloader,
//! validate composer.json, list outdated packages. Each runs in a Run tab.

use gpui::{AppContext, Context, Window, actions};

use super::Workspace;
use crate::dialogs::{FileOp, InputDialog};
use crate::run_configs::{Kind, RunConfig};

actions!(
    workspace,
    [ComposerInstall, ComposerUpdate, ComposerRequire, ComposerRemove, ComposerDumpAutoload, ComposerValidate, ComposerOutdated]
);

impl Workspace {
    pub(super) fn run_composer(&mut self, args: &str, cx: &mut Context<Self>) {
        let config = RunConfig {
            name: format!("composer {}", args.split_whitespace().next().unwrap_or("")),
            kind: Kind::Shell { command: format!("composer {args}") },
            env: Default::default(),
        };
        self.start_run(config, false, cx);
    }

    pub(super) fn composer_install(&mut self, _: &ComposerInstall, _: &mut Window, cx: &mut Context<Self>) {
        self.run_composer("install", cx);
    }

    pub(super) fn composer_update(&mut self, _: &ComposerUpdate, _: &mut Window, cx: &mut Context<Self>) {
        self.run_composer("update", cx);
    }

    pub(super) fn composer_dump_autoload(&mut self, _: &ComposerDumpAutoload, _: &mut Window, cx: &mut Context<Self>) {
        self.run_composer("dump-autoload", cx);
    }

    pub(super) fn composer_validate(&mut self, _: &ComposerValidate, _: &mut Window, cx: &mut Context<Self>) {
        self.run_composer("validate", cx);
    }

    pub(super) fn composer_outdated(&mut self, _: &ComposerOutdated, _: &mut Window, cx: &mut Context<Self>) {
        self.run_composer("outdated --direct", cx);
    }

    pub(super) fn composer_require(&mut self, _: &ComposerRequire, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| InputDialog::new(FileOp::ComposerRequire, window, cx));
        self.show_modal(dialog, window, cx);
    }

    pub(super) fn composer_remove(&mut self, _: &ComposerRemove, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| InputDialog::new(FileOp::ComposerRemove, window, cx));
        self.show_modal(dialog, window, cx);
    }
}
