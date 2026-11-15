use meerai_core::{EffectClass, ToolDefinition, ToolError, ToolOutput, async_trait};
use serde_json::Value;

use meerai_session::{QuestionAnswer, QuestionOption, QuestionPrompt};

pub const QUESTION_TOOL: &str = "question-ask";
const MAX_QUESTIONS: usize = 4;
const MAX_OPTIONS: usize = 8;
const MIN_OPTIONS: usize = 2;
const MAX_HEADER_CHARS: usize = 24;

pub struct QuestionToolset;

#[async_trait]
impl meerai_core::Toolset for QuestionToolset {
    fn name(&self) -> String {
        "question".to_string()
    }

    fn definition(&self) -> Vec<ToolDefinition> {
        vec![ToolDefinition {
            r#type: "function".to_string(),
            name: QUESTION_TOOL.to_string(),
            description: "Ask the user one or more single- or multiple-choice questions. Use this \
                          only when a product or configuration decision blocks progress; do not \
                          use it for permission requests."
                .to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": MAX_QUESTIONS,
                        "items": {
                            "type": "object",
                            "properties": {
                                "header": {
                                    "type": "string",
                                    "description": "Short label, at most 24 characters"
                                },
                                "question": { "type": "string" },
                                "options": {
                                    "type": "array",
                                    "minItems": MIN_OPTIONS,
                                    "maxItems": MAX_OPTIONS,
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": { "type": "string" },
                                            "description": { "type": "string" }
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multiple": {
                                    "type": "boolean",
                                    "description": "Allow more than one selected label"
                                }
                            },
                            "required": ["header", "question", "options"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        }]
    }

    fn contain(&self, fn_name: &str) -> bool {
        fn_name == QUESTION_TOOL
    }

    fn effect_of(&self, _fn_name: &str) -> EffectClass {
        EffectClass::Read
    }

    fn should_spill_output(&self, _fn_name: &str) -> bool {
        false
    }

    async fn invoke(&self, _fn_name: &str, _args: &str) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::Fail(
            "question-ask is handled by the agent harness and needs an interactive client"
                .to_string(),
        ))
    }
}

pub fn parse_questions(args: &str) -> Result<Vec<QuestionPrompt>, String> {
    let value: Value =
        serde_json::from_str(args).map_err(|error| format!("invalid JSON: {error}"))?;
    let items: Vec<&Value> = match value.get("questions") {
        Some(Value::Array(array)) => array.iter().collect(),
        Some(_) => return Err("questions must be an array".to_string()),
        None if value.get("question").is_some() => vec![&value],
        None => return Err("missing questions".to_string()),
    };
    if items.is_empty() || items.len() > MAX_QUESTIONS {
        return Err(format!(
            "questions must contain 1 to {MAX_QUESTIONS} entries"
        ));
    }

    let mut prompts = Vec::with_capacity(items.len());
    for item in items {
        prompts.push(parse_prompt(item)?);
    }
    Ok(prompts)
}

fn parse_prompt(value: &Value) -> Result<QuestionPrompt, String> {
    let header = value
        .get("header")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if header.is_empty() {
        return Err("each question needs a header".to_string());
    }
    if header.chars().count() > MAX_HEADER_CHARS {
        return Err(format!(
            "header '{header}' is longer than {MAX_HEADER_CHARS} characters"
        ));
    }
    let question = value
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if question.is_empty() {
        return Err(format!("question '{header}' has no text"));
    }
    let raw_options = value
        .get("options")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("question '{header}' needs an options array"))?;
    if !(MIN_OPTIONS..=MAX_OPTIONS).contains(&raw_options.len()) {
        return Err(format!(
            "question '{header}' needs {MIN_OPTIONS} to {MAX_OPTIONS} options"
        ));
    }
    let mut options = Vec::with_capacity(raw_options.len());
    for raw in raw_options {
        let option = parse_option(raw).ok_or_else(|| {
            format!("question '{header}' has an option that is neither a string nor a label object")
        })?;
        if option.label.trim().is_empty() {
            return Err(format!("question '{header}' has an empty option label"));
        }
        options.push(option);
    }
    Ok(QuestionPrompt {
        header,
        question,
        options,
        multiple: value
            .get("multiple")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

fn parse_option(value: &Value) -> Option<QuestionOption> {
    if let Some(label) = value.as_str() {
        return Some(QuestionOption {
            label: label.trim().to_string(),
            description: None,
        });
    }
    let label = value.get("label")?.as_str()?.trim().to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string);
    Some(QuestionOption { label, description })
}

pub fn format_answers(answers: &[QuestionAnswer]) -> String {
    serde_json::json!({ "answers": answers }).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_questions_with_string_and_object_options() {
        let prompts = parse_questions(
            r#"{"questions":[{"header":"DB","question":"Which?","options":["Postgres",{"label":"SQLite","description":"local"}],"multiple":true}]}"#,
        )
        .unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].header, "DB");
        assert!(prompts[0].multiple);
        assert_eq!(prompts[0].options[0].label, "Postgres");
        assert_eq!(prompts[0].options[1].description.as_deref(), Some("local"));
    }

    #[test]
    fn accepts_single_question_shorthand() {
        let prompts =
            parse_questions(r#"{"header":"DB","question":"Which?","options":["a","b"]}"#).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].options.len(), 2);
    }

    #[test]
    fn rejects_bad_shapes() {
        assert!(parse_questions("{}").is_err());
        assert!(
            parse_questions(r#"{"questions":[{"header":"DB","question":"Q","options":["only"]}]}"#)
                .is_err()
        );
        assert!(
            parse_questions(
                r#"{"questions":[{"header":"DB","question":"Q","options":["a","b"],"multiple":false}]}"#
            )
            .is_ok()
        );
        assert!(
            parse_questions(
                r#"{"questions":[{"header":"this header is far too long for the modal","question":"Q","options":["a","b"]}]}"#
            )
            .is_err()
        );
    }

    #[test]
    fn formats_answers_as_json() {
        let text = format_answers(&[QuestionAnswer {
            header: "DB".to_string(),
            selected: vec!["Postgres".to_string()],
        }]);
        assert_eq!(
            text,
            r#"{"answers":[{"header":"DB","selected":["Postgres"]}]}"#
        );
    }
}
