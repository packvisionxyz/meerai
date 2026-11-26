use super::*;

impl AppState {
    pub fn tab_hit_at(&self, column: u16, row: u16) -> Option<TabHitKind> {
        self.tab_hit_zones
            .iter()
            .find(|zone| {
                let r = zone.rect;
                r.width > 0
                    && r.height > 0
                    && column >= r.x
                    && column < r.x + r.width
                    && row >= r.y
                    && row < r.y + r.height
            })
            .map(|zone| zone.kind)
    }
    pub fn completion_hit_at(&self, column: u16, row: u16) -> Option<usize> {
        if !self.completion.is_active() || self.modal.is_some() || self.mode != InputMode::Insert {
            return None;
        }
        self.completion_hit_zones
            .iter()
            .find(|zone| {
                let r = zone.rect;
                r.width > 0
                    && r.height > 0
                    && column >= r.x
                    && column < r.x + r.width
                    && row >= r.y
                    && row < r.y + r.height
            })
            .map(|zone| zone.index)
    }
    pub fn click_completion_at(&mut self, column: u16, row: u16) -> bool {
        let Some(index) = self.completion_hit_at(column, row) else {
            return false;
        };
        self.completion.set_selected(index);
        self.accept_completion();
        true
    }
    pub fn click_tab_at(&mut self, column: u16, row: u16) -> bool {
        let Some(kind) = self.tab_hit_at(column, row) else {
            return false;
        };

        match kind {
            TabHitKind::Tab(index) => {
                let _ = self.switch_to_tab(index);
                if self.modal.is_none() {
                    self.tab_drag = Some(TabDrag {
                        from: index,
                        start_x: column,
                        current_x: column,
                        moved: false,
                    });
                }
                true
            }
            TabHitKind::CloseTab(index) => self.close_tab(index),
            TabHitKind::NewTab => {
                self.request_new_tab();
                true
            }
        }
    }
    fn queued_hit_at(&self, column: u16, row: u16) -> Option<QueuedHitKind> {
        self.queued_hit_zones
            .iter()
            .find(|zone| {
                column >= zone.rect.x
                    && column < zone.rect.x + zone.rect.width
                    && row >= zone.rect.y
                    && row < zone.rect.y + zone.rect.height
            })
            .map(|zone| zone.kind)
    }
    fn click_queued_at(&mut self, column: u16, row: u16) -> bool {
        let Some(kind) = self.queued_hit_at(column, row) else {
            return false;
        };
        match kind {
            QueuedHitKind::Remove(idx) => {
                let removed = {
                    let tab = self.tab_mut();
                    if idx < tab.queued.len() {
                        Some(tab.queued.remove(idx))
                    } else {
                        None
                    }
                };
                if let Some(text) = removed {
                    let preview = meerai_core::text::truncate_single_line(&text, 40);
                    self.status_notice = Some(format!("Removed queued: {preview}"));
                }
            }
            QueuedHitKind::Edit(idx) => {
                if let Some(text) = self.tab().queued.get(idx).cloned() {
                    self.modal = Some(ActiveModal::QueuedEdit {
                        index: idx,
                        input: TextInput::new(text),
                        error: None,
                    });
                }
            }
        }
        true
    }
}
