use std::collections::{HashMap, HashSet};

use meerai_web_protocol::{SessionHistoryResponse, SessionSummaryDto, StoredSessionDto};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HistoryStatus {
    #[default]
    Idle,
    Loading,
    Failed(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionPanelAction {
    Open,
    Fork,
    Delete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionPanelRow {
    pub id: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub live: bool,
}

pub struct SessionPanel {
    pub open: bool,
    pub narrow: bool,
    pub focus_request: u64,
    pub filter: String,
    pub selected: usize,
    pub confirm_delete: Option<String>,
    pub action_pending: bool,
    pub action_error: Option<String>,
    pub history_status: HistoryStatus,
    history_request: u64,
    history: Vec<StoredSessionDto>,
}

impl Default for SessionPanel {
    fn default() -> Self {
        Self {
            open: true,
            narrow: false,
            focus_request: 0,
            filter: String::new(),
            selected: 0,
            confirm_delete: None,
            action_pending: false,
            action_error: None,
            history_status: HistoryStatus::Idle,
            history_request: 0,
            history: Vec::new(),
        }
    }
}

impl SessionPanel {
    pub fn set_narrow(&mut self, narrow: bool) {
        if self.narrow != narrow {
            self.narrow = narrow;
            if narrow {
                self.close();
            } else {
                self.open = true;
            }
        }
    }

    pub fn close(&mut self) {
        self.open = false;
        self.confirm_delete = None;
    }

    pub fn reveal(&mut self, filter: Option<&str>) {
        if let Some(filter) = filter {
            self.set_filter(filter.to_owned());
        }
        self.open = true;
        self.focus_request = self.focus_request.wrapping_add(1);
    }

    pub fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        self.selected = 0;
        self.confirm_delete = None;
    }

    pub fn begin_history_request(&mut self) -> u64 {
        self.history_request = self.history_request.wrapping_add(1);
        self.history_status = HistoryStatus::Loading;
        self.history_request
    }

    pub fn finish_history_request(
        &mut self,
        request: u64,
        result: Result<SessionHistoryResponse, String>,
    ) {
        if request != self.history_request {
            return;
        }
        self.history_status = match result {
            Ok(history) => {
                self.history = history.sessions;
                HistoryStatus::Idle
            }
            Err(error) => HistoryStatus::Failed(error),
        };
    }

    pub fn remove_deleted(&mut self, id: &str) {
        self.history.retain(|session| session.id != id);
        self.history_request = self.history_request.wrapping_add(1);
        self.history_status = HistoryStatus::Idle;
    }

    pub fn narrow_open(&self) -> bool {
        self.open && self.narrow
    }

    pub fn finish_open(&mut self) -> bool {
        if self.narrow_open() {
            self.close();
            return true;
        }
        false
    }

    pub fn rows(&self, live: &[SessionSummaryDto]) -> Vec<SessionPanelRow> {
        let query = self.filter.trim().to_ascii_lowercase();
        let live_row = |session: &SessionSummaryDto| SessionPanelRow {
            id: session.id.clone(),
            title: session.title.clone(),
            model: Some(session.model.clone()),
            live: true,
        };
        let matches = |row: &SessionPanelRow| {
            query.is_empty()
                || [
                    row.id.as_str(),
                    row.title.as_deref().unwrap_or(""),
                    row.model.as_deref().unwrap_or(""),
                ]
                .iter()
                .any(|value| value.to_ascii_lowercase().contains(&query))
        };
        let live_by_id: HashMap<_, _> = live
            .iter()
            .map(|session| (session.id.as_str(), session))
            .collect();
        let mut history_ids = HashSet::new();
        let stored = self
            .history
            .iter()
            .filter_map(|session| {
                history_ids.insert(session.id.as_str());
                let row = if let Some(live) = live_by_id.get(session.id.as_str()) {
                    let mut row = live_row(live);
                    row.title = row.title.or_else(|| session.title.clone());
                    row
                } else {
                    SessionPanelRow {
                        id: session.id.clone(),
                        title: session.title.clone(),
                        model: session.model.clone(),
                        live: false,
                    }
                };
                matches(&row).then_some(row)
            })
            .collect::<Vec<_>>();
        live.iter()
            .filter(|session| !history_ids.contains(session.id.as_str()))
            .map(live_row)
            .filter(matches)
            .chain(stored)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn live(id: &str, title: &str) -> SessionSummaryDto {
        SessionSummaryDto {
            id: id.to_owned(),
            title: Some(title.to_owned()),
            backend: meerai_web_protocol::BackendKind::Native,
            provider: "anthropic".to_owned(),
            model: "claude-sonnet".to_owned(),
            approval_mode: meerai_web_protocol::ApprovalModeDto::Ask,
            status: meerai_web_protocol::SessionStatus::Idle,
            active_turn_id: None,
            active_tool: None,
            turn_started_at_ms: None,
            user_shell_running: false,
            transient: false,
            shell_runs: Vec::new(),
        }
    }

    fn stored(id: &str, title: Option<&str>) -> StoredSessionDto {
        StoredSessionDto {
            id: id.to_owned(),
            title: title.map(str::to_owned),
            model: Some("claude-sonnet".to_owned()),
            created_at_ms: 0,
            updated_at_ms: 0,
            bytes: 0,
            is_open: false,
            is_child: false,
        }
    }

    fn history(sessions: Vec<StoredSessionDto>) -> SessionHistoryResponse {
        SessionHistoryResponse { sessions }
    }

    #[test]
    fn narrow_panels_close_on_shrink_and_reopen_on_grow() {
        let mut panel = SessionPanel::default();
        assert!(panel.open && !panel.narrow);

        panel.set_narrow(true);
        assert!(!panel.open && !panel.narrow_open());

        panel.reveal(None);
        assert!(panel.narrow_open());

        panel.set_narrow(false);
        assert!(panel.open);
        assert!(!panel.narrow_open());
        assert!(!panel.finish_open());
    }

    #[test]
    fn stale_history_responses_are_dropped() {
        let mut panel = SessionPanel::default();
        let first = panel.begin_history_request();
        let second = panel.begin_history_request();
        assert_ne!(first, second);

        panel.finish_history_request(first, Ok(history(vec![stored("old", Some("Old"))])));
        assert_eq!(panel.history_status, HistoryStatus::Loading);

        panel.finish_history_request(second, Ok(history(vec![stored("new", Some("New"))])));
        assert_eq!(panel.history_status, HistoryStatus::Idle);
        let rows = panel.rows(&[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "new");
    }

    #[test]
    fn history_failures_surface_and_deletes_invalidate_in_flight_requests() {
        let mut panel = SessionPanel::default();
        let request = panel.begin_history_request();
        panel.finish_history_request(request, Err("offline".to_owned()));
        assert_eq!(
            panel.history_status,
            HistoryStatus::Failed("offline".to_owned())
        );

        let request = panel.begin_history_request();
        panel.remove_deleted("gone");
        assert_eq!(panel.history_status, HistoryStatus::Idle);
        panel.finish_history_request(request, Ok(history(vec![stored("late", Some("Late"))])));
        assert!(panel.rows(&[]).is_empty());
    }

    #[test]
    fn rows_lead_with_open_sessions_that_are_not_in_history() {
        let mut panel = SessionPanel::default();
        let request = panel.begin_history_request();
        panel.finish_history_request(
            request,
            Ok(history(vec![
                stored("s2", Some("Stored two")),
                stored("s1", None),
            ])),
        );

        let rows = panel.rows(&[live("s1", "Live one"), live("s3", "Live three")]);
        let ids = rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>();
        assert_eq!(ids, ["s3", "s2", "s1"]);
        assert_eq!(rows[0].title.as_deref(), Some("Live three"));
        assert_eq!(rows[1].title.as_deref(), Some("Stored two"));
        assert_eq!(rows[2].title.as_deref(), Some("Live one"));
        assert!(rows[0].live && !rows[1].live && rows[2].live);
    }

    #[test]
    fn filter_matches_id_title_and_model_and_resets_selection() {
        let mut panel = SessionPanel::default();
        let request = panel.begin_history_request();
        panel.finish_history_request(
            request,
            Ok(history(vec![
                stored("aaa", Some("Alpha")),
                stored("bbb", Some("Beta")),
            ])),
        );
        panel.selected = 1;

        panel.set_filter("beta".to_owned());
        assert_eq!(panel.selected, 0);
        let rows = panel.rows(&[]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "bbb");

        panel.set_filter("claude".to_owned());
        assert_eq!(panel.rows(&[]).len(), 2);

        panel.set_filter("aaa".to_owned());
        assert_eq!(panel.rows(&[]).len(), 1);
    }
}
