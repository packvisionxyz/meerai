use super::*;

impl AppState {
    pub(super) fn open_subagent_overlay(&mut self) {
        let mut running = Vec::new();
        let mut all = Vec::new();
        for (idx, entry) in self.tab().transcript.iter().enumerate() {
            if let TranscriptEntry::Subagent(subagent) = entry {
                all.push(idx);
                if matches!(
                    subagent.status,
                    ToolCallStatus::Running | ToolCallStatus::Proposed
                ) {
                    running.push(idx);
                }
            }
        }
        let entries = if running.is_empty() { all } else { running };
        if entries.is_empty() {
            self.status_notice = Some("No subagents in this session".to_string());
            self.subagent_overlay = None;
            return;
        }
        self.subagent_overlay = Some(SubagentOverlay {
            selected: 0,
            entries,
        });
    }

    pub(super) fn close_subagent_overlay(&mut self) {
        self.subagent_overlay = None;
        self.subagent_overlay_zones.clear();
    }

    pub(super) fn toggle_subagent_overlay(&mut self) {
        if self.subagent_overlay.is_some() {
            self.close_subagent_overlay();
        } else {
            self.open_subagent_overlay();
        }
    }

    pub(super) fn subagent_overlay_click(&mut self, column: u16, row: u16) -> bool {
        let Some(overlay) = self.subagent_overlay.clone() else {
            return false;
        };
        if let Some((_, index)) = self
            .subagent_overlay_zones
            .iter()
            .find(|(rect, _)| contains(*rect, column, row))
        {
            let index = *index;
            if let Some(entry_idx) = overlay.entries.get(index).copied() {
                if let Some(overlay) = self.subagent_overlay.as_mut() {
                    overlay.selected = index;
                }
                self.focus_subagent(entry_idx);
            }
            return true;
        }
        if self
            .subagent_hit
            .is_some_and(|hit| contains(hit, column, row))
        {
            return false;
        }
        self.close_subagent_overlay();
        true
    }

    fn focus_subagent(&mut self, entry_idx: usize) {
        let tab_id = self.tab().tab_id;
        let turn = self.tab().transcript[..=entry_idx]
            .iter()
            .rposition(|entry| matches!(entry, TranscriptEntry::User { .. }));
        let mut child = None;
        {
            let tab = self.tab_mut();
            let Some(TranscriptEntry::Subagent(subagent)) = tab.transcript.get_mut(entry_idx)
            else {
                return;
            };
            subagent.expanded = true;
            if let Some(turn) = turn {
                tab.turn_activity.insert(turn, true);
            }
            if subagent.nested.is_none()
                && let Some(session_id) = subagent.child_session_id.clone()
                && !subagent.nested_loading
            {
                subagent.nested_loading = true;
                child = Some((subagent.id.clone(), session_id));
            }
            tab.auto_follow = false;
        }
        if let Some(zone) = self
            .hover_zones
            .iter()
            .find(|zone| zone.entry_idx == entry_idx)
        {
            self.tab_mut().scroll_offset = zone.start_line.saturating_sub(2);
        }
        if let Some((item_id, session_id)) = child {
            self.pending_session_actions
                .push(PendingSessionAction::ReadChildSession {
                    tab_id,
                    item_id,
                    session_id: SessionId::from_raw(session_id),
                });
        }
    }
}

pub(super) fn contains(rect: Rect, column: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && column >= rect.x
        && column < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}
