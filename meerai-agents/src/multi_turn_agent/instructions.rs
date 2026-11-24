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
        context_window_tokens: Option<u64>,
        effort_levels: Option<Vec<EffortLevel>>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        self.apply_instructions(
            format!("Agent instructions ({name}):\n\n{instructions}"),
            model,
            context_window_tokens,
            effort_levels,
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
            None,
            None,
            event_tx,
        )
        .await;
    }

    async fn apply_instructions(
        &mut self,
        text: String,
        model: Option<String>,
        context_window_tokens: Option<u64>,
        effort_levels: Option<Vec<EffortLevel>>,
        event_tx: &mpsc::UnboundedSender<AgentEvent>,
    ) {
        let text = crate::screen::screen_untrusted_text("definition_instructions", &text);
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
            if let Some(tokens) = context_window_tokens {
                self.set_context_window_tokens(tokens);
            }
            self.set_effort_levels(effort_levels);
            self.persist_model().await;
        }
    }

    pub(super) async fn scoped_instructions(&mut self, args: &str) -> Option<String> {
        let root = self
            .harness
            .as_ref()
            .map(|harness| harness.session.meta().cwd)?;
        if self.injected_instruction_dirs.len() >= MAX_SCOPED_INSTRUCTION_FILES {
            return None;
        }

        let raw = serde_json::from_str::<serde_json::Value>(args)
            .ok()
            .and_then(|value| value.get("path")?.as_str().map(str::to_string))?;
        let resolved = resolve_lexical_subpath(&root, &raw)?;
        let directory = nearest_agents_dir(&root, &resolved).await?;
        if self.injected_instruction_dirs.contains(&directory) {
            return None;
        }
        let text = tokio::fs::read_to_string(directory.join("AGENTS.md"))
            .await
            .ok()?;
        if text.trim().is_empty() {
            return None;
        }

        let relative = directory
            .strip_prefix(&root)
            .unwrap_or(&directory)
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        let source = format!("{relative}/AGENTS.md");
        let body = crate::screen::screen_untrusted_text(&source, &capped_instructions(text));
        let content = format!("Project instructions from {source}:\n\n{body}");
        self.injected_instruction_dirs.insert(directory);
        self.chat_history.push(ChatMessage::user(content.clone()));
        Some(content)
    }
}
