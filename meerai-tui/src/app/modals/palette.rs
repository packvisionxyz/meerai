use super::*;

impl AppState {
    pub(crate) fn open_command_palette(&mut self) {
        self.modal = Some(ActiveModal::CommandPalette {
            filter: TextInput::default(),
            selected: 0,
        });
    }
    pub(crate) fn run_palette_action(&mut self, action: PaletteAction) {
        match action {
            PaletteAction::Slash(command) => {
                self.handle_slash_command(command);
            }
            PaletteAction::CopyReply => {
                if !self.copy_latest_reply() {
                    self.status_notice = Some("Nothing to copy".to_string());
                }
            }
            PaletteAction::NextTab => {
                self.next_tab();
            }
            PaletteAction::PreviousTab => {
                self.prev_tab();
            }
            PaletteAction::MoveTabLeft => {
                let active = self.active_tab;
                if active > 0 {
                    self.move_tab(active, active - 1);
                }
            }
            PaletteAction::MoveTabRight => {
                let active = self.active_tab;
                if active + 1 < self.tabs.len() {
                    self.move_tab(active, active + 1);
                }
            }
            PaletteAction::CycleApproval => {
                self.cycle_approval_mode();
            }
            PaletteAction::ToggleChanges => {
                self.toggle_changes_panel();
            }
        }
    }
}
