use super::*;
use meerai_host::HostAction;

fn app_with_workspace() -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.rs"), "fn main() {}").unwrap();
    let (mut app, _rx, _secrets) = test_app();
    app.workspace_root = dir.path().to_path_buf();
    app.transcript_area = Rect::new(2, 5, 80, 20);
    (app, dir)
}

fn lines(app: &mut AppState, rows: &[(&str, usize)]) {
    app.transcript_wrapped_lines = rows
        .iter()
        .map(|(text, gutter)| TranscriptWrappedLine {
            text: (*text).to_string(),
            gutter_len: *gutter,
            hang_len: 0,
            soft_wrapped_next: false,
        })
        .collect();
}

#[test]
fn resolves_a_path_under_the_clicked_cell() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("", 0), ("see src/main.rs:12 now", 0)]);

    let reference = app.path_reference_at(2 + 4, 5 + 1).unwrap();
    assert_eq!(reference.path, "src/main.rs");
    assert_eq!(reference.line, Some(12));
}

#[test]
fn resolves_at_every_character_of_the_path() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("src/main.rs", 0)]);

    for column in 0.."src/main.rs".len() as u16 {
        let reference = app
            .path_reference_at(2 + column, 5)
            .unwrap_or_else(|| panic!("column {column} did not resolve"));
        assert_eq!(reference.path, "src/main.rs");
    }
}

#[test]
fn ignores_clicks_outside_the_path_token() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("see src/main.rs now", 0)]);

    assert!(app.path_reference_at(2, 5).is_none());
    assert!(app.path_reference_at(2 + 17, 5).is_none());
}

#[test]
fn ignores_clicks_in_the_gutter() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("src/main.rs", 3)]);

    assert!(app.path_reference_at(2, 5).is_none());
    assert!(app.path_reference_at(2 + 3, 5).unwrap().path == "src/main.rs");
}

#[test]
fn ignores_clicks_outside_the_transcript_area() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("src/main.rs", 0)]);

    assert!(app.path_reference_at(2 + 4, 4).is_none());
    assert!(app.path_reference_at(2 + 4, 5 + 20).is_none());
    assert!(app.path_reference_at(1, 5).is_none());
}

#[test]
fn ignores_clicks_past_the_last_wrapped_line() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("src/main.rs", 0)]);

    assert!(app.path_reference_at(2 + 4, 5 + 5).is_none());
}

#[test]
fn ignores_paths_that_do_not_exist_in_the_workspace() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("src/absent.rs", 0)]);

    assert!(app.path_reference_at(2 + 3, 5).is_none());
}

#[test]
fn ignores_absolute_and_escaping_paths() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("/etc/passwd", 0), ("../outside.rs", 0)]);

    assert!(app.path_reference_at(2 + 3, 5).is_none());
    assert!(app.path_reference_at(2 + 3, 6).is_none());
}

#[test]
fn ignores_plain_words_and_urls() {
    let (mut app, _dir) = app_with_workspace();
    lines(
        &mut app,
        &[("https://example.com/a.rs", 0), ("memory-search", 1)],
    );

    assert!(app.path_reference_at(2 + 6, 5).is_none());
    assert!(app.path_reference_at(2 + 3, 6).is_none());
}

#[test]
fn click_queues_an_open_action_for_a_resolved_path() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("src/main.rs:7", 0)]);

    assert!(app.click_path_at(2 + 2, 5));
    let queued = app
        .pending_session_actions
        .iter()
        .find_map(|action| match action {
            PendingSessionAction::HostAction { action, .. } => Some(action.clone()),
            _ => None,
        })
        .expect("a host action is queued");
    assert_eq!(
        queued,
        HostAction::OpenInEditor {
            path: "src/main.rs".to_string(),
            line: Some(7),
        }
    );
}

#[test]
fn click_does_nothing_when_no_path_is_under_the_cursor() {
    let (mut app, _dir) = app_with_workspace();
    lines(&mut app, &[("nothing to open here", 0)]);

    assert!(!app.click_path_at(2 + 3, 5));
    assert!(
        !app.pending_session_actions
            .iter()
            .any(|action| matches!(action, PendingSessionAction::HostAction { .. }))
    );
}
