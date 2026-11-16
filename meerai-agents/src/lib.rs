mod acp_subagent;
mod checkpoint;
pub mod events;
mod harness;
pub mod hooks;
pub mod intercept;
mod multi_turn_agent;
pub mod prompt;
pub mod question;
mod redact;
pub mod screen;
pub mod shell_safety;
pub mod subagent;
mod tool_args;

pub use acp_subagent::{AcpChild, AcpRunToolset, AcpSpawner};
pub use events::{
    AbortCause, AgentCommand, AgentEvent, ApprovalRequest, CallId, ErrorCode, ItemDelta, ItemId,
    ItemStatus, PlanStep, PlanStepStatus, QuestionAnswer, QuestionOption, QuestionPrompt,
    QuestionRequest, RewindScope, SessionItem, TurnEndReason, TurnId, Usage,
};
pub use harness::{AgentHarness, HarnessConfig, TokenPricing};
pub use hooks::{HookContext, HookDecision, HookEngine, HookFiring, HookReport};
pub use intercept::{
    Interceptor, SharedInterceptor, StepAction, StepPlan, ToolContext, ToolOutcome, ToolPlan,
    TurnStopAction,
};
pub use multi_turn_agent::{ChatStreamer, MultiTurnAgent, MultiTurnAgentConfig, spawn_worker};
pub use prompt::{BASE_SYSTEM_PROMPT, compose_system_prompt};
pub use question::{QUESTION_TOOL, QuestionToolset, format_answers, parse_questions};
pub use subagent::{
    BuiltinAgent, READ_ONLY_SUBAGENT_TOOLS, SubagentToolFactory, SubagentToolset, builtin_agents,
    find_builtin_agent,
};

pub mod agents {
    pub use crate::harness::{AgentHarness, HarnessConfig};
    pub use crate::multi_turn_agent::{MultiTurnAgent, MultiTurnAgentConfig};
}
