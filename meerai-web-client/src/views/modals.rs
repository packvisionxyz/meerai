use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use leptos::prelude::*;
use leptos::tachys::view::any_view::AnyView;
use meerai_web_protocol::{
    ApprovalDecisionDto, AuthKindDto, BackendKind, FileDiffResponse, GrantScopeDto, ModelItemDto,
    ProviderItemDto, RewindScopeDto, SearchModeDto,
};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;

use crate::keymap::build_question_answers;
use crate::state::{
    ActiveModal, ProviderModalReturn, RunKindDto, activate_provider_from_modal, back_to_runs,
    custom_model_candidate, execute_palette_action, execute_slash_command, filtered_commands,
    filtered_models_sorted, filtered_palette_actions, filtered_providers_sorted,
    format_duration_ms, format_tokens, is_recent_model, is_removable_credential_badge,
    open_api_key_modal, open_provider_credential_update, open_provider_model_select,
    open_provider_select, open_run_row, return_from_provider_modal, running_subagent_rows,
    set_status_notice, start_oauth_modal, update_completion,
};

mod chrome;
mod dispatch;
mod input;
mod provider;
mod session_ops;
mod workspace;
use crate::store::StoreHandle;
use crate::views::command_label;
use crate::views::settings::SettingsModal;
pub use chrome::*;
pub use dispatch::*;
pub use input::*;
pub use provider::*;
pub use session_ops::*;
pub use workspace::*;
