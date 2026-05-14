//! Local agent-skill listing for the Warp CLI.

use std::{collections::BTreeMap, path::PathBuf};

use anyhow::{anyhow, Context};
use warp_cli::agent::ListAgentConfigsArgs;
use warpui::{platform::TerminationMode, AppContext, ModelContext, SingletonEntity};

use crate::ai::skills::{read_skills_from_directories, SkillDescriptor, SkillManager};
use crate::server::server_api::ai::{
    AgentListEnvironment, AgentListItem, AgentListSource, AgentListVariant,
};

const MAX_LINE_WIDTH: usize = 90;

/// Singleton model that runs CLI list commands with access to app-local state.
struct AgentConfigRunner;

/// List locally available agent skills.
pub fn list_agents(ctx: &mut AppContext, args: ListAgentConfigsArgs) -> anyhow::Result<()> {
    let runner = ctx.add_singleton_model(|_ctx| AgentConfigRunner);
    runner.update(ctx, |runner, ctx| runner.list(args.repo.clone(), ctx))
}

impl AgentConfigRunner {
    fn list(&self, repo: Option<String>, ctx: &mut ModelContext<Self>) -> anyhow::Result<()> {
        let agents = match repo {
            Some(repo_spec) => self.list_repo_agents(&repo_spec)?,
            None => self.list_visible_agents(ctx)?,
        };

        Self::print_agents_table(&agents);
        ctx.terminate_app(TerminationMode::ForceTerminate, None);
        Ok(())
    }

    fn list_visible_agents(
        &self,
        ctx: &mut ModelContext<Self>,
    ) -> anyhow::Result<Vec<AgentListItem>> {
        let cwd = std::env::current_dir().context("Unable to determine current directory")?;
        let skills = SkillManager::as_ref(ctx).get_skills_for_working_directory(Some(&cwd), ctx);
        Ok(Self::items_from_descriptors(skills))
    }

    fn list_repo_agents(&self, repo_spec: &str) -> anyhow::Result<Vec<AgentListItem>> {
        let repo_path = resolve_local_repo_path(repo_spec)?;
        let skill_dirs = ai::skills::SKILL_PROVIDER_DEFINITIONS
            .iter()
            .map(|definition| repo_path.join(&definition.skills_path));
        let skills = read_skills_from_directories(skill_dirs)
            .into_iter()
            .map(SkillDescriptor::from)
            .collect::<Vec<_>>();
        Ok(Self::items_from_descriptors(skills))
    }

    fn items_from_descriptors(skills: Vec<SkillDescriptor>) -> Vec<AgentListItem> {
        let mut grouped: BTreeMap<String, Vec<AgentListVariant>> = BTreeMap::new();
        for skill in skills {
            let source_name = format!("{:?}", skill.provider).to_ascii_lowercase();
            let id = skill.reference.to_string();
            let variant = AgentListVariant {
                id: id.clone(),
                description: skill.description,
                base_prompt: String::new(),
                source: AgentListSource {
                    owner: "local".to_string(),
                    name: source_name,
                    skill_path: id,
                },
                environments: Vec::<AgentListEnvironment>::new(),
            };
            grouped.entry(skill.name).or_default().push(variant);
        }

        grouped
            .into_iter()
            .map(|(name, variants)| AgentListItem { name, variants })
            .collect()
    }

    /// Print a list of agents in a card-style format.
    fn print_agents_table(agents: &[AgentListItem]) {
        if agents.is_empty() {
            println!("No agents found.");
            return;
        }

        if agents.len() == 1 {
            println!("\nAgent:");
        } else {
            println!("\nAgents ({}):", agents.len());
        }

        for agent in agents {
            println!("\n{}", agent.name);

            for variant in &agent.variants {
                let mut table = super::output::standard_table();

                table.add_row(vec![format!("ID: {}", variant.id)]);

                if !variant.description.is_empty() {
                    let description_cell = super::text_layout::render_labeled_wrapped_field(
                        "Description",
                        &variant.description,
                        MAX_LINE_WIDTH,
                    );
                    table.add_row(vec![description_cell]);
                }

                if !variant.base_prompt.is_empty() {
                    let mut chars = variant.base_prompt.chars();
                    let truncated: String = chars.by_ref().take(100).collect();
                    let truncated_prompt = if chars.next().is_some() {
                        format!("{truncated}...")
                    } else {
                        truncated
                    };
                    let prompt_cell = super::text_layout::render_labeled_wrapped_field(
                        "Base Prompt",
                        &truncated_prompt,
                        MAX_LINE_WIDTH,
                    );
                    table.add_row(vec![prompt_cell]);
                }

                table.add_row(vec![format!(
                    "Source: {}/{}",
                    variant.source.owner, variant.source.name
                )]);

                if !variant.environments.is_empty() {
                    let env_entries: Vec<_> = variant
                        .environments
                        .iter()
                        .map(|e| format!("{} ({})", e.name, e.uid))
                        .collect();
                    table.add_row(vec![format!("Environments: {}", env_entries.join(", "))]);
                }

                println!("{table}");
            }
        }
    }
}

fn resolve_local_repo_path(repo_spec: &str) -> anyhow::Result<PathBuf> {
    let direct_path = PathBuf::from(repo_spec);
    if direct_path.exists() {
        return direct_path
            .canonicalize()
            .with_context(|| format!("Unable to resolve {}", direct_path.display()));
    }

    let Some(repo_name) = repo_name_from_spec(repo_spec) else {
        return Err(anyhow!(
            "Repository lookup is local-only in this build; pass a local path or an owner/repo slug that already exists under the current directory."
        ));
    };

    let cwd = std::env::current_dir().context("Unable to determine current directory")?;
    let candidate = cwd.join(repo_name);
    if candidate.exists() {
        return candidate
            .canonicalize()
            .with_context(|| format!("Unable to resolve {}", candidate.display()));
    }

    Err(anyhow!(
        "Repository lookup is local-only in this build; '{}' was not found as a local path and '{}' does not exist under {}.",
        repo_spec,
        candidate.display(),
        cwd.display()
    ))
}

fn repo_name_from_spec(spec: &str) -> Option<&str> {
    let trimmed = spec.trim().trim_end_matches(".git").trim_end_matches('/');
    let parts = trimmed.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [owner, repo] if !owner.is_empty() && !repo.is_empty() => Some(*repo),
        [.., owner, repo] if !owner.is_empty() && !repo.is_empty() => Some(*repo),
        _ => None,
    }
}

impl warpui::Entity for AgentConfigRunner {
    type Event = ();
}

impl SingletonEntity for AgentConfigRunner {}
