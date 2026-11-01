//! Find Usages (⌥F7) lists every reference to the symbol at the caret across the project;
//! Highlight Usages in File (⌘⇧F7) marks them in this editor until the text changes or Esc.

use gpui::{Context, Window};

use super::{ClearUsageHighlights, EditorView, FindUsages, HighlightUsages, RenameSymbol};

impl EditorView {
    pub(super) fn find_usages(&mut self, _: &FindUsages, window: &mut Window, cx: &mut Context<Self>) {
        self.find_usages_at(self.caret_byte(), window, cx);
    }

    /// Searches in the background, then shows the results in a list with a preview.
    pub(super) fn find_usages_at(&mut self, offset: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        let text = self.buffer.text();
        let Some(symbol) = self.intel.symbol_at(&path, &text, offset) else { return };
        let open = if self.buffer.is_dirty() { vec![(path, text)] } else { Vec::new() };
        let intel = self.intel.clone();
        let workspace = self.workspace.clone();
        let name = symbol.name.clone();
        cx.spawn_in(window, async move |_, cx| {
            let found = cx.background_executor().spawn(async move { intel.find_usages(&symbol, &open) }).await;
            workspace.update_in(cx, |ws, window, cx| ws.show_usages(&name, found, window, cx)).ok();
        })
        .detach();
    }

    pub(super) fn highlight_usages(&mut self, _: &HighlightUsages, _: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        let text = self.buffer.text();
        let Some(symbol) = self.intel.symbol_at(&path, &text, self.caret_byte()) else { return };
        let found = self.intel.find_usages_in(&symbol, &path, &text);
        self.usage_highlights = Some((self.buffer.version(), found));
        cx.notify();
    }

    pub(super) fn clear_usage_highlights(&mut self, _: &ClearUsageHighlights, _: &mut Window, cx: &mut Context<Self>) {
        self.usage_highlights = None;
        cx.notify();
    }

    /// Usage ranges to draw, if still valid for the current text.
    pub(super) fn current_usage_highlights(&self) -> Vec<std::ops::Range<usize>> {
        match &self.usage_highlights {
            Some((version, ranges)) if *version == self.buffer.version() => ranges.clone(),
            _ => Vec::new(),
        }
    }

    /// Refactor ▸ Rename (⇧F6).
    pub(super) fn rename_symbol(&mut self, _: &RenameSymbol, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.path() else { return };
        let Some(symbol) = self.intel.symbol_at(&path, &self.buffer.text(), self.caret_byte()) else { return };
        // Unsaved edits first, so offsets match the files on disk.
        self.save_now(cx);
        self.workspace.update(cx, |ws, cx| ws.start_rename(symbol, window, cx)).ok();
    }
}
