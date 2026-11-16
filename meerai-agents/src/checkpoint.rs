use meerai_core::{EffectClass, async_trait};
use meerai_session::{CheckpointId, SessionEvent};

use crate::events::ErrorCode;
use crate::intercept::{Interceptor, ToolContext, ToolPlan};

pub struct CheckpointInterceptor;

#[async_trait]
impl Interceptor for CheckpointInterceptor {
    async fn before_tool(&self, plan: &ToolPlan<'_>, ctx: &ToolContext<'_>) -> Vec<SessionEvent> {
        if plan.effect != EffectClass::Write {
            return Vec::new();
        }
        let Some(store) = ctx.snapshots else {
            return Vec::new();
        };
        let Some(path) = crate::tool_args::tool_write_path(plan.args) else {
            return Vec::new();
        };
        match store
            .capture(plan.turn_id, ctx.workspace, std::path::Path::new(&path))
            .await
        {
            Ok(Some(full)) => vec![SessionEvent::CheckpointCreated {
                checkpoint_id: CheckpointId::new(),
                turn_id: plan.turn_id.clone(),
                paths: vec![full],
            }],
            Ok(None) => Vec::new(),
            Err(error) => vec![SessionEvent::Error {
                code: ErrorCode::Internal,
                message: format!("checkpoint failed: {error:#}"),
                retryable: false,
            }],
        }
    }
}
