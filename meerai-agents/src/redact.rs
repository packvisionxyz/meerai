use std::borrow::Cow;
use std::sync::OnceLock;

use meerai_core::{ToolObservation, ToolOutput, ToolRun};
use regex::Regex;

struct SecretPattern {
    kind: &'static str,
    regex: Regex,
}

fn patterns() -> &'static [SecretPattern] {
    static PATTERNS: OnceLock<Vec<SecretPattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            ("anthropic_key", r"sk-ant-[A-Za-z0-9_-]{20,}"),
            ("openai_key", r"sk-[A-Za-z0-9_-]{20,}"),
            ("google_api_key", r"AIza[0-9A-Za-z_-]{35}"),
            ("github_token", r"ghp_[A-Za-z0-9]{36}"),
            ("github_token", r"github_pat_[A-Za-z0-9_]{20,}"),
            ("aws_access_key_id", r"AKIA[0-9A-Z]{16}"),
            ("slack_token", r"xox[baprs]-[A-Za-z0-9-]{10,}"),
            ("bearer_token", r"Bearer\s+[A-Za-z0-9._~+/-]{20,}"),
            (
                "private_key",
                r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
            ),
        ]
        .into_iter()
        .map(|(kind, pattern)| SecretPattern {
            kind,
            regex: Regex::new(pattern).expect("static redaction pattern compiles"),
        })
        .collect()
    })
}

pub(crate) fn redact_text(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    let mut text = Cow::Borrowed(input);
    for pattern in patterns() {
        if !pattern.regex.is_match(&text) {
            continue;
        }
        let replacement = format!("[REDACTED:{}]", pattern.kind);
        text = Cow::Owned(
            pattern
                .regex
                .replace_all(&text, replacement.as_str())
                .into_owned(),
        );
    }
    text.into_owned()
}

pub(crate) fn redact_run(run: &mut ToolRun) {
    redact_observation(&mut run.observation);
    match &mut run.output {
        ToolOutput::Text(text) | ToolOutput::Fail(text) | ToolOutput::Stop(text) => {
            *text = redact_text(text);
        }
    }
    for change in &mut run.file_changes {
        change.unified_diff = redact_text(&change.unified_diff);
    }
}

fn redact_observation(observation: &mut ToolObservation) {
    observation.stdout = redact_text(&observation.stdout);
    observation.stderr = redact_text(&observation.stderr);
    if let Some(hint) = &observation.hint {
        observation.hint = Some(redact_text(hint));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use meerai_core::{FileChangeKind, FileDiff, ObservationStatus};

    #[test]
    fn redacts_each_supported_secret_shape() {
        let google = format!("AIza{}", "A".repeat(35));
        let github = format!("ghp_{}", "a".repeat(36));
        let github_pat = format!("github_pat_{}", "b".repeat(30));
        let slack = format!("xoxb-{}", "1".repeat(12));
        let bearer = format!("Bearer {}", "t".repeat(24));
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----";

        let cases: [(&str, &str); 9] = [
            ("sk-abcdefghijklmnopqrstuvwxyz012345", "openai_key"),
            ("sk-ant-api03-abcdefghijklmnopqrstuvwxyz", "anthropic_key"),
            (google.as_str(), "google_api_key"),
            (github.as_str(), "github_token"),
            (github_pat.as_str(), "github_token"),
            ("AKIAIOSFODNN7EXAMPLE", "aws_access_key_id"),
            (slack.as_str(), "slack_token"),
            (bearer.as_str(), "bearer_token"),
            (pem, "private_key"),
        ];

        for (secret, kind) in cases {
            let redacted = redact_text(&format!("before {secret} after"));
            assert_eq!(
                redacted,
                format!("before [REDACTED:{kind}] after"),
                "secret: {secret}"
            );
        }
    }

    #[test]
    fn leaves_ordinary_text_unchanged() {
        let text = "fn main() {\n    println!(\"no secrets here\");\n}";
        assert_eq!(redact_text(text), text);
        assert_eq!(redact_text(""), "");
    }

    #[test]
    fn redacts_run_observation_output_and_diff_text() {
        let secret = "ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let mut run = ToolRun {
            output: ToolOutput::Text(format!("token {secret}")),
            observation: ToolObservation {
                status: ObservationStatus::Error,
                exit_code: Some(1),
                stdout: format!("out {secret}"),
                stderr: format!("err {secret}"),
                hint: Some(format!("hint {secret}")),
            },
            file_changes: vec![FileDiff {
                path: "src/a.rs".to_string(),
                kind: FileChangeKind::Modified,
                unified_diff: format!("+token={secret}"),
            }],
        };

        redact_run(&mut run);

        let rendered = format!(
            "{:?} {:?} {:?} {:?}",
            run.output, run.observation, run.file_changes, run
        );
        assert!(!rendered.contains(secret));
        assert!(rendered.contains("[REDACTED:github_token]"));
        assert_eq!(run.observation.exit_code, Some(1));
        assert_eq!(run.file_changes[0].path, "src/a.rs");
    }
}
