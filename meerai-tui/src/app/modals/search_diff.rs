use super::*;

impl AppState {
    pub fn open_diff_modal(&mut self, file_index: usize) {
        let Some(entry) = self.file_changes.get(file_index) else {
            return;
        };
        let diff = build_file_diff(&self.workspace_root, &entry.path, &entry.status);
        self.modal = Some(ActiveModal::DiffView {
            file_index,
            diff,
            scroll: 0,
            list: DiffListState::Closed,
        });
    }
    pub fn open_search_modal(&mut self, mode: SearchMode, initial_query: &str) {
        let workspace_files = self.workspace.list_files();
        let query = TextInput::new(initial_query.trim());
        let mut results = Vec::new();
        let mut selected = 0;
        let mut preview = None;
        Self::refresh_search(
            SearchSources {
                workspace: self.workspace.as_ref(),
                workspace_root: &self.workspace_root,
                mode,
                workspace_files: &workspace_files,
                query: &query,
            },
            SearchOutput {
                results: &mut results,
                selected: &mut selected,
                preview: &mut preview,
            },
        );
        self.modal = Some(ActiveModal::Search {
            query,
            mode,
            workspace_files,
            results,
            selected,
            preview,
        });
    }
    pub(crate) fn refresh_search(sources: SearchSources<'_>, out: SearchOutput<'_>) {
        let SearchSources {
            workspace,
            workspace_root,
            mode,
            workspace_files,
            query,
        } = sources;
        let SearchOutput {
            results,
            selected,
            preview,
        } = out;
        *results = match mode {
            SearchMode::Files => search_files(workspace_files, query),
            SearchMode::Content => search_content(workspace, workspace_files, query),
        };
        *selected = 0;
        *preview = results
            .first()
            .map(|item| build_search_preview(workspace_root, item, 20));
    }
    pub(crate) fn stage_diff_path(
        &mut self,
        path: &str,
        stage: bool,
    ) -> Result<Option<usize>, String> {
        if stage {
            stage_file(&self.workspace_root, path)?;
        } else {
            unstage_file(&self.workspace_root, path)?;
        }
        self.file_changes = list_git_changes(&self.workspace_root);
        Ok(self
            .file_changes
            .iter()
            .position(|entry| entry.path == path))
    }
}
