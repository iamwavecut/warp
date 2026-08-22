use itertools::Itertools;
use warp_core::features::FeatureFlag;
use warpui::{Entity, ModelHandle, SingletonEntity};

use crate::ai::skills::{SkillManager, SkillManagerEvent};
use crate::search::SyncDataSource;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::DataSourceRunErrorWrapper;
use crate::settings::AISettings;
use crate::terminal::input::slash_commands::{
    AcceptSlashCommandOrLocalPrompt, GuiSlashCommandDataSource, InlineItem, SlashCommandDataSource,
};
use crate::user_config::{WarpConfig, WarpConfigUpdateEvent};

/// Returns whether filesystem-backed local prompts can be shown in the zero-state menu.
///
/// This gate is intentionally kept separate from the source lookup so the local-first policy is
/// testable without constructing the full GUI data source.
pub(super) fn should_show_local_prompts(is_ai_enabled: bool) -> bool {
    is_ai_enabled
}

/// Returns whether local skills can be shown in the zero-state menu.
pub(super) fn should_show_local_skills(is_ai_enabled: bool, is_list_skills_enabled: bool) -> bool {
    is_ai_enabled && is_list_skills_enabled
}

/// Returns whether a dependency event can change the contents of the zero-state menu.
pub(super) fn is_zero_state_workflow_event(event: &WarpConfigUpdateEvent) -> bool {
    matches!(event, WarpConfigUpdateEvent::LocalUserWorkflows)
}

/// Returns whether a skill-manager event can change the contents of the zero-state menu.
pub(super) fn is_zero_state_skill_event(event: &SkillManagerEvent) -> bool {
    matches!(event, SkillManagerEvent::SkillsChanged)
}

/// Event emitted by a zero-state source after one of its local backing stores changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdatedZeroState;

pub struct GuiZeroStateDataSource {
    slash_command_data_source: ModelHandle<GuiSlashCommandDataSource>,
}

impl GuiZeroStateDataSource {
    pub fn new(
        slash_command_data_source: &ModelHandle<GuiSlashCommandDataSource>,
        ctx: &mut warpui::ModelContext<Self>,
    ) -> Self {
        ctx.subscribe_to_model(&WarpConfig::handle(ctx), |_, _, event, ctx| {
            if is_zero_state_workflow_event(event) {
                ctx.emit(UpdatedZeroState);
            }
        });
        ctx.subscribe_to_model(&SkillManager::handle(ctx), |_, _, event, ctx| {
            if is_zero_state_skill_event(event) {
                ctx.emit(UpdatedZeroState);
            }
        });

        Self {
            slash_command_data_source: slash_command_data_source.clone(),
        }
    }
}

impl Entity for GuiZeroStateDataSource {
    type Event = UpdatedZeroState;
}

impl SyncDataSource for GuiZeroStateDataSource {
    type Action = AcceptSlashCommandOrLocalPrompt;

    fn run_query(
        &self,
        query: &Query,
        app: &warpui::AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        if !query.text.is_empty() {
            return Ok(vec![]);
        }

        let source = self.slash_command_data_source.as_ref(app);
        let is_cloud_mode_v2 = source.is_cloud_mode_v2();
        let mut results = source.ordered_zero_state_commands(app);

        if should_show_local_skills(
            AISettings::as_ref(app).is_any_ai_enabled(app),
            FeatureFlag::ListSkills.is_enabled(),
        ) {
            let cli_agent_providers = source.active_cli_agent_providers(app);
            let active_session = source.active_session().as_ref(app);
            let cwd = active_session.current_working_directory_location(app);
            let skill_manager_handle = SkillManager::handle(app);
            let skill_manager = skill_manager_handle.as_ref(app);
            let skills = skill_manager.get_skills_for_working_directory(
                cwd.as_ref().and_then(|path| path.to_local_path()),
                app,
            );

            for mut skill in skills
                .into_iter()
                .sorted_by(|a, b| b.name.to_lowercase().cmp(&a.name.to_lowercase()))
            {
                if let Some(providers) = &cli_agent_providers {
                    if !skill_manager.skill_exists_for_any_provider(&skill, providers) {
                        continue;
                    }
                    skill.provider = skill_manager.best_supported_provider(&skill, providers);
                }
                results.push(InlineItem::from_skill(&skill, app));
            }
        }

        if should_show_local_prompts(AISettings::as_ref(app).is_any_ai_enabled(app)) {
            let local_prompts: Vec<_> = WarpConfig::as_ref(app)
                .local_user_workflows()
                .iter()
                .filter(|workflow| workflow.is_agent_mode_workflow())
                .sorted_by(|a, b| b.name().to_lowercase().cmp(&a.name().to_lowercase()))
                .collect();
            for prompt in local_prompts {
                results.push(InlineItem::from_local_prompt(prompt, app));
            }
        }

        Ok(results
            .into_iter()
            .map(|item| item.with_compact_layout(is_cloud_mode_v2).into())
            .collect())
    }
}
