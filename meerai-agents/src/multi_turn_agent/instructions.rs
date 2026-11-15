use super::*;

async fn nearest_agents_dir(root: &Path, resolved: &Path) -> Option<PathBuf> {
    let is_directory = tokio::fs::metadata(resolved)
        .await
        .is_ok_and(|meta| meta.is_dir());
    let mut current = if is_directory {
        Some(resolved)
    } else {
        resolved.parent()
    };
    while let Some(directory) = current {
        if directory == root || !directory.starts_with(root) {
            return None;
        }
        let candidate = directory.join("AGENTS.md");
        if tokio::fs::metadata(&candidate)
            .await
            .is_ok_and(|meta| meta.is_file())
        {
            return Some(directory.to_path_buf());
        }
        current = directory.parent();
    }
    None
}

pub(super) fn session_title(prompt: &str) -> Option<String> {
    let line = prompt.lines().find(|line| !line.trim().is_empty())?;
    let mut title = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.is_empty() {
        return None;
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        let end = title
            .char_indices()
            .nth(MAX_TITLE_CHARS)
            .map_or(title.len(), |(index, _)| index);
        title.truncate(end);
        title = title.trim_end().to_string();
    }
    Some(title)
}

fn capped_instructions(text: String) -> String {
    if text.len() <= MAX_SCOPED_INSTRUCTION_BYTES {
        return text;
    }
    let end = floor_char_boundary(&text, MAX_SCOPED_INSTRUCTION_BYTES);
    format!("{}\n\n[truncated]", &text[..end])
}

impl MultiTurnAgent {
    pub(super) async fn apply_pending_steers(
        &mut self,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) -> bool {
        if self.pending_steers.is_empty() {
            return false;
        }

        let steers: Vec<String> = self.pending_steers.drain(..).collect();
        for text in steers {
            self.log_event(
                event_tx,
                SessionEvent::UserMessage {
                    source: InputSource::Steering,
                    content: vec![ContentBlock::Text(text.clone())],
                },
            )
            .await;
            self.chat_history.push(ChatMessage::user(text));
        }
        true
    }

    pub(super) async fn apply_agent_instructions(
        &mut self,
        name: &str,
        instructions: &str,
        model: Option<String>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        self.apply_instructions(
            format!("Agent instructions ({name}):\n\n{instructions}"),
            model,
            event_tx,
        )
        .await;
    }

    pub(super) async fn apply_skill_instructions(
        &mut self,
        name: &str,
        instructions: &str,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        self.apply_instructions(
            format!("Skill instructions ({name}):\n\n{instructions}"),
            None,
            event_tx,
        )
        .await;
    }

    async fn apply_instructions(
        &mut self,
        text: String,
        model: Option<String>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        self.log_event(
            event_tx,
            SessionEvent::UserMessage {
                source: InputSource::System,
                content: vec![ContentBlock::Text(text.clone())],
            },
        )
        .await;
        self.chat_history.push(ChatMessage::user(text));
        if let Some(model) = model {
            self.set_model(model);
        }
    }

    pub(super) async fn inject_scoped_instructions(
        &mut self,
        args: &str,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        let Some(root) = self
            .harness
            .as_ref()
            .map(|harness| harness.session.meta().cwd)
        else {
            return;
        };
        if self.injected_instruction_dirs.len() >= MAX_SCOPED_INSTRUCTION_FILES {
            return;
        }

        let Some(raw) = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|value| value.get("path")?.as_str().map(str::to_string))
        else {
            return;
        };
        let Some(resolved) = resolve_lexical_subpath(&root, &raw) else {
            return;
        };
        let Some(directory) = nearest_agents_dir(&root, &resolved).await else {
            return;
        };
        if self.injected_instruction_dirs.contains(&directory) {
            return;
        }
        let Ok(text) = tokio::fs::read_to_string(directory.join("AGENTS.md")).await else {
            return;
        };
        if text.trim().is_empty() {
            return;
        }

        let relative = directory
            .strip_prefix(&root)
            .unwrap_or(&directory)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let content = format!(
            "Project instructions from {relative}/AGENTS.md:\n\n{}",
            capped_instructions(text)
        );
        self.injected_instruction_dirs.insert(directory);
        self.log_event(
            event_tx,
            SessionEvent::UserMessage {
                source: InputSource::RuntimeContext,
                content: vec![ContentBlock::Text(content.clone())],
            },
        )
        .await;
        self.chat_history.push(ChatMessage::user(content));
    }
}
