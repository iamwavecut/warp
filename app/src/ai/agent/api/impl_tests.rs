use super::get_supported_tools;
use crate::ai::agent::api::RequestParams;
use crate::ai::blocklist::SessionContext;
use crate::ai::llms::LLMId;
use crate::terminal::model::session::SessionType;
use warp_core::features::FeatureFlag;
use warp_core::HostId;
use warp_multi_agent_api as api;

fn request_params_with_ask_user_question_enabled(ask_user_question_enabled: bool) -> RequestParams {
    let model = LLMId::from("test-model");

    RequestParams {
        input: vec![],
        request_task_id: None,
        conversation_token: None,
        tasks: vec![],
        session_context: SessionContext::new_for_test(),
        model: model.clone(),
        mcp_context: None,
        should_redact_secrets: false,
        custom_provider_route: None,
        computer_use_enabled: false,
        ask_user_question_enabled,
        remote_codebase_search_available: false,
        orchestration_enabled: false,
        supported_tools_override: None,
        parent_agent_id: None,
        agent_name: None,
    }
}

fn request_params_for_remote(remote_codebase_search_available: bool) -> RequestParams {
    let mut params = request_params_with_ask_user_question_enabled(false);
    params.session_context =
        SessionContext::new_with_session_type_for_test(Some(SessionType::WarpifiedRemote {
            host_id: Some(HostId::new("host".to_string())),
        }));
    params.remote_codebase_search_available = remote_codebase_search_available;
    params
}

#[test]
fn supported_tools_omits_ask_user_question_when_disabled() {
    let params = request_params_with_ask_user_question_enabled(false);
    let supported_tools = get_supported_tools(&params);

    assert!(!supported_tools.contains(&api::ToolType::AskUserQuestion));
}

#[test]
fn supported_tools_includes_ask_user_question_when_enabled_and_feature_flag_is_enabled() {
    if !FeatureFlag::AskUserQuestion.is_enabled() {
        return;
    }

    let params = request_params_with_ask_user_question_enabled(true);
    let supported_tools = get_supported_tools(&params);

    assert!(supported_tools.contains(&api::ToolType::AskUserQuestion));
}

#[test]
fn remote_supported_tools_include_search_codebase_when_index_is_available() {
    let _flag = FeatureFlag::RemoteCodebaseIndexing.override_enabled(true);
    let params = request_params_for_remote(true);
    let supported_tools = get_supported_tools(&params);

    assert!(supported_tools.contains(&api::ToolType::SearchCodebase));
}
#[test]
fn remote_supported_tools_omit_search_codebase_when_feature_flag_is_disabled() {
    let _flag = FeatureFlag::RemoteCodebaseIndexing.override_enabled(false);
    let params = request_params_for_remote(true);
    let supported_tools = get_supported_tools(&params);

    assert!(!supported_tools.contains(&api::ToolType::SearchCodebase));
}

#[test]
fn remote_supported_tools_omit_search_codebase_when_index_is_unavailable() {
    let _flag = FeatureFlag::RemoteCodebaseIndexing.override_enabled(true);
    let params = request_params_for_remote(false);
    let supported_tools = get_supported_tools(&params);

    assert!(!supported_tools.contains(&api::ToolType::SearchCodebase));
}
