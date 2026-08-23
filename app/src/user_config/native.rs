use std::io::Write;
use std::path::{Path, PathBuf};
use std::{fs, io};

use anyhow::{Result, anyhow};
use itertools::Itertools;
use repo_metadata::RepositoryUpdate;
use warp_errors::report_error;
use warpui::{ModelContext, ModelHandle, SingletonEntity};

use super::util::{
    for_each_dir_entry, has_name, is_config_file, parse_multi_launch_config_dir_entry,
    parse_multi_workflow_dir_entry, parse_single_theme_dir_entry, parse_tab_config_dir_entry,
};
use super::{
    LAUNCH_CONFIG_COMMENT, WarpConfigUpdateEvent, custom_model_routers_dir, launch_configs_dir,
    tab_configs_dir, themes_dir, workflows_dir,
};
use crate::ai::custom_model_routers::{
    CustomModelRouter, LocalCustomModelRouterRepository, ModelConfigError, RouterFileRevision,
    parse_model_config_yaml,
};
use crate::features::FeatureFlag;
use crate::launch_configs::launch_config::LaunchConfig;
use crate::tab_configs::{TabConfig, TabConfigError};
use crate::themes::theme::WarpThemeConfig;
use crate::warp_managed_paths_watcher::{
    WarpManagedPathsWatcher, WarpManagedPathsWatcherEvent, repository_update_touches_path,
    repository_update_touches_prefix,
};
use crate::workflows::local_saved_prompts::{LocalSavedPromptRepository, is_atomic_temp_path};
use crate::workflows::workflow::Workflow;

impl super::WarpConfig {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        // Load launch configs, and workflows from disk asynchronously on a background
        // thread.
        //
        // Themes are required during initialization by `Settings`, so we load this synchronously
        // on startup. We should investigate the possibility of offloading theme loading to a
        // background thread in the future.
        let _ = ctx.spawn(
            async move { load_launch_configs(&launch_configs_dir()) },
            |me, launch_configs, ctx| {
                me.launch_configs = launch_configs;
                ctx.emit(WarpConfigUpdateEvent::LaunchConfigs);
            },
        );
        if FeatureFlag::TabConfigs.is_enabled() {
            let _ = ctx.spawn(
                async move { load_tab_configs(&tab_configs_dir()) },
                |me, (tab_configs, tab_config_errors), ctx| {
                    me.tab_configs = tab_configs;
                    me.tab_config_errors = tab_config_errors;
                    ctx.emit(WarpConfigUpdateEvent::TabConfigs);
                    // Don't emit TabConfigErrors on startup — the error toast
                    // should only appear when the user saves a config file,
                    // not on app restart.
                },
            );
        }
        let _ = ctx.spawn(
            async move {
                let user_workflows = load_workflows(&workflows_dir());
                let saved_prompts = LocalSavedPromptRepository::for_user().list();
                (user_workflows, saved_prompts)
            },
            |me, (user_workflows, saved_prompts), ctx| {
                me.local_user_workflows = user_workflows;
                match saved_prompts {
                    Ok(saved_prompts) => me.local_saved_prompts = saved_prompts,
                    Err(error) => report_error!(
                        anyhow::Error::new(error).context("Failed to load local saved prompts")
                    ),
                }
                ctx.emit(WarpConfigUpdateEvent::LocalUserWorkflows);
            },
        );
        let model_config_dir = custom_model_routers_dir();
        let _ = ctx.spawn(
            async move { load_model_configs(&model_config_dir) },
            |me, (routers, errors), ctx| {
                me.custom_model_routers = routers;
                me.custom_model_router_errors = errors;
                ctx.emit(WarpConfigUpdateEvent::ModelConfigs);
            },
        );
        ctx.subscribe_to_model(
            &WarpManagedPathsWatcher::handle(ctx),
            Self::handle_warp_managed_paths_event,
        );

        Self {
            theme_config: load_theme_configs(&themes_dir()),
            ..Default::default()
        }
    }

    fn handle_warp_managed_paths_event(
        &mut self,
        _: ModelHandle<WarpManagedPathsWatcher>,
        event: &WarpManagedPathsWatcherEvent,
        ctx: &mut ModelContext<Self>,
    ) {
        let WarpManagedPathsWatcherEvent::FilesChanged(update) = event;

        if update_touches_dir(update, &themes_dir()) {
            let theme_dir = themes_dir();
            let _ = ctx.spawn(
                async move { load_theme_configs(&theme_dir) },
                |me, theme_config, ctx| {
                    me.theme_config = theme_config;
                    ctx.emit(WarpConfigUpdateEvent::Themes);
                },
            );
        }

        if update_touches_workflows_dir(update, &workflows_dir()) {
            let workflow_dir = workflows_dir();
            let _ = ctx.spawn(
                async move {
                    let workflows = load_workflows(&workflow_dir);
                    let saved_prompts = LocalSavedPromptRepository::for_user().list();
                    (workflows, saved_prompts)
                },
                |me, (workflows, saved_prompts), ctx| {
                    me.local_user_workflows = workflows;
                    match saved_prompts {
                        Ok(saved_prompts) => me.local_saved_prompts = saved_prompts,
                        Err(error) => report_error!(
                            anyhow::Error::new(error)
                                .context("Failed to refresh local saved prompts")
                        ),
                    }
                    ctx.emit(WarpConfigUpdateEvent::LocalUserWorkflows);
                },
            );
        }

        if update_touches_dir(update, &launch_configs_dir()) {
            let launch_config_dir = launch_configs_dir();
            let _ = ctx.spawn(
                async move { load_launch_configs(&launch_config_dir) },
                |me, launch_configs, ctx| {
                    me.launch_configs = launch_configs;
                    ctx.emit(WarpConfigUpdateEvent::LaunchConfigs);
                },
            );
        }

        if FeatureFlag::TabConfigs.is_enabled() && update_touches_dir(update, &tab_configs_dir()) {
            let tab_config_dir = tab_configs_dir();
            let _ = ctx.spawn(
                async move { load_tab_configs(&tab_config_dir) },
                |me, (configs, errors), ctx| {
                    me.tab_configs = configs;
                    me.tab_config_errors = errors.clone();
                    ctx.emit(WarpConfigUpdateEvent::TabConfigs);
                    if !errors.is_empty() {
                        ctx.emit(WarpConfigUpdateEvent::TabConfigErrors(errors));
                    }
                },
            );
        }

        if FeatureFlag::SettingsFile.is_enabled()
            && update_touches_path(update, &crate::settings::user_preferences_toml_file_path())
        {
            ctx.emit(WarpConfigUpdateEvent::Settings);
        }

        if update_touches_dir(update, &custom_model_routers_dir()) {
            let model_config_dir = custom_model_routers_dir();
            let _ = ctx.spawn(
                async move { load_model_configs(&model_config_dir) },
                |me, (routers, errors), ctx| {
                    me.custom_model_routers = routers;
                    me.custom_model_router_errors = errors.clone();
                    ctx.emit(WarpConfigUpdateEvent::ModelConfigs);
                    if !errors.is_empty() {
                        ctx.emit(WarpConfigUpdateEvent::ModelConfigErrors(errors));
                    }
                },
            );
        }
    }

    /// Parse and atomically create one local custom model router.
    ///
    /// Existing files must be saved through
    /// [`Self::save_custom_model_router_with_revision`] so an editor cannot
    /// silently overwrite a watcher or another process update.
    #[cfg(feature = "local_fs")]
    pub fn save_custom_model_router(
        name: &str,
        yaml: &str,
        existing_path: Option<&Path>,
    ) -> Result<PathBuf> {
        let directory = custom_model_routers_dir();
        let repository = LocalCustomModelRouterRepository::new(&directory);
        let path = if let Some(path) = existing_path {
            path.to_path_buf()
        } else {
            repository
                .directory()
                .join(find_unused_router_file_name(name, &directory))
        };
        let router = parse_model_config_yaml(yaml, Some(&path))
            .map_err(|error| anyhow!("could not parse custom model router: {error}"))?;
        let stored = if existing_path.is_some() {
            return Err(anyhow!(
                "updating a custom model router requires the revision from the opened file"
            ));
        } else {
            let file_name = path
                .file_name()
                .ok_or_else(|| anyhow!("custom model router path has no file name"))?;
            repository.create(file_name, &router)?
        };
        Ok(stored.path)
    }

    /// Save an already-opened router using the revision captured by the
    /// editor. The repository performs the CAS against its opened directory
    /// descriptor; this helper never rereads a fresh revision on behalf of a
    /// caller.
    #[cfg(feature = "local_fs")]
    pub fn save_custom_model_router_with_revision(
        _name: &str,
        yaml: &str,
        existing_path: &Path,
        expected: &RouterFileRevision,
    ) -> Result<PathBuf> {
        let directory = custom_model_routers_dir();
        let repository = LocalCustomModelRouterRepository::new(&directory);
        let router = parse_model_config_yaml(yaml, Some(existing_path))
            .map_err(|error| anyhow!("could not parse custom model router: {error}"))?;
        let stored = repository.update(existing_path, expected, &router)?;
        Ok(stored.path)
    }

    /// Delete one managed local custom model router file.
    #[cfg(feature = "local_fs")]
    pub fn delete_custom_model_router(path: &Path) -> Result<()> {
        Err(anyhow!(
            "deleting a custom model router requires the revision from the opened file: {}",
            path.display()
        ))
    }

    #[cfg(feature = "local_fs")]
    pub fn delete_custom_model_router_checked(
        path: &Path,
        expected: &RouterFileRevision,
    ) -> Result<()> {
        LocalCustomModelRouterRepository::new(custom_model_routers_dir())
            .delete_checked(path, expected)
            .map_err(Into::into)
    }

    /// This method takes a file name candidate (appends .yaml if missing) and a LaunchConfig as
    /// arguments. It saves the file and returns the filename used if successful.
    #[cfg(feature = "local_fs")]
    pub fn save_new_launch_config(
        file_name: String,
        launch_config: LaunchConfig,
    ) -> Result<String> {
        let file_name = if is_config_file(&file_name) {
            file_name.trim().into()
        } else {
            format!("{file_name}.yaml")
        };

        if !has_name(file_name.trim()) {
            return Err(anyhow!("File name is empty"));
        };

        let path = crate::user_config::launch_configs_dir().join(&file_name);
        if path.exists() {
            return Err(anyhow!("File already exists"));
        };

        let file = crate::util::file::create_file(path)?;
        let mut writer = io::BufWriter::new(file);
        writer.write_all(LAUNCH_CONFIG_COMMENT.as_bytes())?;
        serde_yaml::to_writer(writer, &launch_config)?;
        Ok(file_name)
    }
}

pub fn load_theme_configs(theme_path: &Path) -> WarpThemeConfig {
    let mut theme_configs = WarpThemeConfig::new();
    for_each_dir_entry(theme_path, parse_single_theme_dir_entry)
        .into_iter()
        .for_each(|(theme_name, theme)| theme_configs.add_new_theme(theme_name, theme));
    theme_configs
}

/// Loads all workflows relative to the `workflow_path`.  A YAML file might
/// contain multiple workflows.
pub fn load_workflows(workflow_path: &Path) -> Vec<Workflow> {
    for_each_dir_entry(workflow_path, parse_multi_workflow_dir_entry)
        .into_iter()
        .flatten()
        .collect_vec()
}

/// Loads all launch configs relative to the `launch_config_path`. Each workflow is assumed to be in an
/// individual YAML file.
pub fn load_launch_configs(launch_config_path: &Path) -> Vec<LaunchConfig> {
    for_each_dir_entry(launch_config_path, parse_multi_launch_config_dir_entry)
        .into_iter()
        .flatten()
        .collect_vec()
}

/// Loads one strict local custom model router per YAML file.
pub fn load_model_configs(
    model_config_path: &Path,
) -> (Vec<CustomModelRouter>, Vec<ModelConfigError>) {
    let repository = LocalCustomModelRouterRepository::new(model_config_path);
    let (stored_routers, mut errors) = match repository.list_with_errors() {
        Ok(result) => result,
        Err(error) => {
            return (
                Vec::new(),
                vec![ModelConfigError {
                    file_name: model_config_path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("routers")
                        .to_owned(),
                    file_path: model_config_path.to_path_buf(),
                    error_message: error.to_string(),
                }],
            );
        }
    };
    let mut routers = Vec::new();
    let mut stable_ids = std::collections::HashSet::new();
    for stored in stored_routers {
        let router = stored.router;
        if stable_ids.insert(router.llm_id()) {
            routers.push(router);
        } else {
            errors.push(ModelConfigError {
                file_name: router
                    .source_path
                    .as_deref()
                    .and_then(Path::file_name)
                    .and_then(|name| name.to_str())
                    .unwrap_or("router.yaml")
                    .to_owned(),
                file_path: router.source_path.clone().unwrap_or_default(),
                error_message:
                    "duplicate router filename identity; use one YAML file per stable local id"
                        .to_owned(),
            });
        }
    }
    routers.sort_by(|left, right| {
        left.info
            .display_name
            .to_lowercase()
            .cmp(&right.info.display_name.to_lowercase())
            .then_with(|| left.info.display_name.cmp(&right.info.display_name))
    });
    (routers, errors)
}

#[cfg(feature = "local_fs")]
fn find_unused_router_file_name(name: &str, directory: &Path) -> String {
    let mut stem = String::new();
    for character in name.trim().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            stem.push(character);
        } else if !stem.ends_with('_') {
            stem.push('_');
        }
    }
    let stem = stem.trim_matches('_');
    let stem = if stem.is_empty() { "router" } else { stem };
    let base = format!("{stem}.yaml");
    if !directory.join(&base).exists() {
        return base;
    }
    for suffix in 2..=u32::MAX {
        let candidate = format!("{stem}_{suffix}.yaml");
        if !directory.join(&candidate).exists() {
            return candidate;
        }
    }
    unreachable!("router file name suffix space exhausted")
}

/// Loads all tab configs from `tab_config_path`. Each tab config is an individual TOML file.
///
/// Returns successfully parsed configs and any errors for files that failed to parse.
pub fn load_tab_configs(tab_config_path: &Path) -> (Vec<TabConfig>, Vec<TabConfigError>) {
    let results = for_each_dir_entry(tab_config_path, parse_tab_config_dir_entry);
    let mut configs = Vec::new();
    let mut errors = Vec::new();
    for result in results {
        match result {
            Ok(config) => configs.push(config),
            Err(error) => errors.push(error),
        }
    }
    configs.sort_by(|a, b| {
        let a_name = a.name.to_lowercase();
        let b_name = b.name.to_lowercase();
        a_name.cmp(&b_name).then_with(|| a.name.cmp(&b.name))
    });
    (configs, errors)
}

fn update_touches_dir(update: &RepositoryUpdate, path: &Path) -> bool {
    let canonical_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    repository_update_touches_prefix(update, path)
        || repository_update_touches_prefix(update, &canonical_path)
}

fn update_touches_workflows_dir(update: &RepositoryUpdate, path: &Path) -> bool {
    let touches = |candidate: &Path| candidate.starts_with(path) && !is_atomic_temp_path(candidate);
    update
        .added
        .iter()
        .map(|target| target.path.as_path())
        .chain(update.modified.iter().map(|target| target.path.as_path()))
        .chain(update.deleted.iter().map(|target| target.path.as_path()))
        .chain(update.moved.iter().flat_map(|(to_target, from_target)| {
            [to_target.path.as_path(), from_target.path.as_path()]
        }))
        .any(touches)
}

fn update_touches_path(update: &RepositoryUpdate, path: &Path) -> bool {
    let canonical_path = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    repository_update_touches_path(update, path)
        || repository_update_touches_path(update, &canonical_path)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        path::{Path, PathBuf},
    };

    use repo_metadata::{RepositoryUpdate, TargetFile};

    use super::update_touches_workflows_dir;

    fn update_with_added(path: PathBuf) -> RepositoryUpdate {
        RepositoryUpdate {
            added: HashSet::from([TargetFile::new(path, false)]),
            ..Default::default()
        }
    }

    fn update_with_deleted(path: PathBuf) -> RepositoryUpdate {
        RepositoryUpdate {
            deleted: HashSet::from([TargetFile::new(path, false)]),
            ..Default::default()
        }
    }

    fn update_with_move(to: PathBuf, from: PathBuf) -> RepositoryUpdate {
        RepositoryUpdate {
            moved: HashMap::from([(TargetFile::new(to, false), TargetFile::new(from, false))]),
            ..Default::default()
        }
    }

    #[test]
    fn local_saved_prompt_temporary_watcher_events_are_ignored() {
        let workflows_dir = Path::new("/tmp/warp-workflows");
        let temp_path = workflows_dir.join("local-prompts/.prompt.yaml.tmp-test");
        assert!(!update_touches_workflows_dir(
            &update_with_added(temp_path),
            workflows_dir,
        ));
    }

    #[test]
    fn local_saved_prompt_create_update_delete_emit_one_effective_refresh_each() {
        let workflows_dir = Path::new("/tmp/warp-workflows");
        let final_path = workflows_dir.join("local-prompts/prompt.yaml");
        let temp_path = workflows_dir.join("local-prompts/.prompt.yaml.tmp-test");

        let operations = [
            (
                "create",
                vec![
                    update_with_added(temp_path.clone()),
                    update_with_move(final_path.clone(), temp_path.clone()),
                ],
            ),
            (
                "update",
                vec![
                    update_with_added(temp_path.clone()),
                    update_with_move(final_path.clone(), temp_path.clone()),
                ],
            ),
            ("delete", vec![update_with_deleted(final_path.clone())]),
        ];

        for (operation, updates) in operations {
            let effective_refreshes = updates
                .iter()
                .filter(|update| update_touches_workflows_dir(update, workflows_dir))
                .count();
            assert_eq!(effective_refreshes, 1, "{operation} refresh count");
        }
    }
}
