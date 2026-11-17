use super::*;
use crate::app::commands::parse_mentions;

#[test]
fn mention_parser_applies_skills_then_agent_and_cleans_text() {
    let agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        None,
        None,
        "Review.",
    )];
    let skills = vec![
        sample_skill("explore", DefinitionSource::Agents, None, "Explore."),
        sample_skill("fix", DefinitionSource::Claude, None, "Fix."),
    ];

    let plan = parse_mentions(
        "please @skill/explore and @skill/fix then @agent/reviewer do it",
        &agents,
        &skills,
    );
    assert_eq!(plan.skill_indices, vec![0, 1]);
    assert_eq!(plan.agent_indices, vec![0]);
    assert_eq!(plan.text.as_deref(), Some("please and then do it"));

    let deduped = parse_mentions(
        "@agent/reviewer @agent/reviewer @skill/fix @skill/fix",
        &agents,
        &skills,
    );
    assert_eq!(deduped.agent_indices, vec![0]);
    assert_eq!(deduped.skill_indices, vec![1]);
    assert_eq!(deduped.text.as_deref(), Some(""));

    let unknown = parse_mentions("@agent/nope @skill/nope keep", &agents, &skills);
    assert!(unknown.agent_indices.is_empty());
    assert!(unknown.skill_indices.is_empty());
    assert_eq!(unknown.text, None);
}

#[test]
fn submitting_mention_applies_agent_then_prompt() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        Some("Reviews changes"),
        Some("gemini-3.1-pro-preview"),
        "Review the diff.",
    )];

    submit(&mut app, "@agent/reviewer do the task");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review the diff.".to_string(),
            model: Some("gemini::gemini-3.1-pro-preview".to_string()),
            context_window_tokens: None,
        }
    );
    assert_eq!(app.tab().model, "gemini-3.1-pro-preview");
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("do the task".to_string())
    );
    assert!(app.tab().is_running);
    assert_eq!(
        app.tab().transcript.last(),
        Some(&TranscriptEntry::User {
            text: "do the task".to_string()
        })
    );
}

#[test]
fn mention_popup_arrows_and_tab_insert_without_submitting() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        Some("Reviews changes"),
        None,
        "Review.",
    )];
    app.skills = vec![sample_skill(
        "reviewer",
        DefinitionSource::Claude,
        Some("Review skill"),
        "Skill.",
    )];

    type_text(&mut app, "@reviewer");
    assert!(app.completion.is_active());
    let items = app.completion.items();
    assert_eq!(items[0].label, "reviewer");
    assert_eq!(items[0].item_kind, CompletionItemKind::Agent);
    assert_eq!(items[1].label, "reviewer");
    assert_eq!(items[1].item_kind, CompletionItemKind::Skill);

    app.handle_key_event(key(KeyCode::Down));
    assert_eq!(app.completion.selected(), 1);
    app.handle_key_event(key(KeyCode::Up));
    assert_eq!(app.completion.selected(), 0);
    app.handle_key_event(key(KeyCode::Down));
    app.handle_key_event(key(KeyCode::Tab));

    assert_eq!(app.tab().input, "@skill/reviewer ");
    assert_eq!(app.mode, InputMode::Insert);
    assert!(!app.tab().is_running);
    assert!(rx.try_recv().is_err());
}

#[test]
fn enter_accepts_mention_without_submitting() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        Some("Reviews changes"),
        None,
        "Review.",
    )];

    type_text(&mut app, "@reviewer");
    assert!(app.completion.is_active());
    app.handle_key_event(key(KeyCode::Enter));

    assert_eq!(app.tab().input, "@agent/reviewer ");
    assert!(!app.tab().is_running);
    assert!(rx.try_recv().is_err());

    app.handle_key_event(key(KeyCode::Enter));
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review.".to_string(),
            model: None,
            context_window_tokens: None,
        }
    );
    assert!(rx.try_recv().is_err());
    assert!(!app.tab().is_running);
    assert!(app.tab().input.is_empty());
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::System { text })
            if text == "Applied agent reviewer (.agents)."
    ));
}

#[test]
fn only_mentions_apply_without_starting_a_turn() {
    let (mut app, mut rx, _secrets) = test_app();
    app.skills = vec![
        sample_skill("explore", DefinitionSource::Agents, None, "Explore."),
        sample_skill("fix", DefinitionSource::Claude, None, "Fix."),
    ];
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Claude,
        None,
        None,
        "Review.",
    )];

    submit(&mut app, "@skill/explore @skill/fix @agent/reviewer");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplySkill {
            name: "explore".to_string(),
            instructions: "Explore.".to_string(),
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplySkill {
            name: "fix".to_string(),
            instructions: "Fix.".to_string(),
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review.".to_string(),
            model: None,
            context_window_tokens: None,
        }
    );
    assert!(rx.try_recv().is_err());
    assert!(!app.tab().is_running);
    assert!(app.tab().input.is_empty());
    assert_eq!(
        app.tab().transcript,
        vec![
            TranscriptEntry::System {
                text: "Applied skill explore (.agents).".to_string()
            },
            TranscriptEntry::System {
                text: "Applied skill fix (.claude).".to_string()
            },
            TranscriptEntry::System {
                text: "Applied agent reviewer (.claude).".to_string()
            },
        ]
    );
}

#[test]
fn unknown_mention_stays_in_the_prompt() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        None,
        None,
        "Review.",
    )];

    submit(&mut app, "@agent/nope do it");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("@agent/nope do it".to_string())
    );
    assert!(matches!(
        app.tab().transcript.last(),
        Some(TranscriptEntry::User { text }) if text == "@agent/nope do it"
    ));
}

#[test]
fn repeated_mention_applies_once() {
    let (mut app, mut rx, _secrets) = test_app();
    app.skills = vec![sample_skill(
        "explore",
        DefinitionSource::Agents,
        None,
        "Explore.",
    )];

    submit(&mut app, "@skill/explore @skill/explore go");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplySkill {
            name: "explore".to_string(),
            instructions: "Explore.".to_string(),
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("go".to_string())
    );
    assert!(rx.try_recv().is_err());
}

#[test]
fn mention_agent_model_with_provider_prefix_is_ignored() {
    let (mut app, mut rx, _secrets) = test_app();
    app.agents = vec![sample_agent(
        "reviewer",
        DefinitionSource::Agents,
        None,
        Some("openrouter:deepseek-v4.1"),
        "Review.",
    )];

    submit(&mut app, "@agent/reviewer go");

    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::ApplyAgent {
            name: "reviewer".to_string(),
            instructions: "Review.".to_string(),
            model: None,
            context_window_tokens: None,
        }
    );
    assert_eq!(
        rx.try_recv().unwrap(),
        AgentCommand::SubmitPrompt("go".to_string())
    );
    assert_eq!(app.tab().model, "gemini-3.8-flash");
    assert!(app.tab().transcript.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::System { text } if text.contains("implies a provider change")
    )));
}
