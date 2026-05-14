//! Common utilities for local agent SDK commands.

use crate::ai::ambient_agents::AmbientAgentTaskId;
use crate::ai::llms::{LLMId, LLMPreferences};
use warpui::{AppContext, SingletonEntity};

pub fn validate_agent_mode_base_model_id(
    model_id: &str,
    ctx: &AppContext,
) -> anyhow::Result<LLMId> {
    let llm_prefs = LLMPreferences::as_ref(ctx);

    let llm_id: LLMId = model_id.into();
    let valid_ids = llm_prefs
        .get_base_llm_choices_for_agent_mode()
        .map(|info| info.id.clone())
        .collect::<Vec<_>>();

    if valid_ids.contains(&llm_id) {
        Ok(llm_id)
    } else {
        let suggestions = valid_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>()
            .join(", ");
        Err(anyhow::anyhow!(
            "Unknown model id '{model_id}'. Try one of: {suggestions}"
        ))
    }
}

pub(super) fn parse_ambient_task_id(
    run_id: &str,
    error_prefix: &str,
) -> anyhow::Result<AmbientAgentTaskId> {
    run_id
        .parse()
        .map_err(|err| anyhow::anyhow!("{error_prefix} '{run_id}': {err}"))
}

#[cfg(test)]
mod tests {
    use super::parse_ambient_task_id;

    #[test]
    fn parse_ambient_task_id_accepts_valid_ids() {
        let task_id =
            parse_ambient_task_id("550e8400-e29b-41d4-a716-446655440000", "Invalid run ID")
                .unwrap();

        assert_eq!(task_id.to_string(), "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn parse_ambient_task_id_preserves_error_prefix() {
        let err = parse_ambient_task_id("not-a-run-id", "Invalid run ID").unwrap_err();

        assert!(err.to_string().contains("Invalid run ID 'not-a-run-id'"));
    }
}
