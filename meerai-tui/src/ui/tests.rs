use super::composer::render_composer_line;
use super::statusline::{format_compact_ms, format_compact_tokens};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use meerai_agents::{
    AgentEvent, ErrorCode, ItemDelta, ItemId, SessionItem, TurnEndReason, TurnId, Usage,
};
use meerai_client::providers::{ProviderCatalog, RuntimeProviderState};
use meerai_client::secrets::{MemorySecretStore, SecretStore};
use meerai_common::settings::AppSettings;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tokio::sync::mpsc;

use super::*;

fn test_app() -> AppState {
    let (tx, _rx) = mpsc::unbounded_channel();
    let secrets = Arc::new(MemorySecretStore::new());
    secrets.set_key("gemini", "test-key").unwrap();
    let settings = AppSettings::default();
    let runtime = Arc::new(RwLock::new(RuntimeProviderState::from_settings(
        &settings,
        ProviderCatalog::new(),
    )));
    AppState::new(
        "gemini",
        "gemini-3.8-flash",
        PathBuf::from("."),
        tx,
        runtime,
        secrets,
        settings,
        None,
    )
}

fn render_to_string_with_size(app: &mut AppState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, app)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

fn render_to_string(app: &mut AppState) -> String {
    render_to_string_with_size(app, 120, 28)
}

fn agent_definition(id: &str) -> AgentDefinition {
    AgentDefinition {
        id: id.to_string(),
        source: meerai_common::definitions::DefinitionSource::Agents,
        description: Some("Reviews changes".to_string()),
        model: None,
        steps: None,
        mode: None,
        prompt: String::new(),
        path: PathBuf::from(format!(".agents/agents/{id}.md")),
    }
}

fn skill_definition(id: &str) -> SkillDefinition {
    SkillDefinition {
        id: id.to_string(),
        source: meerai_common::definitions::DefinitionSource::Claude,
        description: Some("Delivers work".to_string()),
        body: String::new(),
        path: PathBuf::from(format!(".claude/skills/{id}/SKILL.md")),
    }
}

fn span_text(spans: &[Span<'_>]) -> String {
    spans.iter().map(|span| span.content.to_string()).collect()
}

#[test]
fn hides_sidebar_when_terminal_narrow_and_clears_hit_area() {
    let mut app = test_app();
    app.file_changes.push(crate::app::FileChangeEntry {
        status: "M".to_string(),
        path: "meerai-tui/src/ui.rs".to_string(),
    });

    let wide = render_to_string_with_size(&mut app, 120, 28);
    assert!(wide.contains("SESSION"));
    assert!(wide.contains("File Changes (1)"));
    assert!(app.visible_file_change_rows == 1);
    assert!(app.sidebar_changes_inner_area.width > 0);

    let narrow = render_to_string_with_size(&mut app, 90, 28);
    assert!(!narrow.contains("File Changes (1)"));
    assert_eq!(app.visible_file_change_rows, 0);
    assert_eq!(app.sidebar_changes_inner_area, Rect::default());
}

#[test]
fn statusline_shows_update_and_the_hit_zone_runs_it() {
    let mut app = test_app();
    app.update_available = Some("9.9.9".to_string());
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("update v9.9.9"));
    let hit = app.update_hit.expect("update hit zone");

    assert!(app.handle_mouse_event(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        column: hit.x + 1,
        row: hit.y,
        modifiers: crossterm::event::KeyModifiers::NONE,
    }));
    assert!(app.tab().transcript.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::System { text } if text.contains("9.9.9")
    )));
}

fn mouse(
    kind: crossterm::event::MouseEventKind,
    column: u16,
    row: u16,
) -> crossterm::event::MouseEvent {
    crossterm::event::MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn composer_drag_selects_text_and_delete_removes_it() {
    let mut app = test_app();
    app.tab_mut().input = "hello world".to_string();
    app.tab_mut().cursor = 0;
    render_to_string(&mut app);

    let input_area = app.composer_input_area;
    assert!(input_area.width > 0);
    assert!(app.composer_block_area.height > 0);

    assert!(app.handle_mouse_event(mouse(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        input_area.x,
        input_area.y,
    )));
    assert!(app.handle_mouse_event(mouse(
        crossterm::event::MouseEventKind::Moved,
        input_area.x + 6,
        input_area.y,
    )));
    assert!(app.handle_mouse_event(mouse(
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        input_area.x + 6,
        input_area.y,
    )));
    assert_eq!(app.composer_selection, Some((0, 6)));

    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let cell = terminal
        .backend()
        .buffer()
        .cell((input_area.x, input_area.y))
        .unwrap();
    assert_eq!(cell.bg, theme::p().surface_high);

    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
    assert_eq!(app.tab().input, "world");
    assert_eq!(app.tab().cursor, 0);
    assert_eq!(app.composer_selection, None);
}

#[test]
fn sidebar_shows_the_workspace_without_duplicated_metrics() {
    let mut app = test_app();
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("dir"));
    assert!(!rendered.contains("tokens "));
    assert!(!rendered.contains("speed "));
    assert!(!rendered.contains("state "));
}

#[test]
fn running_tools_expand_to_show_full_arguments() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "go".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "call-1".to_string(),
            name: "code-grep".to_string(),
            args: format!(
                "{{\"query\":\"{}\",\"target\":\"TARGET-VALUE\"}}",
                "x".repeat(80)
            ),
            status: ToolCallStatus::Running,
            output: None,
            expanded: false,
        }));
    app.tab_mut().turn_activity.insert(0, true);

    let collapsed = render_to_string(&mut app);
    assert!(collapsed.contains("Searching"));
    assert!(!collapsed.contains("TARGET-VALUE"));

    let row = (0..28)
        .find(|y| {
            app.screen_lines
                .get(*y as usize)
                .is_some_and(|line| line.join("").contains("Searching"))
        })
        .expect("tool row");
    let col = app.transcript_inner_area.x + 2;
    assert!(app.toggle_entry_at(col, row));

    let expanded = render_to_string(&mut app);
    assert!(expanded.contains("TARGET-VALUE"));
}

#[test]
fn session_picker_shows_delete_and_confirmation_hints() {
    let mut app = test_app();
    app.modal = Some(ActiveModal::SessionSelect {
        filter: String::new(),
        selected: 0,
        sessions: vec![meerai_session::SessionSummary {
            id: meerai_session::SessionId::from_raw("ses-old"),
            dir: std::path::PathBuf::from("/tmp/ses-old"),
            title: Some("Old".to_string()),
            model: None,
            created_at_ms: 1,
            updated_at_ms: 2,
            bytes: 0,
            parent_session_id: None,
        }],
        loading: false,
        error: None,
    });
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("ctrl+d delete"));

    app.session_delete_confirm = Some(meerai_session::SessionId::from_raw("ses-old"));
    let confirming = render_to_string(&mut app);
    assert!(confirming.contains("Press ctrl+d again to delete"));
}

#[test]
fn renders_command_palette_with_shortcuts() {
    let mut app = test_app();
    app.modal = Some(ActiveModal::CommandPalette {
        filter: String::new(),
        selected: 0,
    });
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("Commands (ctrl+p)"));
    assert!(rendered.contains(" SESSION "));
    assert!(rendered.contains("New session tab"));
    assert!(rendered.contains("ctrl+n"));
    assert!(rendered.contains("shift+tab"));
    assert!(rendered.contains("filter commands"));
    assert!(rendered.contains("Enter run"));
}

#[test]
fn hover_highlights_the_row_under_the_pointer() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "inspect the file".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "call-1".to_string(),
            name: "code-read_file".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: None,
            expanded: false,
        }));
    app.tab_mut().turn_activity.insert(0, true);

    let _ = render_to_string(&mut app);
    let tool_row = (0..28)
        .find(|y| {
            app.screen_lines
                .get(*y as usize)
                .is_some_and(|line| line.join("").contains("Read"))
        })
        .expect("tool row");

    let pos = crate::app::ScreenPos {
        col: 10,
        row: tool_row,
    };
    assert_eq!(app.hovered_entry_at(pos), Some(1));

    let moved = app.handle_mouse_event(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Moved,
        column: pos.col,
        row: pos.row,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    assert!(moved);
    assert_eq!(app.hovered_entry, Some(1));

    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    assert_eq!(
        buffer[(80, tool_row)].bg,
        crate::theme::Palette::default().surface_high,
        "hovered row uses the raised hover background"
    );

    let left = app.handle_mouse_event(crossterm::event::MouseEvent {
        kind: crossterm::event::MouseEventKind::Moved,
        column: 10,
        row: 0,
        modifiers: crossterm::event::KeyModifiers::NONE,
    });
    assert!(left);
    assert_eq!(app.hovered_entry, None);
}

#[test]
fn clicking_a_tool_row_expands_and_collapses_its_output() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "read it".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "call-1".to_string(),
            name: "code-read_file".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: Some(
                (0..12)
                    .map(|index| format!("line-{index}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            expanded: false,
        }));
    app.tab_mut().turn_activity.insert(0, true);

    let collapsed = render_to_string(&mut app);
    assert!(collapsed.contains("+ Read"));
    assert!(collapsed.contains("line-11"));
    assert!(!collapsed.contains("line-0"));

    let row = (0..28)
        .find(|y| {
            app.screen_lines
                .get(*y as usize)
                .is_some_and(|line| line.join("").contains("Read"))
        })
        .expect("tool row");
    let col = app.transcript_inner_area.x + 2;
    assert!(app.toggle_entry_at(col, row));

    let expanded = render_to_string(&mut app);
    assert!(expanded.contains("- Read"));
    assert!(expanded.contains("line-11"));
    assert!(expanded.contains("line-0"));

    assert!(app.toggle_entry_at(col, row));
    let collapsed_again = render_to_string(&mut app);
    assert!(collapsed_again.contains("+ Read"));
    assert!(!collapsed_again.contains("line-0"));
}

#[test]
fn tool_runs_have_a_band_and_shaded_text_when_finished() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "run things".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "running".to_string(),
            name: "code-shell".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Running,
            output: None,
            expanded: false,
        }));
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "done".to_string(),
            name: "code-edit_file".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: None,
            expanded: false,
        }));
    app.tab_mut().turn_activity.insert(0, true);

    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let palette = crate::theme::Palette::default();

    let locate = |needle: &str| -> (u16, u16) {
        for y in 0..28 {
            let text: String = (0..120).map(|x| buffer[(x, y)].symbol()).collect();
            if let Some(col) = text.find(needle) {
                return (col as u16, y);
            }
        }
        panic!("row for {needle}")
    };

    let (run_col, run_row) = locate("[running]");
    let (done_col, done_row) = locate("Edited");
    assert_eq!(
        buffer[(run_col, run_row)].bg,
        palette.surface,
        "running tool row keeps the raised band"
    );
    assert_eq!(
        buffer[(done_col, done_row)].bg,
        palette.surface,
        "finished tool row keeps the band"
    );
    assert_ne!(
        buffer[(run_col, run_row)].fg,
        buffer[(done_col, done_row)].fg,
        "finished tool rows use muted text"
    );
}

#[test]
fn renders_turn_digest_grouping_tools_and_thoughts() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "do the work".to_string(),
    });
    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: String::new(),
        reasoning: "plan the edit".to_string(),
        streaming: false,
        reasoning_expanded: false,
    });
    for (name, status) in [
        ("code-shell", ToolCallStatus::Succeeded),
        ("code-read_file", ToolCallStatus::Succeeded),
        ("code-edit_file", ToolCallStatus::Succeeded),
    ] {
        app.tab_mut()
            .transcript
            .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
                call_id: format!("call-{name}"),
                name: name.to_string(),
                args: "{}".to_string(),
                status,
                output: None,
                expanded: false,
            }));
    }

    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: "All done.".to_string(),
        reasoning: String::new(),
        streaming: false,
        reasoning_expanded: false,
    });
    app.tab_mut().transcript.push(TranscriptEntry::Error {
        text: "boom".to_string(),
        expanded: false,
    });

    let collapsed = render_to_string(&mut app);
    assert!(
        collapsed.contains("+ 1 command · 1 read · 1 edit · 1 thought"),
        "{collapsed}"
    );
    assert!(!collapsed.contains("Thought (1 line)"));
    assert!(!collapsed.contains("Ran"));
    assert!(collapsed.contains("All done."));
    assert!(collapsed.contains("[error]"));

    app.tab_mut().turn_activity.insert(0, true);
    let expanded = render_to_string(&mut app);
    assert!(expanded.contains("- 1 command · 1 read · 1 edit · 1 thought"));
    assert!(expanded.contains("+ Thought (1 line)"));
    assert!(expanded.contains("Ran {}"));
}

#[test]
fn clicking_the_digest_expands_and_collapses_turn_activity() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "go".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "call-1".to_string(),
            name: "code-grep".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: None,
            expanded: false,
        }));

    let collapsed = render_to_string(&mut app);
    assert!(collapsed.contains("+ 1 tool"));
    assert!(!collapsed.contains("Searched"));

    let zone = app
        .digest_hit_zones
        .iter()
        .find(|zone| zone.entry_idx == 0)
        .copied()
        .expect("digest hit zone");
    let col = app.transcript_inner_area.x + 2;
    let row = app.transcript_inner_area.y + zone.start_line as u16;
    assert!(app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), col, row)));
    assert!(app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), col, row)));
    assert_eq!(app.tab().turn_activity.get(&0), Some(&true));

    let expanded = render_to_string(&mut app);
    assert!(expanded.contains("Searched"));

    let zone = app.digest_hit_zones[0];
    let row = app.transcript_inner_area.y + zone.start_line as u16;
    assert!(app.handle_mouse_event(mouse(MouseEventKind::Down(MouseButton::Left), col, row)));
    assert!(app.handle_mouse_event(mouse(MouseEventKind::Up(MouseButton::Left), col, row)));
    assert_eq!(app.tab().turn_activity.get(&0), Some(&false));
    let recollapsed = render_to_string(&mut app);
    assert!(!recollapsed.contains("Searched"));
}

#[test]
fn subagent_expands_to_show_child_conversation() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "go".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::Subagent(crate::app::SubagentEntry {
            id: "sub-1".to_string(),
            agent: "explore".to_string(),
            status: ToolCallStatus::Succeeded,
            report: Some("final report".to_string()),
            child_session_id: Some("ses-child".to_string()),
            expanded: false,
            nested: None,
            nested_refreshed_at: None,
            nested_loading: false,
        }));
    app.tab_mut().turn_activity.insert(0, true);

    let collapsed = render_to_string(&mut app);
    assert!(collapsed.contains("+ [subagent] explore [ok]"));
    assert!(collapsed.contains("final report"));

    let zone = app
        .thinking_hit_zones
        .iter()
        .find(|zone| zone.entry_idx == 1)
        .copied()
        .expect("subagent hit zone");
    let col = app.transcript_inner_area.x + 2;
    let row = app.transcript_inner_area.y + zone.start_line as u16;
    assert!(app.toggle_entry_at(col, row));

    let loading = render_to_string(&mut app);
    assert!(loading.contains("- [subagent] explore [ok]"));
    assert!(loading.contains("loading conversation…"));

    if let Some(TranscriptEntry::Subagent(entry)) = app.tab_mut().transcript.get_mut(1) {
        entry.nested = Some(vec![
            TranscriptEntry::User {
                text: "find the bug".to_string(),
            },
            TranscriptEntry::Assistant {
                text: "Found it.".to_string(),
                reasoning: "checking".to_string(),
                streaming: false,
                reasoning_expanded: false,
            },
            TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
                call_id: "c1".to_string(),
                name: "code-grep".to_string(),
                args: "{}".to_string(),
                status: ToolCallStatus::Succeeded,
                output: Some("src/lib.rs:10".to_string()),
                expanded: false,
            }),
        ]);
    }
    let expanded = render_to_string(&mut app);
    assert!(expanded.contains("you: find the bug"));
    assert!(expanded.contains("thought (1 line)"));
    assert!(expanded.contains("Found it."));
    assert!(expanded.contains("[tool] code-grep [ok]"));
    assert!(!expanded.contains("final report"));

    let zone = app
        .thinking_hit_zones
        .iter()
        .find(|zone| zone.entry_idx == 1)
        .copied()
        .expect("subagent hit zone");
    let row = app.transcript_inner_area.y + zone.start_line as u16;
    assert!(app.toggle_entry_at(col, row));
    let recollapsed = render_to_string(&mut app);
    assert!(recollapsed.contains("final report"));
    assert!(!recollapsed.contains("you: find the bug"));
}

#[test]
fn digest_counts_acp_run_as_agent() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "go".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "acp-1".to_string(),
            name: "acp-run".to_string(),
            args: "{\"agent\":\"agy\",\"task\":\"fix it\"}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: None,
            expanded: false,
        }));
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("+ 1 agent"), "{rendered}");
    assert!(!rendered.contains("1 tool"), "{rendered}");
}

#[test]
fn digest_uses_gerunds_for_active_work() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "go".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "c1".to_string(),
            name: "code-read_file".to_string(),
            args: "{\"path\":\"src/lib.rs\"}".to_string(),
            status: ToolCallStatus::Running,
            output: None,
            expanded: false,
        }));
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("+ Reading 1"), "{rendered}");
}

#[test]
fn digest_hover_marks_summary_for_text_highlight() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "go".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "c1".to_string(),
            name: "code-grep".to_string(),
            args: "{}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: None,
            expanded: false,
        }));
    let _ = render_to_string(&mut app);
    let zone = app.digest_hit_zones[0];
    let col = app.transcript_inner_area.x + 4;
    let row = app.transcript_inner_area.y + zone.start_line as u16;
    let _ = app.handle_mouse_event(mouse(crossterm::event::MouseEventKind::Moved, col, row));
    let _ = render_to_string(&mut app);
    assert_eq!(app.hovered_digest, Some(0));
}

#[test]
fn expanded_edit_renders_colored_diff_with_counts() {
    let mut app = test_app();
    let diff = "Edited src/lib.rs\n\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n fn main() {\n-    old();\n+    new();\n }\n";
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::app::ToolCallEntry {
            call_id: "e1".to_string(),
            name: "code-edit_file".to_string(),
            args: "{\"path\":\"src/lib.rs\",\"old_string\":\"old\",\"new_string\":\"new\"}"
                .to_string(),
            status: ToolCallStatus::Succeeded,
            output: Some(diff.to_string()),
            expanded: false,
        }));

    let collapsed = render_to_string(&mut app);
    assert!(collapsed.contains("+ Edited src/lib.rs [ok] +1 -1"));
    assert!(!collapsed.contains("new();"));

    let row = (0..28)
        .find(|y| {
            app.screen_lines
                .get(*y as usize)
                .is_some_and(|line| line.join("").contains("Edited"))
        })
        .expect("edit row");
    let col = app.transcript_inner_area.x + 2;
    assert!(app.toggle_entry_at(col, row));

    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    let palette = crate::theme::Palette::default();
    let find = |needle: &str| -> u16 {
        for y in 0..28 {
            let text: String = (0..120).map(|x| buffer[(x, y)].symbol()).collect();
            if text.contains(needle) {
                return y;
            }
        }
        panic!("row for {needle}");
    };
    let row_has_fg =
        |row: u16, color: ratatui::style::Color| (0..120).any(|x| buffer[(x, row)].fg == color);
    assert!(row_has_fg(find("old();"), palette.diff_removed));
    assert!(row_has_fg(find("new();"), palette.diff_added));

    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("- Edited src/lib.rs [ok] +1 -1"));
    assert!(rendered.contains("@@ -1,2 +1,2 @@"));
}

#[test]
fn statusline_shows_running_subagent_names() {
    let mut app = test_app();
    app.tab_mut().is_running = true;
    app.tab_mut().active_tool = Some("subagent-run".to_string());
    for (id, agent) in [("s1", "explore"), ("s2", "build")] {
        app.tab_mut()
            .transcript
            .push(TranscriptEntry::Subagent(crate::app::SubagentEntry {
                id: id.to_string(),
                agent: agent.to_string(),
                status: ToolCallStatus::Running,
                report: None,
                child_session_id: None,
                expanded: false,
                nested: None,
                nested_refreshed_at: None,
                nested_loading: false,
            }));
    }
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("explore, build"));
}

#[test]
fn paints_sidebar_panel_and_user_band_without_main_borders() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "highlight me".to_string(),
    });
    app.file_changes.push(crate::app::FileChangeEntry {
        status: "M".to_string(),
        path: "src/main.rs".to_string(),
    });

    let backend = TestBackend::new(120, 28);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|frame| render(frame, &mut app)).unwrap();
    let buffer = terminal.backend().buffer().clone();

    let surface = crate::theme::Palette::default().surface;
    assert_eq!(buffer[(110, 4)].bg, surface, "sidebar panel background");

    let band_row = (0..28)
        .find(|y| {
            (0..100).any(|x| {
                let mut symbol = String::new();
                symbol.push_str(buffer[(x, *y)].symbol());
                symbol == "▌"
            })
        })
        .expect("user band row");
    assert_eq!(buffer[(80, band_row)].bg, surface, "user band background");

    let rendered: String = (0..28)
        .flat_map(|y| (0..120).map(move |x| (x, y)))
        .map(|(x, y)| buffer[(x, y)].symbol().to_string())
        .collect();
    assert!(!rendered.contains('┌'), "no box border in the main layout");
}

#[test]
fn renders_transcript_completion_and_statusline() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "Explain Cargo.toml".to_string(),
    });
    let turn_id = TurnId::from_raw("turn-ui-render");
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: turn_id.clone(),
        model: "gemini::gemini-3.8-flash".to_string(),
    });
    let message_id = ItemId::from_raw("item-ui-render");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::AgentMessage {
            id: message_id.clone(),
            text: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: message_id.clone(),
        delta: ItemDelta::AgentText("Sure thing.".to_string()),
    });
    app.handle_agent_event(AgentEvent::UsageUpdated {
        usage: Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(25),
            total_tokens: Some(125),
            cached_tokens: None,
            cache_creation_tokens: None,
        },
        cumulative: Usage {
            prompt_tokens: Some(100),
            completion_tokens: Some(25),
            total_tokens: Some(125),
            cached_tokens: Some(40),
            cache_creation_tokens: None,
        },
        est_cost_usd: None,
    });
    app.handle_agent_event(AgentEvent::ItemCompleted {
        item: SessionItem::AgentMessage {
            id: message_id,
            text: "Sure thing.".to_string(),
        },
    });
    app.handle_agent_event(AgentEvent::TurnCompleted {
        turn_id,
        reason: TurnEndReason::Completed,
    });

    app.tab_mut().metrics.last_tokens_per_sec = Some(42.5);
    app.tab_mut().metrics.last_ttft_ms = Some(310);
    app.tab_mut().metrics.last_turn_ms = Some(1400);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert!(app.completion.is_active());

    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("INS"));
    assert!(rendered.contains("gemini / gemini-3.8-flash"));
    assert!(rendered.contains("[saved]"));
    assert!(rendered.contains("↑100 ↓25 Σ125"));
    assert!(rendered.contains("session ↑100 ↓25 Σ125 ↻40"));
    assert!(rendered.contains("42.5 tok/s"));
    assert!(rendered.contains("1.4s"));
    assert!(rendered.contains("meerai"));
    assert!(!rendered.contains("> You"));
    assert!(!rendered.contains("◆ Assistant"));
    assert!(rendered.contains("SESSION"));
    assert!(rendered.contains("MCP"));
    assert!(rendered.contains("AGENTS"));
    assert!(rendered.contains("TOOLS"));
    assert!(rendered.contains("File Changes"));
}

#[test]
fn renders_agent_and_skill_mention_badges() {
    use meerai_common::definitions::{AgentDefinition, DefinitionSource, SkillDefinition};

    let mut app = test_app();
    app.agents = vec![AgentDefinition {
        id: "reviewer".to_string(),
        source: DefinitionSource::Agents,
        description: Some(
            "Reviews changes and reports the highest-risk findings first.".to_string(),
        ),
        model: None,
        steps: None,
        mode: None,
        prompt: String::new(),
        path: PathBuf::from(".agents/agents/reviewer.md"),
    }];
    app.skills = vec![SkillDefinition {
        id: "reviewer".to_string(),
        source: DefinitionSource::Claude,
        description: Some("Plans a change and verifies the final result.".to_string()),
        body: String::new(),
        path: PathBuf::from(".claude/skills/reviewer/SKILL.md"),
    }];

    for ch in "@reviewer".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
    }
    assert!(app.completion.is_active());

    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("Mentions"));
    assert!(rendered.contains("reviewer"));
    assert!(!rendered.contains("agent reviewer"));
    assert!(!rendered.contains("skill reviewer"));
    assert!(
        rendered.contains(".agents — Reviews changes and reports the highest-risk findings first.")
    );
    assert!(rendered.contains(".claude — Plans a change and verifies the final result."));
    assert!(!rendered.contains("@agent/reviewer"));
    assert!(!rendered.contains("@skill/reviewer"));
}

#[test]
fn composer_renders_prompt_and_marker() {
    let mut app = test_app();
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("Ask meerai anything"));
    assert!(rendered.contains("❯"));
}

#[test]
fn composer_renders_known_mention_chips() {
    let agents = vec![agent_definition("reviewer")];
    let skills = vec![skill_definition("deliver")];

    let (spans, cursor) = render_composer_line("@skill/deliver", Some(14), &agents, &skills);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content, "deliver");
    assert_eq!(spans[0].style.fg, Some(theme::p().interactive));
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(cursor, 7);

    let (spans, cursor) = render_composer_line("@agent/reviewer", Some(15), &agents, &skills);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].content, "reviewer");
    assert_eq!(spans[0].style.fg, Some(theme::p().accent));
    assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
    assert_eq!(cursor, 8);
}

#[test]
fn composer_keeps_unknown_and_partial_mentions_raw() {
    let agents = vec![agent_definition("reviewer")];
    let skills = vec![skill_definition("deliver")];

    let (spans, cursor) = render_composer_line("@skill/doesnotexist", Some(19), &agents, &skills);
    assert_eq!(span_text(&spans), "@skill/doesnotexist");
    assert_eq!(cursor, 19);

    let (spans, cursor) = render_composer_line("@skill/del", Some(10), &agents, &skills);
    assert_eq!(span_text(&spans), "@skill/del");
    assert_eq!(cursor, 10);

    let (spans, _) = render_composer_line("@agent/nope @skill/deliver", None, &agents, &skills);
    assert_eq!(span_text(&spans), "@agent/nope deliver");
}

#[test]
fn composer_renders_multiple_chips_and_surrounding_text() {
    let agents = vec![agent_definition("reviewer")];
    let skills = vec![skill_definition("deliver")];

    let line = "run @skill/deliver then @agent/reviewer now";
    let (spans, _) = render_composer_line(line, None, &agents, &skills);
    let texts: Vec<String> = spans.iter().map(|span| span.content.to_string()).collect();
    assert_eq!(texts, vec!["run ", "deliver", " then ", "reviewer", " now"]);
    assert_eq!(span_text(&spans), "run deliver then reviewer now");
    assert_eq!(spans[1].style.fg, Some(theme::p().interactive));
    assert_eq!(spans[3].style.fg, Some(theme::p().accent));
}

#[test]
fn composer_cursor_column_uses_chip_display_width() {
    let agents = vec![agent_definition("reviewer")];
    let skills = vec![skill_definition("deliver")];
    let line = "x @skill/deliver";

    let (_, before) = render_composer_line(line, Some(2), &agents, &skills);
    assert_eq!(before, 2);

    let (_, inside) = render_composer_line(line, Some(6), &agents, &skills);
    assert_eq!(inside, 9);

    let (_, after) = render_composer_line(line, Some(line.chars().count()), &agents, &skills);
    assert_eq!(after, 9);

    let (_, middle) = render_composer_line(line, Some(4), &agents, &skills);
    assert_eq!(middle, 9);
}

#[test]
fn composer_cursor_keeps_multiline_row_and_chip_columns() {
    let agents = vec![agent_definition("reviewer")];
    let skills = vec![skill_definition("deliver")];
    let input = "first line\nrun @skill/deliver";
    let cursor = input.len();

    let (row, raw_col) = cursor_row_col(input, cursor);
    assert_eq!(row, 1);

    let line = input.split('\n').nth(row).unwrap();
    let (spans, display_col) = render_composer_line(line, Some(raw_col), &agents, &skills);
    assert_eq!(span_text(&spans), "run deliver");
    assert_eq!(display_col, "run deliver".chars().count());
}

#[test]
fn statusline_renders_approval_mode_badge() {
    let mut app = test_app();
    app.tab_mut().approval_mode = ApprovalMode::Ask;
    let ask = render_to_string(&mut app);
    assert!(ask.contains("ASK"));
    assert!(ask.contains("INS"));

    app.tab_mut().approval_mode = ApprovalMode::Auto;
    assert!(render_to_string(&mut app).contains("AUTO"));

    app.tab_mut().approval_mode = ApprovalMode::Plan;
    assert!(render_to_string(&mut app).contains("PLAN"));
}

#[test]
fn acp_tab_renders_backend_badge_and_external_agent_header() {
    let mut app = test_app();
    assert!(render_to_string(&mut app).contains("gemini / gemini-3.8-flash"));

    app.tab_mut().backend = crate::Backend::Acp {
        preset: "agy".to_string(),
        remote_session_id: None,
    };
    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("ACP:agy"));
    assert!(rendered.contains("agy (acp)"));
    assert!(rendered.contains("policy not applied"));
    assert!(rendered.contains("ASK"));
    assert!(!rendered.contains("gemini / gemini-3.8-flash"));
}

#[test]
fn initial_acp_backend_renders_badge_and_header() {
    let mut app = test_app();
    app.set_initial_session(
        meerai_session::SessionId::from_raw("sess-acp-launch"),
        Some("ACP launch".to_string()),
        crate::Backend::Acp {
            preset: "agy".to_string(),
            remote_session_id: None,
        },
        Vec::new(),
        &[],
        ApprovalMode::Ask,
    );

    assert!(app.tab().is_acp());
    assert_eq!(app.tab().acp_preset(), Some("agy"));

    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("ACP:agy"));
    assert!(rendered.contains("agy (acp)"));
    assert!(rendered.contains("policy not applied"));
}

#[test]
fn renders_approval_overlay_with_preview_and_key_hints() {
    use meerai_agents::events::{ApprovalKind, ApprovalPreview, ApprovalRequest, RequestId};
    use meerai_core::EffectClass;

    let mut app = test_app();
    let request = ApprovalRequest {
        request_id: RequestId::from_raw("req-ui"),
        turn_id: TurnId::from_raw("turn-ui-approval"),
        kind: ApprovalKind::ToolCall,
        preview: ApprovalPreview {
            title: "edit: src/main.rs".to_string(),
            detail: "tool: code-write_file\neffect: write\nresource: src/main.rs".to_string(),
        },
        effect: Some(EffectClass::Write),
    };
    app.handle_agent_event(AgentEvent::ApprovalRequested { request });

    let rendered = render_to_string(&mut app);
    assert!(rendered.contains("Approval required"));
    assert!(rendered.contains("edit: src/main.rs"));
    assert!(rendered.contains("kind: tool call · effect: write"));
    assert!(rendered.contains("resource: src/main.rs"));
    assert!(rendered.contains("y accept"));
    assert!(rendered.contains("t turn"));
    assert!(rendered.contains("a session"));
    assert!(rendered.contains("esc cancel"));

    app.handle_key_event(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::NONE));
    let editing = render_to_string(&mut app);
    assert!(editing.contains("Reason"));
    assert!(editing.contains("Enter:send decline reason"));

    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    let second = ApprovalRequest {
        request_id: RequestId::from_raw("req-ui-2"),
        turn_id: TurnId::from_raw("turn-ui-approval"),
        kind: ApprovalKind::ToolCall,
        preview: ApprovalPreview {
            title: "shell: cargo test".to_string(),
            detail: "tool: code-shell\neffect: execute".to_string(),
        },
        effect: Some(EffectClass::Execute),
    };
    app.handle_agent_event(AgentEvent::ApprovalRequested { request: second });
    let queued = render_to_string(&mut app);
    assert!(queued.contains("Approval required (2)"));
    assert!(queued.contains("edit: src/main.rs"));
    assert!(!queued.contains("shell: cargo test"));
}

#[test]
fn formats_compact_tokens_and_ms() {
    assert_eq!(format_compact_tokens(0), "0");
    assert_eq!(format_compact_tokens(999), "999");
    assert_eq!(format_compact_tokens(1_000), "1k");
    assert_eq!(format_compact_tokens(1_100), "1.1k");
    assert_eq!(format_compact_tokens(1_000_000), "1M");
    assert_eq!(format_compact_tokens(2_500_000), "2.5M");

    assert_eq!(format_compact_ms(45), "45ms");
    assert_eq!(format_compact_ms(1_420), "1.4s");
    assert_eq!(format_compact_ms(65_000), "1m5s");
}

#[test]
fn renders_collapsible_thinking_and_records_click_hit_zone() {
    let mut app = test_app();
    app.file_changes.push(crate::app::FileChangeEntry {
        status: "M".to_string(),
        path: "meerai-tui/src/ui.rs".to_string(),
    });
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "hi".to_string(),
    });
    app.handle_agent_event(AgentEvent::TurnStarted {
        turn_id: TurnId::from_raw("turn-ui-thinking"),
        model: "gemini::gemini-4-hydrogen".to_string(),
    });
    let reasoning_id = ItemId::from_raw("item-ui-thinking");
    app.handle_agent_event(AgentEvent::ItemStarted {
        item: SessionItem::Reasoning {
            id: reasoning_id.clone(),
            summary: String::new(),
        },
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: reasoning_id,
        delta: ItemDelta::ReasoningSummary("secret thought step 1\nstep 2".to_string()),
    });
    app.handle_agent_event(AgentEvent::ItemDelta {
        item_id: ItemId::from_raw("item-ui-thinking-text"),
        delta: ItemDelta::AgentText("Hello!".to_string()),
    });

    app.tab_mut().turn_activity.insert(0, true);
    let collapsed = render_to_string(&mut app);
    assert!(collapsed.contains("+ Thinking (2 lines)"));
    assert!(!collapsed.contains("secret thought step 1"));
    assert!(collapsed.contains("File Changes (1)"));
    assert!(collapsed.contains("meerai-tui/src/ui.rs"));
    assert!(!app.thinking_hit_zones.is_empty());

    let zone = app
        .thinking_hit_zones
        .iter()
        .find(|zone| zone.entry_idx == 1)
        .copied()
        .expect("thinking hit zone");
    let click_row = app.transcript_inner_area.y + zone.start_line as u16;
    let click_col = app.transcript_inner_area.x + 2;
    assert!(app.toggle_entry_at(click_col, click_row));

    let expanded = render_to_string(&mut app);
    assert!(expanded.contains("- Thinking (2 lines)"));
    assert!(expanded.contains("secret thought step 1"));
}

#[test]
fn renders_provider_apikey_masked_and_model_modals() {
    let mut app = test_app();

    app.modal = Some(ActiveModal::ProviderSelect {
        filter: String::new(),
        selected: 0,
    });
    let provider_screen = render_to_string(&mut app);
    assert!(provider_screen.contains("Connect Provider (/connect)"));

    app.modal = Some(ActiveModal::ApiKeyInput {
        provider_id: "opencode-go".to_string(),
        provider_name: "OpenCode Go".to_string(),
        api_key: "sk-super-secret".to_string(),
        base_url: String::new(),
        editing_base_url: false,
        existing_key_status: KeyStatus::Missing,
        error: None,
    });
    let key_screen = render_to_string(&mut app);
    assert!(key_screen.contains("Connect OpenCode Go (opencode-go)"));
    assert!(key_screen.contains("•••••••••••••••"));
    assert!(!key_screen.contains("sk-super-secret"));

    app.modal = Some(ActiveModal::ModelInput {
        input: String::new(),
        selected_recent: None,
        error: None,
    });
    let model_screen = render_to_string(&mut app);
    assert!(model_screen.contains("Set Model [gemini] (/model)"));

    app.modal = Some(ActiveModal::DiffView {
        file_index: 0,
        diff: crate::diff::FileDiffView {
            path: "src/lib.rs".to_string(),
            status: "M".to_string(),
            additions: 1,
            deletions: 1,
            rows: vec![crate::diff::DiffRow {
                kind: DiffRowKind::Modified,
                left: Some(DiffCell {
                    line_no: Some(10),
                    text: "old_val".to_string(),
                }),
                right: Some(DiffCell {
                    line_no: Some(10),
                    text: "new_val".to_string(),
                }),
            }],
        },
        scroll: 0,
    });
    let diff_screen = render_to_string(&mut app);
    assert!(diff_screen.contains("Original (a/src/lib.rs)"));
    assert!(diff_screen.contains("Working Tree (b/src/lib.rs)"));
    assert!(diff_screen.contains("old_val"));
    assert!(diff_screen.contains("new_val"));

    app.modal = Some(ActiveModal::Search {
        query: "lib".to_string(),
        mode: SearchMode::Files,
        workspace_files: vec!["src/lib.rs".to_string()],
        results: vec![crate::search::SearchMatch {
            path: "src/lib.rs".to_string(),
            line: Some(10),
            column: Some(1),
            snippet: "pub fn lib()".to_string(),
        }],
        selected: 0,
        preview: Some(crate::search::SearchPreview {
            title: "src/lib.rs:10".to_string(),
            lines: vec![(10, "pub fn lib() {}".to_string(), true)],
            scroll: 0,
        }),
    });
    let search_screen = render_to_string(&mut app);
    assert!(search_screen.contains("Search [Files] (1 matches) (/search)"));
    assert!(search_screen.contains("Preview: src/lib.rs:10"));
}
#[test]
fn renders_collapsible_single_line_error_and_expands_on_click() {
    let mut app = test_app();
    app.handle_agent_event(AgentEvent::Error {
        code: ErrorCode::Provider,
        message: "LLM stream request failed: WebModelCall { body: \"{\\\"error\\\":{\\\"code\\\":\\\"MissingSessionID\\\",\\\"message\\\":\\\"Request is missing x-opencode-session\\\"}}\", headers: { foo: bar }\nline 2 details }".to_string(),
    });

    let collapsed = render_to_string(&mut app);
    assert!(
        collapsed.contains("+ [error] MissingSessionID: Request is missing x-opencode-session")
    );
    assert!(!collapsed.contains("line 2 details"));
    assert!(!collapsed.contains("- [error]"));
    assert_eq!(app.thinking_hit_zones.len(), 1);

    let zone = app.thinking_hit_zones[0];
    let click_row = app.transcript_inner_area.y + zone.start_line as u16;
    let click_col = app.transcript_inner_area.x + 2;
    assert!(app.toggle_entry_at(click_col, click_row));

    let expanded = render_to_string(&mut app);
    assert!(expanded.contains("- [error] (2 lines)"));
    assert!(expanded.contains("line 2 details"));

    let zone2 = app.thinking_hit_zones[0];
    let click_row2 = app.transcript_inner_area.y + zone2.start_line as u16;
    assert!(app.toggle_entry_at(click_col, click_row2));
    let recollapsed = render_to_string(&mut app);
    assert!(recollapsed.contains("+ [error]"));
    assert!(!recollapsed.contains("line 2 details"));
}

#[test]
fn renders_session_tabs_badges_and_session_select_modal() {
    let mut app = test_app();
    app.tab_mut().title = Some("First Tab".to_string());
    app.tab_mut().session_id = meerai_session::SessionId::from_raw("sess-first");

    let (tx2, _rx2) = mpsc::unbounded_channel();
    let (_etx2, erx2) = mpsc::unbounded_channel();
    app.apply_opened_session(
        crate::OpenedSession {
            session_id: meerai_session::SessionId::from_raw("sess-second"),
            title: Some("Second Tab".to_string()),
            backend: crate::Backend::Native,
            provider: "gemini".to_string(),
            model: "gemini-3.8-flash".to_string(),
            approval_mode: ApprovalMode::Ask,
            transcript: Vec::new(),
            events: Vec::new(),
            cmd_tx: tx2,
            event_rx: erx2,
        },
        false,
    );
    app.switch_to_tab(0);
    app.tabs[1].has_unread = true;

    let tab_bar_screen = render_to_string(&mut app);
    assert!(tab_bar_screen.contains("1: First Tab"));
    assert!(tab_bar_screen.contains("2: Second Tab •"));
    assert!(tab_bar_screen.contains(" + "));
    assert!(app.tab_hit_zones.len() >= 3);

    app.modal = Some(ActiveModal::SessionSelect {
        filter: String::new(),
        selected: 0,
        sessions: vec![
            meerai_session::SessionSummary {
                id: meerai_session::SessionId::from_raw("sess-first"),
                dir: PathBuf::from("/tmp/sess-first"),
                title: Some("First Tab".to_string()),
                model: Some("gemini-3.8-flash".to_string()),
                created_at_ms: 100,
                updated_at_ms: 200,
                bytes: 64,
                parent_session_id: None,
            },
            meerai_session::SessionSummary {
                id: meerai_session::SessionId::from_raw("sess-second"),
                dir: PathBuf::from("/tmp/sess-second"),
                title: Some("Second Tab".to_string()),
                model: Some("gemini-3.8-flash".to_string()),
                created_at_ms: 80,
                updated_at_ms: 150,
                bytes: 64,
                parent_session_id: None,
            },
            meerai_session::SessionSummary {
                id: meerai_session::SessionId::from_raw("sess-other"),
                dir: PathBuf::from("/tmp/sess-other"),
                title: Some("Other Task".to_string()),
                model: Some("gemini-3.8-flash".to_string()),
                created_at_ms: 50,
                updated_at_ms: 100,
                bytes: 64,
                parent_session_id: None,
            },
        ],
        loading: false,
        error: None,
    });

    let modal_screen = render_to_string(&mut app);
    assert!(modal_screen.contains("Switch Session (/sessions)"));
    assert!(modal_screen.contains("[tab 1*]"));
    assert!(modal_screen.contains("[tab 2]"));
    assert!(modal_screen.contains("Other Task"));
}

#[test]
fn renders_distinct_user_thinking_tool_and_response_roles() {
    let mut app = test_app();
    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "Inspect Cargo.toml".to_string(),
    });
    app.tab_mut()
        .transcript
        .push(TranscriptEntry::ToolCall(crate::ToolCallEntry {
            call_id: "tc-1".to_string(),
            name: "code-read_file".to_string(),
            args: "{\"path\":\"Cargo.toml\"}".to_string(),
            status: ToolCallStatus::Succeeded,
            output: Some("[workspace]\nmembers = []".to_string()),
            expanded: false,
        }));
    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: "# Summary\n```toml\nname = \"meerai\"\n```".to_string(),
        reasoning: "Check workspace layout.".to_string(),
        streaming: false,
        reasoning_expanded: true,
    });
    app.tab_mut().turn_activity.insert(0, true);

    let rendered = render_to_string(&mut app);
    assert!(!rendered.contains("▌ you"));
    assert!(rendered.contains("▌ Inspect Cargo.toml"));
    assert!(rendered.contains("+ Read Cargo.toml [ok]"));
    assert!(rendered.contains("↳ [workspace]"));
    assert!(rendered.contains("members = []"));
    assert!(rendered.contains("- Thought (1 line)"));
    assert!(!rendered.contains("● Response"));
    assert!(rendered.contains("Summary"));
    assert!(rendered.contains("name = \"meerai\""));
    assert!(!rendered.contains("```toml"));
}

#[test]
fn sidebar_padding_hover_and_click_alignment() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = test_app();
    app.file_changes = vec![
        crate::FileChangeEntry {
            status: "M".to_string(),
            path: "src/lib.rs".to_string(),
        },
        crate::FileChangeEntry {
            status: "A".to_string(),
            path: "src/new.rs".to_string(),
        },
    ];

    let mut terminal = Terminal::new(TestBackend::new(120, 28)).expect("terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("draw sidebar");

    let changes = app.sidebar_changes_inner_area;
    assert!(changes.width > 0 && changes.height >= 2);
    let header_row = {
        let buf = terminal.backend().buffer();
        (0..120)
            .map(|x| buf[(x, changes.y - 2)].symbol())
            .collect::<String>()
    };
    assert!(
        header_row.contains("File Changes (2)"),
        "expected File Changes header above row area, got {header_row:?}"
    );

    assert!(app.handle_mouse_event(mouse(MouseEventKind::Moved, changes.x, changes.y)));
    assert_eq!(app.hovered_file_change, Some(0));

    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("redraw hovered row");
    let buf = terminal.backend().buffer();
    assert_eq!(
        buf[(changes.x, changes.y)].bg,
        crate::theme::Palette::default().surface_high
    );

    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        changes.x,
        changes.y
    )));
    assert!(matches!(app.modal, Some(ActiveModal::DiffView { .. })));
}

#[test]
fn renders_floating_toast_with_semantic_severity_colors() {
    let mut app = test_app();
    app.status_notice = Some("Copied 5 chars to clipboard".to_string());

    let mut terminal = Terminal::new(TestBackend::new(120, 28)).expect("terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("draw success toast");

    let palette = crate::theme::Palette::default();
    let buf = terminal.backend().buffer();
    let search_area = app.transcript_inner_area;
    let mut found_toast_cell = None;
    for y in search_area.y.saturating_sub(2)..search_area.y + search_area.height {
        let row_text: String = (0..120).map(|x| buf[(x, y)].symbol()).collect();
        if row_text.contains("Copied 5 chars to clipboard") {
            let x = (search_area.x.saturating_sub(2)..search_area.x + search_area.width + 2)
                .find(|&cx| buf[(cx, y)].symbol() == "▌")
                .expect("toast left stripe");
            found_toast_cell = Some((x, y));
            break;
        }
    }
    let (tx, ty) = found_toast_cell.expect("toast should appear in transcript area");
    assert_eq!(buf[(tx, ty)].bg, palette.surface_high);
    assert_eq!(buf[(tx, ty)].fg, palette.success);
    let full: String = (0..28)
        .flat_map(|y| (0..120).map(move |x| buf[(x, y)].symbol()))
        .collect();
    assert!(!full.contains('┌') && !full.contains('┐'));

    app.status_notice = Some("Failed to load workspace".to_string());
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("draw error toast");
    let buf = terminal.backend().buffer();
    let (ex, ey) = (search_area.x.saturating_sub(2)..search_area.x + search_area.width + 2)
        .find(|&cx| buf[(cx, ty)].symbol() == "▌")
        .map(|cx| (cx, ty))
        .expect("error toast left stripe");
    assert_eq!(buf[(ex, ey)].bg, palette.surface_high);
    assert_eq!(buf[(ex, ey)].fg, palette.error);
}

#[test]
fn modal_and_completion_hover_and_click_interactions() {
    use crossterm::event::{MouseButton, MouseEventKind};

    let mut app = test_app();
    assert!(app.handle_key_event(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)));
    assert!(matches!(
        app.modal,
        Some(ActiveModal::CommandPalette { .. })
    ));

    let _ = render_to_string(&mut app);
    let item_zone = app
        .modal_hit_zones
        .iter()
        .find(|z| matches!(z.kind, ModalHitKind::ListItem(_)))
        .copied()
        .expect("command palette list hit zone");
    let close_zone = app
        .modal_hit_zones
        .iter()
        .find(|z| z.kind == ModalHitKind::Close)
        .copied()
        .expect("modal close hit zone");

    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Moved,
        item_zone.rect.x,
        item_zone.rect.y
    )));
    assert!(app.hovered_modal_hit.is_some());

    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        close_zone.rect.x,
        close_zone.rect.y
    )));
    assert!(app.modal.is_none());

    assert!(app.handle_key_event(KeyEvent::from(KeyCode::Char('/'))));
    let _ = render_to_string(&mut app);
    assert!(!app.completion_hit_zones.is_empty());
    let first = app.completion_hit_zones[0];
    assert!(app.handle_mouse_event(mouse(MouseEventKind::Moved, first.rect.x, first.rect.y)));
    assert_eq!(app.hovered_completion, Some(first.index));

    assert!(app.handle_mouse_event(mouse(
        MouseEventKind::Down(MouseButton::Left),
        first.rect.x,
        first.rect.y
    )));
    assert!(app.tab().input.starts_with('/'));
    assert!(app.tab().input.len() > 1);
}

#[test]
fn empty_transcript_quickstart_and_padded_code_blocks() {
    let mut app = test_app();
    let empty_rendered = render_to_string(&mut app);
    assert!(empty_rendered.contains("Quick start"));
    assert!(!empty_rendered.contains('┌'));

    app.tab_mut().transcript.push(TranscriptEntry::User {
        text: "Show example".to_string(),
    });
    app.tab_mut().transcript.push(TranscriptEntry::Assistant {
        text: "Here is code:\n\n```rust\nlet x = 1;\n```\n".to_string(),
        reasoning: String::new(),
        streaming: false,
        reasoning_expanded: false,
    });

    let mut terminal = Terminal::new(TestBackend::new(120, 28)).expect("terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("draw padded code block");
    let buf = terminal.backend().buffer();
    let area = app.transcript_inner_area;
    let palette = crate::theme::Palette::default();

    let code_row = (area.y..area.y + area.height)
        .find(|&y| {
            let text: String = (area.x..area.x + area.width)
                .map(|x| buf[(x, y)].symbol())
                .collect();
            text.contains("    let x = 1;")
        })
        .expect("indented code line");
    assert_eq!(buf[(area.x + 2, code_row - 1)].bg, palette.surface_high);
    assert_eq!(buf[(area.x + 2, code_row)].bg, palette.surface_high);
    assert_eq!(buf[(area.x + 2, code_row + 1)].bg, palette.surface_high);
}

#[test]
fn renders_logo_header_and_composer_outer_margins() {
    let mut app = test_app();
    let mut terminal = Terminal::new(TestBackend::new(120, 28)).expect("terminal");
    terminal
        .draw(|frame| render(frame, &mut app))
        .expect("draw logo and composer margin");

    let buf = terminal.backend().buffer();
    let palette = crate::theme::Palette::default();
    let full: String = (0..28)
        .flat_map(|y| (0..120).map(move |x| buf[(x, y)].symbol()))
        .collect();
    assert!(full.contains("██      ██"));
    assert!(full.contains("████  ████"));
    assert!(full.contains("██  ██  ██"));
    assert!(full.contains("Workspace:"));

    let block = app.composer_block_area;
    assert_eq!(block.x, 2);
    assert_eq!(buf[(0, block.y)].bg, palette.background);
    assert_eq!(buf[(1, block.y)].bg, palette.background);
    assert_eq!(buf[(block.x, block.y)].bg, palette.surface);
    assert_eq!(buf[(block.x + block.width, block.y)].bg, palette.background);
    assert_eq!(
        buf[(block.x, block.y + block.height)].bg,
        palette.background
    );
}
