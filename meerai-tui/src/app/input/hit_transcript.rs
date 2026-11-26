use super::*;

impl AppState {
    pub(crate) fn sidebar_changes_contains(&self, column: u16, row: u16) -> bool {
        let area = self.sidebar_changes_inner_area;
        area.width > 0
            && area.height > 0
            && column >= area.x
            && column < area.x + area.width
            && row >= area.y
            && row < area.y + area.height
    }
    pub fn sidebar_hit_at(&self, column: u16, row: u16) -> Option<SidebarHitKind> {
        if self.modal.is_some() || !self.is_sidebar_visible() {
            return None;
        }
        let contains = |area: Rect| {
            area.width > 0
                && area.height > 0
                && column >= area.x
                && column < area.x + area.width
                && row >= area.y
                && row < area.y + area.height
        };
        if contains(self.sidebar_diagnose_head_area) {
            return Some(SidebarHitKind::DiagnoseHead);
        }
        if contains(self.sidebar_changes_head_area) {
            return Some(SidebarHitKind::ChangesHead);
        }
        if self.file_change_more_hit.is_some_and(contains) {
            return Some(SidebarHitKind::ChangesMore);
        }
        None
    }
    pub fn status_hit_at(&self, column: u16, row: u16) -> Option<StatusHitKind> {
        if let Some((_, kind)) = self
            .runs_hits
            .iter()
            .find(|(rect, _)| rect_contains(*rect, column, row))
        {
            return Some(StatusHitKind::Run(*kind));
        }
        if self
            .update_hit
            .is_some_and(|hit| rect_contains(hit, column, row))
        {
            return Some(StatusHitKind::Update);
        }
        if self
            .web_server_hit
            .is_some_and(|hit| rect_contains(hit, column, row))
        {
            return Some(StatusHitKind::WebServer);
        }
        None
    }
    pub fn hovered_file_change_at(&self, column: u16, row: u16) -> Option<usize> {
        if !self.is_sidebar_visible()
            || self.modal.is_some()
            || !self.sidebar_changes_contains(column, row)
        {
            return None;
        }
        let area = self.sidebar_changes_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        let row_idx = (row - area.y) as usize;
        if row_idx >= self.visible_file_change_rows {
            return None;
        }
        let idx = self.file_change_scroll + row_idx;
        if idx >= self.file_changes.len() {
            return None;
        }
        Some(idx)
    }
    pub fn click_file_change_at(&mut self, column: u16, row: u16) -> bool {
        if let Some(hit) = self.file_change_more_hit
            && column >= hit.x
            && column < hit.x + hit.width
            && row >= hit.y
            && row < hit.y + hit.height
        {
            let shown = self.visible_file_change_rows.max(1);
            let max_start = self.file_changes.len().saturating_sub(shown);
            self.file_change_scroll = if self.file_change_scroll >= max_start {
                0
            } else {
                (self.file_change_scroll + shown).min(max_start)
            };
            return true;
        }
        let Some(row_idx) = self.hovered_file_change_at(column, row) else {
            return false;
        };
        self.open_diff_modal(row_idx);
        true
    }
    pub fn path_reference_at(&self, column: u16, row: u16) -> Option<meerai_syntax::PathRef> {
        let area = self.transcript_area;
        if area.width == 0 || area.height == 0 {
            return None;
        }
        if column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        let scroll = self
            .tabs
            .get(self.active_tab)
            .map_or(0, |tab| tab.scroll_offset);
        let line_index = scroll + (row - area.y) as usize;
        let line = self.transcript_wrapped_lines.get(line_index)?;
        let offset = display_to_char(&line.text, (column - area.x) as usize);
        if offset < line.gutter_len.max(line.hang_len) {
            return None;
        }
        let byte_start = char_to_byte(&line.text, offset);
        let byte_end = char_to_byte(&line.text, offset.saturating_add(1));
        if byte_start >= byte_end || byte_end > line.text.len() {
            return None;
        }
        if !line.text.is_char_boundary(byte_start) || !line.text.is_char_boundary(byte_end) {
            return None;
        }
        let byte = line.text.as_bytes()[byte_start];
        if !is_path_byte(byte) {
            return None;
        }
        let start = line.text[..byte_start]
            .rfind(|ch: char| !is_path_byte(ch as u8))
            .map_or(0, |index| index + 1);
        let end = line.text[byte_start..]
            .find(|ch: char| !is_path_byte(ch as u8))
            .map_or(line.text.len(), |index| byte_start + index);
        let token = &line.text[start..end];
        let reference = meerai_syntax::detect_path_reference(token)?;
        if !self.workspace_root.join(&reference.path).is_file() {
            return None;
        }
        Some(reference)
    }
    pub fn click_path_at(&mut self, column: u16, row: u16) -> bool {
        let Some(reference) = self.path_reference_at(column, row) else {
            return false;
        };
        self.queue_host_action(meerai_host::HostAction::OpenInEditor {
            path: reference.path,
            line: reference.line,
        });
        true
    }
    pub fn hovered_entry_at(&self, pos: ScreenPos) -> Option<usize> {
        if self.modal.is_some() {
            return None;
        }
        self.hovered_zone_at(self.transcript_inner_area, self.effective_scroll(), pos)
    }
    pub(crate) fn hovered_zone_at(
        &self,
        area: Rect,
        scroll: usize,
        pos: ScreenPos,
    ) -> Option<usize> {
        if area.width == 0
            || pos.col < area.x
            || pos.col >= area.x + area.width
            || pos.row < area.y
            || pos.row >= area.y + area.height
        {
            return None;
        }
        let row = scroll.saturating_add((pos.row - area.y) as usize);
        self.hover_zones
            .iter()
            .find(|zone| row >= zone.start_line && row < zone.end_line)
            .map(|zone| zone.entry_idx)
    }
    pub(crate) fn entry_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.transcript_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        let virtual_line = self
            .effective_scroll()
            .saturating_add((row - area.y) as usize);
        self.thinking_hit_zones
            .iter()
            .find(|z| virtual_line >= z.start_line && virtual_line < z.end_line)
            .map(|z| z.entry_idx)
    }
    pub fn toggle_entry_at(&mut self, column: u16, row: u16) -> bool {
        match self.entry_at(column, row) {
            Some(idx) => self.toggle_entry_index(idx),
            None => false,
        }
    }
    pub(crate) fn open_first_diff(&mut self) -> bool {
        if self.file_changes.is_empty() {
            self.status_notice = Some("No file changes to review".to_string());
        } else {
            self.open_diff_modal(0);
        }
        true
    }
    pub(crate) fn digest_at(&self, column: u16, row: u16) -> Option<usize> {
        let area = self.transcript_inner_area;
        if area.width == 0
            || area.height == 0
            || column < area.x
            || column >= area.x + area.width
            || row < area.y
            || row >= area.y + area.height
        {
            return None;
        }
        let virtual_line = self
            .effective_scroll()
            .saturating_add((row - area.y) as usize);
        self.digest_hit_zones
            .iter()
            .find(|z| virtual_line >= z.start_line && virtual_line < z.end_line)
            .map(|z| z.entry_idx)
    }
    pub(crate) fn toggle_turn_activity(&mut self, group_idx: usize) -> bool {
        if !crate::ui::transcript::is_activity_group_start(&self.tab().transcript, group_idx) {
            return false;
        }
        let tab = self.tab_mut();
        let expanded = tab.turn_activity.get(&group_idx).copied().unwrap_or(false);
        tab.turn_activity.insert(group_idx, !expanded);
        true
    }
    pub(crate) fn toggle_entry_index(&mut self, entry_idx: usize) -> bool {
        let mut load_child: Option<String> = None;
        let toggled = match self.tab_mut().transcript.get_mut(entry_idx) {
            Some(TranscriptEntry::Assistant {
                reasoning,
                reasoning_expanded,
                ..
            }) if !reasoning.is_empty() => {
                *reasoning_expanded = !*reasoning_expanded;
                true
            }
            Some(TranscriptEntry::Error { expanded, .. }) => {
                *expanded = !*expanded;
                true
            }
            Some(TranscriptEntry::ToolCall(tc)) => {
                tc.expanded = !tc.expanded;
                true
            }
            Some(TranscriptEntry::Subagent(entry)) if entry.child_session_id.is_some() => {
                let item_id = entry.id.clone();
                self.subagent_view_cache.selection = None;
                self.subagent_view_cache.anchor = None;
                self.modal = Some(ActiveModal::RunDetail {
                    run: RunTarget::Subagent(entry_idx),
                    selected: 0,
                    scroll: 0,
                });
                load_child = Some(item_id);
                true
            }
            Some(TranscriptEntry::User { .. }) => {
                self.modal = Some(ActiveModal::UserMessageActions {
                    entry_idx,
                    selected: 0,
                });
                true
            }
            _ => false,
        };
        if let Some(item_id) = load_child {
            self.request_child_session_load(&item_id, true);
        }
        toggled
    }
    pub(crate) fn rewind_to_entry(&mut self, entry_idx: usize, message: String) -> bool {
        if self.tab().is_running {
            self.status_notice = Some("Cancel the running turn before rewinding".to_string());
            return true;
        }
        let turn = self.tab().user_turns.get(&entry_idx).cloned();
        let Some(turn_id) = turn else {
            self.status_notice = Some("Rewind is unavailable for this message".to_string());
            return true;
        };
        let _ = self.tab().cmd_tx.send(AgentCommand::Rewind {
            turn_id,
            scope: meerai_agents::RewindScope::Both,
        });
        let tab = self.tab_mut();
        tab.transcript.truncate(entry_idx);
        tab.user_turns.retain(|idx, _| *idx < entry_idx);
        tab.turn_activity.retain(|idx, _| *idx < entry_idx);
        tab.auto_follow = true;
        self.restore_composer_message(message);
        self.status_notice =
            Some("Rewound code and conversation; message restored to the composer".to_string());
        true
    }
    pub(crate) fn fork_from_entry(&mut self, entry_idx: usize, draft: String) -> bool {
        if self.tab().is_running {
            self.status_notice = Some("Cancel the running turn before forking".to_string());
            return true;
        }
        if self.session_opener.is_none() {
            self.status_notice = Some("Fork needs an open session".to_string());
            return true;
        }
        let session_id = self.tab().session_id.clone();
        let previous_turn = self.tab().transcript[..entry_idx]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, entry)| matches!(entry, TranscriptEntry::User { .. }))
            .and_then(|(idx, _)| self.tab().user_turns.get(&idx).cloned());
        self.pending_session_actions
            .push(PendingSessionAction::ForkSession {
                session_id,
                turn: previous_turn,
                draft,
            });
        self.status_notice = Some("Forking session...".to_string());
        true
    }
}
