use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use meerai_input::TextInput;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextAction {
    Ignored,
    Edited,
    Submit,
}

pub fn handle_text_key(
    input: &mut TextInput,
    key: KeyEvent,
    width: usize,
    multiline: bool,
    vertical: bool,
) -> TextAction {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);

    match key.code {
        KeyCode::Char('a' | 'A') if ctrl => {
            input.select_all();
            TextAction::Edited
        }
        KeyCode::Char('u' | 'U') if ctrl => {
            input.delete_to_line_start();
            TextAction::Edited
        }
        KeyCode::Char('k' | 'K') if ctrl => {
            input.delete_to_line_end();
            TextAction::Edited
        }
        KeyCode::Char('w' | 'W') if ctrl => {
            input.delete_word_backward();
            TextAction::Edited
        }
        KeyCode::Backspace if alt => {
            input.delete_word_backward();
            TextAction::Edited
        }
        KeyCode::Delete if alt => {
            input.delete_word_forward();
            TextAction::Edited
        }
        KeyCode::Left if ctrl && shift => {
            input.extend_word_left();
            TextAction::Edited
        }
        KeyCode::Right if ctrl && shift => {
            input.extend_word_right();
            TextAction::Edited
        }
        KeyCode::Left if ctrl => {
            input.move_word_left();
            TextAction::Edited
        }
        KeyCode::Right if ctrl => {
            input.move_word_right();
            TextAction::Edited
        }
        KeyCode::Left if shift => {
            input.extend_left();
            TextAction::Edited
        }
        KeyCode::Right if shift => {
            input.extend_right();
            TextAction::Edited
        }
        KeyCode::Left => {
            input.move_left();
            TextAction::Edited
        }
        KeyCode::Right => {
            input.move_right();
            TextAction::Edited
        }
        KeyCode::Home if ctrl => {
            input.move_document_start();
            TextAction::Edited
        }
        KeyCode::End if ctrl => {
            input.move_document_end();
            TextAction::Edited
        }
        KeyCode::Home if shift => {
            input.extend_home();
            TextAction::Edited
        }
        KeyCode::End if shift => {
            input.extend_end();
            TextAction::Edited
        }
        KeyCode::Home => {
            input.move_home();
            TextAction::Edited
        }
        KeyCode::End => {
            input.move_end();
            TextAction::Edited
        }
        KeyCode::Up if vertical && shift => {
            input.extend_up(width);
            TextAction::Edited
        }
        KeyCode::Down if vertical && shift => {
            input.extend_down(width);
            TextAction::Edited
        }
        KeyCode::Up if vertical => {
            input.move_up(width);
            TextAction::Edited
        }
        KeyCode::Down if vertical => {
            input.move_down(width);
            TextAction::Edited
        }
        KeyCode::Backspace => {
            input.backspace();
            TextAction::Edited
        }
        KeyCode::Delete => {
            input.delete_forward();
            TextAction::Edited
        }
        KeyCode::Char('j' | 'J') if ctrl && multiline => {
            input.insert_newline();
            TextAction::Edited
        }
        KeyCode::Enter if multiline && (shift || alt || ctrl) => {
            input.insert_newline();
            TextAction::Edited
        }
        KeyCode::Enter => TextAction::Submit,
        KeyCode::Char(c) if !ctrl && !alt => {
            input.insert_char(c);
            TextAction::Edited
        }
        _ => TextAction::Ignored,
    }
}
