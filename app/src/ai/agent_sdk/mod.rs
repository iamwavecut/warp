//! Agent SDK entry points for invoking Agent-related functionality from the app.
//! For now this provides a simple runner that echoes the received command.

use std::fmt::Write;
use std::path::Path;

use crate::ai::agent::api::direct_openai::CustomProviderRoute;
use crate::ai::agent_sdk::driver::harness::{
    HarnessKind, LocalHarnessRepository, LocalHarnessResumePayload, harness_kind,
    resume_cli_help_is_compatible,
};
use crate::ai::agent_sdk::driver::{AgentDriverOptions, AgentRunPrompt, Task};
use crate::ai::agent_sdk::mcp_config::build_mcp_servers_from_specs;
use crate::ai::custom_model_routers::{
    RouterRequestFacts, is_local_custom_router_id, resolve_router_selection,
};
use crate::ai::execution_profiles::profiles::AIExecutionProfilesModel;
use crate::ai::llms::{LLMId, LLMPreferences};
use crate::workflows::{
    command_parser::WorkflowCommandDisplayData, local_saved_prompts::LocalSavedPromptRepository,
};
use ai::api_keys::ApiKeyManager;
use anyhow::Context;
use command::r#async::Command as AsyncCommand;
use uuid::Uuid;
use warp_cli::skill::SkillSpec;
use warp_cli::{
    CliCommand, GlobalOptions,
    agent::{AgentCommand, OutputFormat},
};
use warp_core::features::FeatureFlag;
#[cfg(not(target_family = "wasm"))]
use warp_logging::log_file_path;
use warpui::{AppContext, platform::TerminationMode};
use warpui::{
    ModelSpawner, SingletonEntity,
    r#async::{FutureExt as _, TimeoutError},
};

use crate::{ai::ambient_agents::task::HarnessConfig, server::server_api::ai::AgentConfigSnapshot};
use driver::AgentDriverError;

use crate::ai::local_named_agents::{
    LocalNamedAgentRepository, LocalNamedAgentRunMetadata, LocalNamedAgentRunStatus,
    NamedAgentBundle, NamedAgentRunOverrides, merge_named_agent_config, profile_sync_id,
    validate_named_config_file, validate_named_mcp_servers, validate_named_run_args,
};
use crate::ai::mcp::TemplatableMCPServerManager;
use crate::ai::skills::{
    ResolveSkillError, ResolvedSkill, clone_repo_for_skill, resolve_skill_spec,
};
use crate::settings::AISettings;

pub use driver::AgentDriver;
pub(crate) use driver::harness::{task_env_vars, validate_cli_installed};
use warp_cli::agent::{Harness, Prompt, RunAgentArgs};

mod admin;
mod agent_config;
mod common;
pub(crate) mod config_file;
pub(crate) mod driver;
mod mcp;
pub(crate) mod mcp_config;
mod model;
pub mod output;
mod profiles;
mod schedule;
mod text_layout;

/// Run a Warp CLI command.
pub fn run(
    ctx: &mut AppContext,
    command: CliCommand,
    global_options: GlobalOptions,
) -> anyhow::Result<()> {
    launch_command(ctx, command, global_options)
}

/// Dispatch a CLI command to its handler.
fn dispatch_command(
    ctx: &mut AppContext,
    command: CliCommand,
    global_options: GlobalOptions,
) -> anyhow::Result<()> {
    match command {
        CliCommand::Agent(agent_cmd) => run_agent(ctx, global_options, agent_cmd),
        CliCommand::MCP(mcp_cmd) => mcp::run(ctx, global_options, mcp_cmd),
        CliCommand::Model(model_cmd) => model::run(ctx, global_options, model_cmd),
        CliCommand::Schedule(schedule_cmd) => {
            schedule::run(ctx, schedule_cmd, global_options.output_format)
        }
        CliCommand::Login => admin::login(ctx),
        CliCommand::Logout => admin::logout(ctx),
        CliCommand::Whoami => admin::whoami(ctx, global_options.output_format),
    }
}

fn format_skill_resolution_error(err: ResolveSkillError) -> String {
    match err {
        ResolveSkillError::NotFound { skill } => {
            format!("Skill '{skill}' not found")
        }
        ResolveSkillError::RepoNotFound { repo } => {
            format!("Repository '{repo}' not found")
        }
        ResolveSkillError::Ambiguous { skill, candidates } => {
            let mut msg = format!(
                "Skill '{skill}' is ambiguous; specify as repo:skill_name\n\nCandidates:\n"
            );
            for path in candidates {
                msg.push_str(&format!("- {}\n", path.display()));
            }
            msg
        }
        ResolveSkillError::OrgMismatch {
            repo,
            expected,
            found,
        } => {
            format!("Repository '{repo}' found but belongs to org '{found}', expected '{expected}'")
        }
        ResolveSkillError::ParseFailed { path, message } => {
            format!("Failed to parse skill file {}: {message}", path.display())
        }
        ResolveSkillError::CloneFailed { org, repo, message } => {
            format!("Failed to clone repository '{org}/{repo}': {message}")
        }
    }
}

/// Run the agent with the provided command.
fn run_agent(
    ctx: &mut AppContext,
    global_options: GlobalOptions,
    command: AgentCommand,
) -> anyhow::Result<()> {
    match command {
        AgentCommand::Run(args) => {
            if args.skill.is_some() && !FeatureFlag::OzPlatformSkills.is_enabled() {
                return Err(anyhow::anyhow!("unexpected argument '--skill' found"));
            }
            let harness = args.harness.unwrap_or_default();
            if harness != Harness::Oz && !FeatureFlag::AgentHarness.is_enabled() {
                return Err(anyhow::anyhow!("unexpected argument '--harness' found"));
            }
            if harness == Harness::OpenCode {
                return Err(anyhow::anyhow!(
                    "The opencode harness is only supported for local child agent launches."
                ));
            }

            // Start the agent driver runner, which will handle the rest of the setup steps
            // (managing both sync and async steps) as well as triggering the driver.
            let runner = ctx.add_singleton_model(|_| AgentDriverRunner);
            runner.update(ctx, move |_, ctx| {
                let spawner = ctx.spawner();
                ctx.spawn(
                    AgentDriverRunner::setup_and_run_driver(
                        spawner,
                        args,
                        global_options.output_format,
                    ),
                    |_, result, _ctx| {
                        if let Err(e) = result {
                            report_fatal_error(e.into(), _ctx);
                        }
                    },
                );
            });

            Ok(())
        }
        command @ (AgentCommand::Create(_)
        | AgentCommand::Show(_)
        | AgentCommand::Update(_)
        | AgentCommand::Delete(_)) => {
            crate::ai::local_named_agents::run_named_agent_crud(ctx, command)
        }
        AgentCommand::Profile(sub) => profiles::run(ctx, global_options, sub),
        AgentCommand::List(args) => agent_config::list_agents(ctx, args),
    }
}

/// Build the merged agent configuration from all sources and the Task for the driver.
/// Merge precedence: bundle < one-shot file < CLI/UI < invoked skill.
fn build_merged_config_and_task(
    args: &RunAgentArgs,
    resolved_bundle_skills: &[ResolvedSkill],
    resolved_invoked_skill: &Option<ResolvedSkill>,
    prompt: &Option<Prompt>,
    named_bundle: Option<&NamedAgentBundle>,
    ctx: &mut AppContext,
) -> anyhow::Result<(AgentConfigSnapshot, Task)> {
    let loaded_file = match args.config_file.file.as_deref() {
        Some(path) => Some(config_file::load_config_file(path)?),
        None => None,
    };

    let cli_mcp_servers = build_mcp_servers_from_specs(&args.all_mcp_specs())?;

    let skill_name = resolved_invoked_skill
        .as_ref()
        .or_else(|| resolved_bundle_skills.first())
        .map(|skill| skill.name.clone());
    let bundle_skill_instructions = (!resolved_bundle_skills.is_empty()).then(|| {
        resolved_bundle_skills
            .iter()
            .map(|skill| skill.instructions.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    });
    let invoked_skill_instructions = resolved_invoked_skill
        .as_ref()
        .map(|skill| skill.instructions.clone());

    let harness_override = args.harness.map(HarnessConfig::from_harness_type);

    let cli_config = AgentConfigSnapshot {
        name: args.name.clone(),
        environment_id: None,
        model_id: args.model.model.clone(),
        base_prompt: None,
        mcp_servers: cli_mcp_servers,
        profile_id: args.profile.clone(),
        worker_host: None,
        skill_spec: args.skill.as_ref().map(ToString::to_string),
        computer_use_enabled: args.computer_use.computer_use_override(),
        harness: harness_override,
        harness_auth_secrets: None,
    };

    let mut merged_config = if let Some(bundle) = named_bundle {
        let overrides = NamedAgentRunOverrides {
            one_shot: loaded_file.as_ref().map(|file| file.file.clone()),
            cli: cli_config,
            bundle_skill_instructions,
            invoked_skill_instructions,
        };
        merge_named_agent_config(bundle, &overrides)?
    } else {
        let file_merged = config_file::merge_with_precedence(loaded_file.as_ref(), cli_config);
        let mut merged = file_merged;
        if let Some(instructions) = &invoked_skill_instructions {
            merged.base_prompt = Some(instructions.clone());
        }
        if let Some(name) = args.name.clone().or(skill_name) {
            merged.name = Some(name);
        }
        merged
    };

    let runtime_mcp_specs = match merged_config.mcp_servers.as_ref() {
        Some(mcp_servers) => config_file::mcp_specs_from_mcp_servers(mcp_servers)?,
        None => Vec::new(),
    };

    let model_override: Option<LLMId> = merged_config
        .model_id
        .as_deref()
        .map(|model_id| common::validate_agent_mode_base_model_id(model_id, ctx))
        .transpose()?;

    if let Some(profile) = merged_config.profile_id.as_deref() {
        let sync_id = profile_sync_id(profile)
            .map_err(|_| anyhow::anyhow!(AgentDriverError::ProfileError(profile.to_owned())))?;
        if AIExecutionProfilesModel::as_ref(ctx)
            .get_profile_id_by_sync_id(&sync_id)
            .is_none()
        {
            return Err(anyhow::anyhow!(AgentDriverError::ProfileError(
                profile.to_owned()
            )));
        }
    }

    // Keep the task config snapshot aligned with the effective model selection.
    merged_config.model_id = model_override.clone().map(|id| id.to_string());

    // Combine base_prompt with user prompt locally.
    let local_prompt = match (merged_config.base_prompt.as_deref(), prompt) {
        (Some(base_prompt), Some(Prompt::PlainText(user_prompt))) => {
            Prompt::PlainText(format!("{base_prompt}\n\n{user_prompt}"))
        }
        (Some(base_prompt), None) => {
            // Skill-only invocation: use skill instructions as the prompt
            Prompt::PlainText(base_prompt.to_string())
        }
        (_, Some(p)) => p.clone(),
        (None, None) => {
            return Err(anyhow::anyhow!(AgentDriverError::InvalidRuntimeState));
        }
    };

    let task = Task {
        prompt: AgentRunPrompt::Local(resolve_prompt(&local_prompt, ctx)?),
        model: model_override,
        profile: merged_config.profile_id.clone(),
        mcp_specs: runtime_mcp_specs,
        harness: harness_kind(
            merged_config
                .harness
                .as_ref()
                .map(|config| config.harness_type)
                .unwrap_or(args.harness.unwrap_or_default()),
        )?,
        local_only: named_bundle.is_some(),
    };

    Ok((merged_config, task))
}

/// Validate every local named-agent dependency before `AgentDriver::new` can
/// create a terminal, start an MCP process, invoke a harness, or issue HTTP.
/// This intentionally resolves provider credentials and profile identity only;
/// it never sends a request or starts a process. The resolved route is carried
/// into the driver so the later local execution path cannot silently fall back
/// to a hosted model or re-resolve after terminal startup.
fn preflight_named_execution(
    config: &AgentConfigSnapshot,
    task: &Task,
    ctx: &AppContext,
) -> anyhow::Result<CustomProviderRoute> {
    let model_id = config
        .model_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("named agent requires a concrete local model"))?;
    let selected_model = common::validate_agent_mode_base_model_id(model_id, ctx)?;
    let providers = &AISettings::as_ref(ctx).custom_providers;
    let concrete_model = if is_local_custom_router_id(selected_model.as_str()) {
        let router = LLMPreferences::as_ref(ctx)
            .custom_model_router_for_id(&selected_model)
            .ok_or_else(|| anyhow::anyhow!("local model router is not loaded"))?;
        let (_, target) =
            resolve_router_selection(router, &RouterRequestFacts::baseline(), providers)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        format!("custom/{}/{}", target.provider_name, target.model_id)
    } else {
        selected_model.to_string()
    };
    let route = crate::ai::agent::api::direct_openai::resolve_custom_provider_route_with_readiness(
        &concrete_model,
        providers,
        ApiKeyManager::as_ref(ctx).keys(),
        ApiKeyManager::as_ref(ctx).keys_ready(),
    )?
    .ok_or_else(|| anyhow::anyhow!("local custom provider route is not configured"))?;
    if !route.effective_capabilities().chat {
        anyhow::bail!("local custom provider does not support chat");
    }

    if let Some(profile) = config.profile_id.as_deref() {
        let sync_id =
            profile_sync_id(profile).map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if AIExecutionProfilesModel::as_ref(ctx)
            .get_profile_id_by_sync_id(&sync_id)
            .is_none()
        {
            return Err(anyhow::anyhow!(AgentDriverError::ProfileError(
                profile.to_owned()
            )));
        }
    }

    let mcp_manager = TemplatableMCPServerManager::as_ref(ctx);
    for spec in &task.mcp_specs {
        if let warp_cli::mcp::MCPSpec::Uuid(uuid) = spec
            && mcp_manager.get_installed_server(uuid).is_none()
        {
            return Err(anyhow::anyhow!(AgentDriverError::MCPServerNotFound(*uuid)));
        }
    }

    if config.computer_use_enabled == Some(true)
        && !(FeatureFlag::AgentModeComputerUse.is_enabled()
            && FeatureFlag::LocalComputerUse.is_enabled()
            && computer_use::is_supported_on_current_platform())
    {
        anyhow::bail!("computer use is unavailable on this local machine");
    }

    match &task.harness {
        HarnessKind::ThirdParty(harness) => {
            harness.validate().map_err(|error| anyhow::anyhow!(error))?
        }
        HarnessKind::Unsupported(harness) => {
            anyhow::bail!("the {harness} harness is not available for a local named agent")
        }
        HarnessKind::Oz => {}
    }

    Ok(route)
}

/// Resolve a `Prompt` to a plain string.
fn resolve_prompt(prompt: &Prompt, _ctx: &AppContext) -> Result<String, AgentDriverError> {
    match prompt {
        Prompt::PlainText(prompt_str) => Ok(prompt_str.to_string()),
    }
}

/// Resolve a local saved prompt without consulting CloudModel or any Warp
/// service. Argument placeholders use the same workflow display machinery as
/// the terminal workflow picker, including stored default values.
fn resolve_saved_prompt(selector: &str) -> Result<Prompt, AgentDriverError> {
    let saved_prompt = LocalSavedPromptRepository::for_user()
        .resolve(selector)
        .map_err(|error| AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(error)))?;
    let query =
        WorkflowCommandDisplayData::new_from_workflow(saved_prompt.workflow()).to_command_string();
    Ok(Prompt::PlainText(query))
}

/// Singleton model that provides a ModelContext for spawning async operations
/// when starting the agent driver. This is needed because conversation fetching
/// requires spawning an async task, which requires a ModelContext.
struct AgentDriverRunner;

const RESUME_CAPABILITY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

async fn run_resume_capability_probe(
    cli_name: &str,
    argument: &str,
) -> Result<String, AgentDriverError> {
    let mut command = AsyncCommand::new(cli_name);
    command.arg(argument).kill_on_drop(true);
    let output = command
        .output()
        .with_timeout(RESUME_CAPABILITY_TIMEOUT)
        .await
        .map_err(|_: TimeoutError| AgentDriverError::HarnessSetupFailed {
            harness: cli_name.to_owned(),
            reason: format!("local `{cli_name} {argument}` capability probe timed out"),
        })?
        .map_err(|error| AgentDriverError::HarnessSetupFailed {
            harness: cli_name.to_owned(),
            reason: format!(
                "failed to run local `{cli_name} {argument}` capability probe: {error}"
            ),
        })?;
    let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
    if !output.stderr.is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&String::from_utf8_lossy(&output.stderr));
    }
    Ok(text)
}

impl warpui::Entity for AgentDriverRunner {
    type Event = ();
}

impl warpui::SingletonEntity for AgentDriverRunner {}

impl AgentDriverRunner {
    async fn setup_and_run_driver(
        foreground: ModelSpawner<Self>,
        args: RunAgentArgs,
        output_format: OutputFormat,
    ) -> Result<(), AgentDriverError> {
        let (driver_options, task, named_run_id) =
            Self::build_driver_options_and_task(&foreground, args).await?;
        let result: Result<(), AgentDriverError> = async {

            match &task.harness {
                HarnessKind::Unsupported(harness) => {
                    return Err(AgentDriverError::HarnessSetupFailed {
                        harness: harness.to_string(),
                        reason: format!(
                            "The {harness} harness is only supported for local child agent launches."
                        ),
                    });
                }
                HarnessKind::Oz | HarnessKind::ThirdParty(_) => {}
            }

            // Validate that the third-party harness is installed and authed.
            if let HarnessKind::ThirdParty(harness) = &task.harness {
                harness.validate()?;
                if driver_options
                    .local_resume
                    .as_ref()
                    .is_some_and(|resume| resume.is_resume)
                {
                    Self::validate_resume_cli_compatibility(harness.as_ref()).await?;
                }
            }

            // Run the driver
            foreground
                .spawn(move |_, ctx| {
                    Self::create_and_run_driver(
                        ctx,
                        driver_options,
                        output_format,
                        task,
                        named_run_id,
                    );
                })
                .await?;

            Ok(())
        }
        .await;

        if result.is_err()
            && let Some(run_id) = named_run_id
        {
            Self::record_named_run_status(run_id, LocalNamedAgentRunStatus::Failed);
        }
        result
    }

    /// Probe only the installed CLI's help/version surface before a resumed
    /// harness can receive the prompt or start any tool process.
    async fn validate_resume_cli_compatibility(
        harness: &dyn crate::ai::agent_sdk::driver::harness::ThirdPartyHarness,
    ) -> Result<(), AgentDriverError> {
        let cli_name = harness.cli_agent().command_prefix().to_owned();
        let help_output = run_resume_capability_probe(&cli_name, "--help").await?;
        if resume_cli_help_is_compatible(harness.harness(), &help_output).is_ok() {
            return Ok(());
        }

        let version_output = run_resume_capability_probe(&cli_name, "--version")
            .await
            .unwrap_or_else(|error| error.to_string());
        let reason = resume_cli_help_is_compatible(harness.harness(), &help_output)
            .expect_err("resume capability probe should reject unsupported help output");
        Err(AgentDriverError::HarnessSetupFailed {
            harness: cli_name,
            reason: format!(
                "cannot resume this local run: {reason}. Installed CLI version probe: {}",
                version_output.trim()
            ),
        })
    }

    /// Resolve the skill spec from args, if one was provided.
    ///
    /// In sandboxed mode with a fully-qualified spec (org + repo), the repo is
    /// cloned first since it may not exist locally. Otherwise we resolve directly
    /// against the local filesystem.
    async fn resolve_skill(
        foreground: &ModelSpawner<Self>,
        args: &RunAgentArgs,
        working_dir: &Path,
    ) -> Result<Option<ResolvedSkill>, AgentDriverError> {
        if !FeatureFlag::OzPlatformSkills.is_enabled() {
            return Ok(None);
        }
        let Some(skill_spec) = args.skill.clone() else {
            return Ok(None);
        };

        // In sandboxed mode with a fully-qualified spec, clone the repo first.
        let needs_clone = args.sandboxed && skill_spec.org.is_some() && skill_spec.repo.is_some();
        if needs_clone {
            let org = skill_spec.org.as_ref().expect("org checked above");
            let repo_name = skill_spec.repo.as_ref().expect("repo checked above");
            log::info!("Cloning {org}/{repo_name} for skill resolution in sandboxed mode");
            clone_repo_for_skill(org, repo_name, working_dir)
                .await
                .map_err(|err| {
                    AgentDriverError::SkillResolutionFailed(format_skill_resolution_error(err))
                })?;
        }

        let working_dir_buf = working_dir.to_path_buf();
        let skill = foreground
            .spawn(move |_, ctx| resolve_skill_spec(&skill_spec, &working_dir_buf, ctx))
            .await?
            .map_err(|err| {
                AgentDriverError::SkillResolutionFailed(format_skill_resolution_error(err))
            })?;
        log::debug!(
            "Resolved skill '{}' from {}",
            skill.name,
            skill.skill_path.display()
        );
        Ok(Some(skill))
    }

    /// Resolve every skill persisted in a named bundle before starting the
    /// driver. Bundle references are local-only: unlike an explicitly
    /// sandboxed CLI skill they never clone a repository or perform network
    /// I/O.
    async fn resolve_named_skills(
        foreground: &ModelSpawner<Self>,
        specs: &[SkillSpec],
        working_dir: &Path,
    ) -> Result<Vec<ResolvedSkill>, AgentDriverError> {
        if specs.is_empty() {
            return Ok(Vec::new());
        }
        if !FeatureFlag::OzPlatformSkills.is_enabled() {
            return Err(AgentDriverError::SkillResolutionFailed(
                "local named agent skills are disabled".to_owned(),
            ));
        }

        let mut resolved = Vec::with_capacity(specs.len());
        for spec in specs {
            let skill_spec = spec.clone();
            let resolve_working_dir = working_dir.to_path_buf();
            let skill = foreground
                .spawn(move |_, ctx| resolve_skill_spec(&skill_spec, &resolve_working_dir, ctx))
                .await?
                .map_err(|error| {
                    AgentDriverError::SkillResolutionFailed(format_skill_resolution_error(error))
                })?;
            ensure_named_skill_containment(&skill, working_dir)?;
            log::debug!(
                "Resolved named-agent skill '{}' from {}",
                skill.name,
                skill.skill_path.display()
            );
            resolved.push(skill);
        }
        Ok(resolved)
    }

    /// Build the AgentDriverOptions and Task for a local CLI agent run.
    async fn build_driver_options_and_task(
        foreground: &ModelSpawner<Self>,
        mut args: RunAgentArgs,
    ) -> Result<(AgentDriverOptions, Task, Option<Uuid>), AgentDriverError> {
        let resume_record = args
            .resume
            .as_deref()
            .map(|selector| {
                let run_id = Uuid::parse_str(selector).map_err(|error| {
                    AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                        "invalid local harness run ID '{selector}': {error}"
                    ))
                })?;
                LocalHarnessRepository::for_user()
                    .read(run_id)
                    .map_err(|error| AgentDriverError::ConfigBuildFailed(anyhow::Error::new(error)))
            })
            .transpose()?;

        if let Some(record) = resume_record.as_ref() {
            if let Some(requested_harness) = args.harness
                && requested_harness != record.harness
            {
                return Err(AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                    "resume harness conflict: local run {} uses {}, but {} was requested",
                    record.run_id,
                    record.harness,
                    requested_harness
                )));
            }
            // An omitted `--harness` adopts the stored harness. An explicit
            // value, including `--harness oz`, was checked above and must not
            // silently override the local session contract.
            args.harness.get_or_insert(record.harness);
        }

        let stored_working_dir = resume_record
            .as_ref()
            .map(|record| {
                LocalHarnessRepository::for_user().canonical_working_dir(&record.working_dir)
            })
            .transpose()
            .map_err(|error| AgentDriverError::ConfigBuildFailed(anyhow::Error::new(error)))?;

        // Get the working directory. Claude sessions are project-bound: an
        // override would select a different encoded transcript directory, so
        // reject it unless it resolves to the original canonical project.
        if let (Some(explicit_cwd), Some(record), Some(stored_cwd)) = (
            args.cwd.as_ref(),
            resume_record.as_ref(),
            stored_working_dir.as_ref(),
        ) && record.harness == Harness::Claude
        {
            let explicit_cwd = dunce::canonicalize(explicit_cwd).map_err(|error| {
                AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                    "Unable to resolve {}: {error}",
                    explicit_cwd.display()
                ))
            })?;
            if explicit_cwd != *stored_cwd {
                return Err(AgentDriverError::ConfigBuildFailed(anyhow::anyhow!(
                    "Claude local resume is bound to {}; --cwd {} selects a different project",
                    stored_cwd.display(),
                    explicit_cwd.display()
                )));
            }
        }

        let working_dir = match (args.cwd.as_ref(), resume_record.as_ref()) {
            (Some(dir), _) => dunce::canonicalize(dir)
                .with_context(|| format!("Unable to resolve {}", dir.display())),
            (None, Some(_record)) => Ok(stored_working_dir
                .clone()
                .expect("stored working directory is present for a resume")),
            (None, None) => std::env::current_dir()
                .context("Unable to determine working directory")
                .and_then(|dir| {
                    dunce::canonicalize(&dir)
                        .with_context(|| format!("Unable to resolve {}", dir.display()))
                }),
        }
        .map_err(AgentDriverError::ConfigBuildFailed)?;

        if let Some(record) = resume_record.as_ref() {
            let mut validation_record = record.clone();
            validation_record.working_dir = working_dir.clone();
            LocalHarnessRepository::for_user()
                .validate_transcript(&validation_record)
                .map_err(|error| AgentDriverError::ConfigBuildFailed(anyhow::Error::new(error)))?;
        }

        let resume_record_for_driver = resume_record.clone().map(|mut record| {
            // A resume may deliberately override the original cwd. Keep the
            // validated locator and session binding, but hand the effective
            // cwd to the harness so its local config/MCP resolution follows
            // the current invocation.
            record.working_dir = working_dir.clone();
            record
        });

        let named_record = args
            .agent
            .as_deref()
            .map(|selector| LocalNamedAgentRepository::for_user().resolve(selector))
            .transpose()
            .map_err(|error| AgentDriverError::ConfigBuildFailed(anyhow::Error::new(error)))?;
        if named_record.is_some() {
            LocalNamedAgentRepository::for_user()
                .repair_stale_runs()
                .map_err(|error| AgentDriverError::ConfigBuildFailed(anyhow::Error::new(error)))?;
        }
        let named_bundle = named_record.as_ref().map(|record| record.bundle().clone());
        let named_record_for_metadata = named_record.clone();

        if named_bundle.is_some() {
            validate_named_run_args(&args)
                .map_err(|error| AgentDriverError::ConfigBuildFailed(anyhow::Error::new(error)))?;
            if let Some(path) = args.config_file.file.as_deref() {
                let loaded = config_file::load_config_file(path)
                    .map_err(AgentDriverError::ConfigBuildFailed)?;
                validate_named_config_file(&loaded.file).map_err(|error| {
                    AgentDriverError::ConfigBuildFailed(anyhow::Error::new(error))
                })?;
            }
            let cli_mcp_servers = build_mcp_servers_from_specs(&args.all_mcp_specs())
                .map_err(AgentDriverError::ConfigBuildFailed)?;
            if let Some(mcp_servers) = cli_mcp_servers.as_ref() {
                validate_named_mcp_servers(mcp_servers).map_err(|error| {
                    AgentDriverError::ConfigBuildFailed(anyhow::Error::new(error))
                })?;
            }
        }

        let bundle_skill_specs = named_bundle
            .as_ref()
            .map(|bundle| {
                bundle
                    .skills
                    .iter()
                    .map(|skill| {
                        skill
                            .parse()
                            .map_err(|error: String| AgentDriverError::SkillResolutionFailed(error))
                    })
                    .collect::<Result<Vec<SkillSpec>, AgentDriverError>>()
            })
            .transpose()?
            .unwrap_or_default();
        let resolved_bundle_skills =
            Self::resolve_named_skills(foreground, &bundle_skill_specs, &working_dir).await?;

        // Resolve an explicitly invoked skill after the bundle skills. Its
        // instructions are applied last by the merge function.
        let resolved_invoked_skill = Self::resolve_skill(foreground, &args, &working_dir).await?;
        if named_bundle.is_some()
            && let Some(skill) = resolved_invoked_skill.as_ref()
        {
            ensure_named_skill_containment(skill, &working_dir)?;
        }

        // Extract variables we want to use later before moving args into the closure
        let prompt = match args.saved_prompt.as_deref() {
            Some(selector) => Some(resolve_saved_prompt(selector)?),
            None => args.prompt_arg.to_prompt(),
        };

        // Build the AgentConfigSnapshot, Task, and AgentDriverOptions
        let prompt_clone = prompt.clone();
        let (task, driver_options, named_run_id) = foreground
            .spawn(move |_, ctx| -> anyhow::Result<_> {
                let (merged_config, task) = build_merged_config_and_task(
                    &args,
                    &resolved_bundle_skills,
                    &resolved_invoked_skill,
                    &prompt_clone,
                    named_bundle.as_ref(),
                    ctx,
                )?;
                if merged_config.environment_id.is_some() {
                    return Err(anyhow::anyhow!(
                        "cloud environments are disabled in this local-first build"
                    ));
                }

                let selected_harness = merged_config
                    .harness
                    .as_ref()
                    .map(|config| config.harness_type)
                    .unwrap_or(args.harness.unwrap_or_default());

                if let Some(record) = resume_record_for_driver.as_ref()
                    && selected_harness != record.harness
                {
                    return Err(anyhow::anyhow!(
                        "resume harness conflict: local run {} uses {}, but {} was resolved",
                        record.run_id,
                        record.harness,
                        selected_harness
                    ));
                }

                let local_resume = if let Some(record) = resume_record_for_driver.as_ref() {
                    Some(LocalHarnessResumePayload::from_record(record))
                } else if matches!(selected_harness, Harness::Codex | Harness::Claude) {
                    Some(LocalHarnessResumePayload::fresh(
                        Uuid::new_v4(),
                        selected_harness,
                        &working_dir,
                        None,
                        merged_config
                            .harness
                            .as_ref()
                            .and_then(|config| config.model_config()),
                    ))
                } else {
                    None
                };

                let direct_provider_route = named_bundle
                    .is_some()
                    .then(|| preflight_named_execution(&merged_config, &task, ctx))
                    .transpose()?;

                let third_party_harness_model_config = merged_config
                    .harness
                    .as_ref()
                    .and_then(|config| config.model_config());

                let driver_options = driver::AgentDriverOptions {
                    working_dir: working_dir.clone(),
                    task_id: None,
                    parent_run_id: None,
                    idle_on_complete: args.idle_on_complete.map(|d| d.into()),
                    secrets: Default::default(),
                    environment: None,
                    selected_harness,
                    third_party_harness_model_config,
                    local_only: named_bundle.is_some(),
                    direct_provider_route,
                    local_resume,
                };

                let named_run_id = if let Some(record) = named_record_for_metadata.as_ref() {
                    let run_id = Uuid::new_v4();
                    let metadata = LocalNamedAgentRunMetadata::from_record(
                        run_id,
                        record,
                        &merged_config,
                        &working_dir,
                    )?;
                    LocalNamedAgentRepository::for_user()
                        .write_run_metadata(&metadata)
                        .map_err(|error| anyhow::Error::new(error))?;
                    Some(run_id)
                } else {
                    None
                };

                Ok((task, driver_options, named_run_id))
            })
            .await?
            .map_err(AgentDriverError::ConfigBuildFailed)?;

        Ok((driver_options, task, named_run_id))
    }

    /// Create the AgentDriver and start running the task.
    fn create_and_run_driver(
        ctx: &mut AppContext,
        driver_options: driver::AgentDriverOptions,
        output_format: OutputFormat,
        task: driver::Task,
        named_run_id: Option<Uuid>,
    ) {
        // Initializing the driver will fail if not logged in. Since we check that above, panic here - it's difficult to
        // fallibly instantiate a UI framework model.
        let driver = ctx.add_singleton_model(|ctx| {
            AgentDriver::new(driver_options, ctx).expect("Could not initialize driver")
        });

        driver.update(ctx, |driver, ctx| {
            driver.set_output_format(output_format);
            let agent_future = driver.run(task, ctx);

            ctx.spawn(agent_future, move |_, result, ctx| {
                if let Some(run_id) = named_run_id {
                    let status = match &result {
                        Ok(()) => LocalNamedAgentRunStatus::Succeeded,
                        Err(AgentDriverError::ConversationCancelled { .. }) => {
                            LocalNamedAgentRunStatus::Cancelled
                        }
                        Err(_) => LocalNamedAgentRunStatus::Failed,
                    };
                    Self::record_named_run_status(run_id, status);
                }

                match result {
                    Ok(()) | Err(AgentDriverError::ConversationCancelled { .. }) => {
                        ctx.terminate_app(TerminationMode::ForceTerminate, None);
                    }
                    Err(err) => report_fatal_error(err.into(), ctx),
                }
            });
        });
    }

    fn record_named_run_status(run_id: Uuid, status: LocalNamedAgentRunStatus) {
        if let Err(error) = LocalNamedAgentRepository::for_user().mark_run_status(run_id, status) {
            log::warn!("Failed to update local named-agent run {run_id}: {error}");
        }
    }
}

fn ensure_named_skill_containment(
    skill: &ResolvedSkill,
    working_dir: &Path,
) -> Result<(), AgentDriverError> {
    let root = dunce::canonicalize(working_dir).map_err(|error| {
        AgentDriverError::SkillResolutionFailed(format!(
            "unable to canonicalize local skill working root: {error}"
        ))
    })?;
    let path = dunce::canonicalize(&skill.skill_path).map_err(|_| {
        AgentDriverError::SkillResolutionFailed(
            "local named-agent skill path is unavailable".to_owned(),
        )
    })?;
    if !path.starts_with(&root) {
        return Err(AgentDriverError::SkillResolutionFailed(
            "local named-agent skill must remain inside the working directory".to_owned(),
        ));
    }
    Ok(())
}

/// Launch a CLI command through local state only.
fn launch_command(
    ctx: &mut AppContext,
    command: CliCommand,
    global_options: GlobalOptions,
) -> anyhow::Result<()> {
    dispatch_command(ctx, command, global_options)
}

/// Report a fatal error and terminate the app.
fn report_fatal_error(err: anyhow::Error, ctx: &mut AppContext) {
    let mut message = err.to_string();
    for cause in err.chain().skip(1) {
        let _ = write!(&mut message, "\n=> {cause}");
    }

    #[cfg(not(target_family = "wasm"))]
    {
        if let Ok(path) = log_file_path() {
            let _ = write!(
                message,
                "\n\nFor more information, check Warp logs at {}",
                path.display()
            );
        }
    }

    let error = anyhow::anyhow!(message);
    ctx.terminate_app(TerminationMode::ForceTerminate, Some(Err(error)));
}
