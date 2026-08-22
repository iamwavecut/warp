use super::*;
use crate::warp_managed_paths_watcher::WarpManagedPathsWatcher;
use ai::skills::{ParsedSkill, SkillProvider, SkillScope, get_provider_for_path};
use repo_metadata::{DirectoryWatcher, RepoMetadataModel, repositories::DetectedRepositories};
use std::collections::{HashMap, HashSet};
use std::fs;
use tempfile::TempDir;
use warp_core::channel::ChannelState;
use warp_core::ui::icons::Icon;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warpui::App;
use watcher::HomeDirectoryWatcher;

fn local(path: impl Into<PathBuf>) -> LocalOrRemotePath {
    LocalOrRemotePath::Local(path.into())
}

// ============================================================================
// Tests for get_skills_for_working_directory subdirectory scoping
// ============================================================================

#[test]
fn get_skills_for_working_directory_scopes_subdirectory_skills() {
    // This test verifies the key scoping behavior:
    // - Root skills are visible from anywhere in the repo
    // - Subdirectory skills are only visible when working_directory is within that subdirectory

    // Use real temp directories so DetectedRepositories can canonicalize paths
    // and correctly report repo_root, which controls ancestor-vs-descendant scoping.
    // Canonicalize the temp base to avoid macOS /var -> /private/var symlink mismatches.
    let temp = TempDir::new().unwrap();
    let base = dunce::canonicalize(temp.path()).unwrap();
    let repo = base.join("repo");
    let frontend_dir = repo.join("packages/frontend");
    let backend_dir = repo.join("packages/backend");
    fs::create_dir_all(&frontend_dir).unwrap();
    fs::create_dir_all(&backend_dir).unwrap();

    // Create mock skills
    let root_skill_path = repo.join(".agents/skills/root-skill/SKILL.md");
    let frontend_skill_path = frontend_dir.join(".agents/skills/frontend-skill/SKILL.md");

    let root_skill = ParsedSkill {
        name: "root-skill".to_string(),
        description: "A root skill".to_string(),
        path: local(root_skill_path.clone()),
        content: "# Root skill".to_string(),
        line_range: None,
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let frontend_skill = ParsedSkill {
        name: "frontend-skill".to_string(),
        description: "A frontend skill".to_string(),
        path: local(frontend_skill_path.clone()),
        content: "# Frontend skill".to_string(),
        line_range: None,
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    // Build the internal state manually
    let mut directory_skills: HashMap<LocalOrRemotePath, HashSet<LocalOrRemotePath>> =
        HashMap::new();
    directory_skills
        .entry(local(repo.clone()))
        .or_default()
        .insert(local(root_skill_path.clone()));
    directory_skills
        .entry(local(frontend_dir.clone()))
        .or_default()
        .insert(local(frontend_skill_path.clone()));

    let mut skills_by_path: HashMap<LocalOrRemotePath, ParsedSkill> = HashMap::new();
    skills_by_path.insert(local(root_skill_path.clone()), root_skill);
    skills_by_path.insert(local(frontend_skill_path.clone()), frontend_skill);

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        let repo_handle = app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let skill_manager_handle = app.add_singleton_model(SkillManager::new);

        // Register the repo root so get_root_for_path returns Some.
        let canonical_repo =
            warp_util::standardized_path::StandardizedPath::from_local_canonicalized(&repo)
                .unwrap();
        repo_handle.update(&mut app, |repos, _ctx| {
            repos.insert_test_repo_root(canonical_repo);
        });

        // Inject the test state
        skill_manager_handle.update(&mut app, |manager, _ctx| {
            manager.directory_skills = directory_skills;
            manager.skills_by_path = skills_by_path;
        });

        // Test 1: From frontend directory, should see both root and frontend skills
        let skills_from_frontend = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_directory(Some(&frontend_dir), ctx)
        });
        let names_from_frontend: Vec<&str> = skills_from_frontend
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            names_from_frontend.contains(&"root-skill"),
            "Root skill should be visible from frontend dir"
        );
        assert!(
            names_from_frontend.contains(&"frontend-skill"),
            "Frontend skill should be visible from frontend dir"
        );

        // Test 2: From backend directory, should only see root skill (not frontend skill)
        let skills_from_backend = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_directory(Some(&backend_dir), ctx)
        });
        let names_from_backend: Vec<&str> = skills_from_backend
            .iter()
            .map(|s| s.name.as_str())
            .collect();
        assert!(
            names_from_backend.contains(&"root-skill"),
            "Root skill should be visible from backend dir"
        );
        assert!(
            !names_from_backend.contains(&"frontend-skill"),
            "Frontend skill should NOT be visible from backend dir"
        );

        // Test 3: From repo root, should only see root skill (not frontend skill)
        let skills_from_root = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_directory(Some(&repo), ctx)
        });
        let names_from_root: Vec<&str> = skills_from_root.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names_from_root.contains(&"root-skill"),
            "Root skill should be visible from repo root"
        );
        assert!(
            !names_from_root.contains(&"frontend-skill"),
            "Frontend skill should NOT be visible from repo root"
        );
    });
}

#[test]
fn get_skills_for_working_directory_name_collision_returns_both() {
    // When the same skill name exists at root and subdirectory, both should be returned.
    // The caller (agent) is responsible for precedence based on path proximity.

    // Use real temp directories so DetectedRepositories can canonicalize paths.
    // Canonicalize the temp base to avoid macOS /var -> /private/var symlink mismatches.
    let temp = TempDir::new().unwrap();
    let base = dunce::canonicalize(temp.path()).unwrap();
    let repo = base.join("repo");
    let subdir = repo.join("packages/frontend");
    fs::create_dir_all(&subdir).unwrap();

    let root_skill_path = repo.join(".agents/skills/deploy/SKILL.md");
    let subdir_skill_path = subdir.join(".agents/skills/deploy/SKILL.md");

    let root_skill = ParsedSkill {
        name: "deploy".to_string(),
        description: "Root deploy".to_string(),
        path: local(root_skill_path.clone()),
        content: "# Root deploy".to_string(),
        line_range: None,
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let subdir_skill = ParsedSkill {
        name: "deploy".to_string(),
        description: "Subdir deploy".to_string(),
        path: local(subdir_skill_path.clone()),
        content: "# Subdir deploy".to_string(),
        line_range: None,
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let mut directory_skills: HashMap<LocalOrRemotePath, HashSet<LocalOrRemotePath>> =
        HashMap::new();
    directory_skills
        .entry(local(repo.clone()))
        .or_default()
        .insert(local(root_skill_path.clone()));
    directory_skills
        .entry(local(subdir.clone()))
        .or_default()
        .insert(local(subdir_skill_path.clone()));

    let mut skills_by_path: HashMap<LocalOrRemotePath, ParsedSkill> = HashMap::new();
    skills_by_path.insert(local(root_skill_path.clone()), root_skill);
    skills_by_path.insert(local(subdir_skill_path.clone()), subdir_skill);

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        let repo_handle = app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let skill_manager_handle = app.add_singleton_model(SkillManager::new);

        // Register the repo root so get_root_for_path returns Some.
        let canonical_repo =
            warp_util::standardized_path::StandardizedPath::from_local_canonicalized(&repo)
                .unwrap();
        repo_handle.update(&mut app, |repos, _ctx| {
            repos.insert_test_repo_root(canonical_repo);
        });

        skill_manager_handle.update(&mut app, |manager, _ctx| {
            manager.directory_skills = directory_skills;
            manager.skills_by_path = skills_by_path;
        });

        // From subdir: should see both "deploy" skills (root + subdir)
        let skills = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_directory(Some(&subdir), ctx)
        });
        let deploy_skills: Vec<_> = skills.iter().filter(|s| s.name == "deploy").collect();
        assert_eq!(
            deploy_skills.len(),
            2,
            "Both deploy skills should be visible from subdir"
        );

        // From repo root: should only see root "deploy"
        let skills = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_directory(Some(&repo), ctx)
        });
        let deploy_skills: Vec<_> = skills.iter().filter(|s| s.name == "deploy").collect();
        assert_eq!(
            deploy_skills.len(),
            1,
            "Only root deploy should be visible from repo root"
        );
        assert_eq!(deploy_skills[0].description, "Root deploy");
    });
}

#[test]
fn agent_environment_skills_always_included() {
    // In an agent environment, all skills should be in scope regardless of
    // the working directory—even when cwd is inside a different repo or
    // when working_directory is None.

    let temp = TempDir::new().unwrap();
    let base = dunce::canonicalize(temp.path()).unwrap();
    let repo_a = base.join("repo-a");
    let repo_b = base.join("repo-b");
    fs::create_dir_all(&repo_a).unwrap();
    fs::create_dir_all(&repo_b).unwrap();

    let skill_a_path = repo_a.join(".agents/skills/build/SKILL.md");
    let skill_b_path = repo_b.join(".agents/skills/deploy/SKILL.md");

    let skill_a = ParsedSkill {
        name: "build".to_string(),
        description: "Repo A skill".to_string(),
        path: local(skill_a_path.clone()),
        content: "# Build".to_string(),
        line_range: None,
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let skill_b = ParsedSkill {
        name: "deploy".to_string(),
        description: "Repo B skill".to_string(),
        path: local(skill_b_path.clone()),
        content: "# Deploy".to_string(),
        line_range: None,
        provider: SkillProvider::Agents,
        scope: SkillScope::Project,
    };

    let mut directory_skills: HashMap<LocalOrRemotePath, HashSet<LocalOrRemotePath>> =
        HashMap::new();
    directory_skills
        .entry(local(repo_a.clone()))
        .or_default()
        .insert(local(skill_a_path.clone()));
    directory_skills
        .entry(local(repo_b.clone()))
        .or_default()
        .insert(local(skill_b_path.clone()));

    let mut skills_by_path: HashMap<LocalOrRemotePath, ParsedSkill> = HashMap::new();
    skills_by_path.insert(local(skill_a_path.clone()), skill_a);
    skills_by_path.insert(local(skill_b_path.clone()), skill_b);

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        let repo_handle = app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let skill_manager_handle = app.add_singleton_model(SkillManager::new);

        let canonical_repo_a =
            warp_util::standardized_path::StandardizedPath::from_local_canonicalized(&repo_a)
                .unwrap();
        repo_handle.update(&mut app, |repos, _ctx| {
            repos.insert_test_repo_root(canonical_repo_a);
        });

        skill_manager_handle.update(&mut app, |manager, _ctx| {
            manager.directory_skills = directory_skills;
            manager.skills_by_path = skills_by_path;
            manager.is_agent_environment = true;
        });

        // From inside repo_a, both repo_a and repo_b skills are visible
        // because is_agent_environment skips the ancestor filter.
        let skills = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_directory(Some(&repo_a), ctx)
        });
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names.contains(&"build"),
            "Repo A skill should be visible from repo A"
        );
        assert!(
            names.contains(&"deploy"),
            "Repo B skill should be visible from repo A in agent environment"
        );

        // With no working directory, all skills are still included.
        let skills_none = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_directory(None, ctx)
        });
        let names_none: Vec<&str> = skills_none.iter().map(|s| s.name.as_str()).collect();
        assert!(
            names_none.contains(&"build"),
            "Repo A skill should be visible even without a working directory"
        );
        assert!(
            names_none.contains(&"deploy"),
            "Repo B skill should be visible even without a working directory"
        );
    });
}

#[test]
fn get_skills_for_working_location_keeps_local_and_remote_skills_separate() {
    let temp = TempDir::new().unwrap();
    let local_repo = temp.path().join("local-repo");
    fs::create_dir_all(&local_repo).unwrap();
    let local_repo = dunce::canonicalize(local_repo).unwrap();

    let same_host = warp_util::host_id::HostId::new("same-host".to_string());
    let other_host = warp_util::host_id::HostId::new("other-host".to_string());
    let same_host_repo = LocalOrRemotePath::Remote(warp_util::remote_path::RemotePath::new(
        same_host.clone(),
        warp_util::standardized_path::StandardizedPath::try_new("/repo").unwrap(),
    ));
    let other_host_repo = LocalOrRemotePath::Remote(warp_util::remote_path::RemotePath::new(
        other_host,
        warp_util::standardized_path::StandardizedPath::try_new("/repo").unwrap(),
    ));

    let local_skill_path = local(local_repo.join(".agents/skills/local-only/SKILL.md"));
    let same_host_skill_path = same_host_repo.join(".agents/skills/same-host/SKILL.md");
    let other_host_skill_path = other_host_repo.join(".agents/skills/other-host/SKILL.md");

    let mut directory_skills: HashMap<LocalOrRemotePath, HashSet<LocalOrRemotePath>> =
        HashMap::new();
    directory_skills
        .entry(local(local_repo.clone()))
        .or_default()
        .insert(local_skill_path.clone());
    directory_skills
        .entry(same_host_repo.clone())
        .or_default()
        .insert(same_host_skill_path.clone());
    directory_skills
        .entry(other_host_repo)
        .or_default()
        .insert(other_host_skill_path.clone());

    let mut skills_by_path = HashMap::new();
    for (name, path) in [
        ("local-only", local_skill_path),
        ("same-host", same_host_skill_path),
        ("other-host", other_host_skill_path),
    ] {
        skills_by_path.insert(
            path.clone(),
            ParsedSkill {
                name: name.to_string(),
                description: format!("{name} skill"),
                path,
                content: format!("# {name}"),
                line_range: None,
                provider: SkillProvider::Agents,
                scope: SkillScope::Project,
            },
        );
    }

    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let skill_manager_handle = app.add_singleton_model(SkillManager::new);

        skill_manager_handle.update(&mut app, |manager, _ctx| {
            manager.directory_skills = directory_skills;
            manager.skills_by_path = skills_by_path;
            manager.is_agent_environment = true;
        });

        let remote_skills = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_location(Some(&same_host_repo), ctx)
        });
        let remote_names: HashSet<&str> = remote_skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert!(remote_names.contains("same-host"));
        assert!(!remote_names.contains("local-only"));
        assert!(!remote_names.contains("other-host"));

        let local_skills = skill_manager_handle.read(&app, |manager, ctx| {
            manager.get_skills_for_working_location(Some(&local(local_repo.clone())), ctx)
        });
        let local_names: HashSet<&str> = local_skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert!(local_names.contains("local-only"));
        assert!(!local_names.contains("same-host"));
        assert!(!local_names.contains("other-host"));
    });
}

#[test]
fn test_read_bundled_skills_with_variable_substitution() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path();

    // Create a test skill with variables
    let skill_dir = skills_dir.join("test-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    let skill_file = skill_dir.join("SKILL.md");
    fs::write(
        &skill_file,
        r#"---
name: test-skill
description: Test skill with variables
---

Run `{{warp_cli_binary_name}}` to connect to {{warp_server_url}}.
"#,
    )
    .unwrap();

    let skills = futures::executor::block_on(read_bundled_skills(skills_dir));

    assert_eq!(skills.len(), 1);
    let skill = skills.get("test-skill").unwrap();

    let expected_cli = ChannelState::channel().cli_command_name();
    assert!(
        skill
            .content
            .contains(&format!("Run `{expected_cli}` to connect to ."))
    );
}

#[test]
fn test_read_bundled_skills_preserves_other_content() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path();

    // Create a test skill with both warp and non-warp variables
    let skill_dir = skills_dir.join("test-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    let skill_file = skill_dir.join("SKILL.md");
    fs::write(
        &skill_file,
        r#"---
name: test-skill
description: Test skill with mixed variables
---

Use {{other_var}} and {{warp_cli_binary_name}} together.
"#,
    )
    .unwrap();

    let skills = futures::executor::block_on(read_bundled_skills(skills_dir));

    assert_eq!(skills.len(), 1);
    let skill = skills.get("test-skill").unwrap();

    let expected_cli = ChannelState::channel().cli_command_name();
    assert!(skill.content.contains(&format!(
        "Use {{{{other_var}}}} and {expected_cli} together."
    )));
}

#[test]
fn test_read_bundled_skills_no_variables() {
    let temp_dir = TempDir::new().unwrap();
    let skills_dir = temp_dir.path();

    // Create a test skill with no variables
    let skill_dir = skills_dir.join("test-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    let skill_file = skill_dir.join("SKILL.md");
    fs::write(
        &skill_file,
        r#"---
name: test-skill
description: Test skill without variables
---

Plain content with no variables.
"#,
    )
    .unwrap();

    let skills = futures::executor::block_on(read_bundled_skills(skills_dir));

    assert_eq!(skills.len(), 1);
    let skill = skills.get("test-skill").unwrap();
    assert!(skill.content.contains("Plain content with no variables."));
}

#[test]
fn test_build_bundled_skill_context() {
    let context = build_bundled_skill_context();

    // At least 5 entries: server_url, cli_binary_name, url_scheme, settings_file_path, keybindings_file_path.
    // settings_schema_path is only present when bundled_resources_dir() returns Some.
    assert!(context.len() >= 5);
    assert!(context.contains_key("warp_server_url"));
    assert!(context.contains_key("warp_cli_binary_name"));
    assert!(context.contains_key("warp_url_scheme"));
    assert!(context.contains_key("settings_file_path"));
    assert!(context.contains_key("keybindings_file_path"));

    assert_eq!(context.get("warp_server_url").unwrap(), "");
    assert_eq!(
        context.get("warp_cli_binary_name").unwrap(),
        ChannelState::channel().cli_command_name()
    );
    assert_eq!(
        context.get("warp_url_scheme").unwrap(),
        ChannelState::url_scheme()
    );
    assert_eq!(
        context.get("settings_file_path").unwrap(),
        &crate::settings::user_preferences_toml_file_path()
            .display()
            .to_string()
    );
    assert_eq!(
        context.get("keybindings_file_path").unwrap(),
        &crate::keyboard::keybinding_file_path()
            .display()
            .to_string()
    );
}

#[test]
fn bundled_skill_catalog_merge_is_deterministic_and_noop_safe() {
    let bundled = HashMap::from([
        (
            "base".to_owned(),
            make_bundled_skill_for_test("bundled-base", BundledSkillActivation::Always, Icon::File),
        ),
        (
            "shared".to_owned(),
            make_bundled_skill_for_test(
                "bundled-shared",
                BundledSkillActivation::Always,
                Icon::File,
            ),
        ),
    ]);
    let figma = HashMap::from([
        (
            "figma-only".to_owned(),
            make_bundled_skill_for_test(
                "figma-only",
                BundledSkillActivation::RequiresMcp(crate::ai::mcp::McpIntegration::Figma),
                Icon::Figma,
            ),
        ),
        (
            "shared".to_owned(),
            make_bundled_skill_for_test(
                "figma-shared",
                BundledSkillActivation::RequiresMcp(crate::ai::mcp::McpIntegration::Figma),
                Icon::Figma,
            ),
        ),
    ]);

    // The two async loads are merged by source role, so completion order cannot discard Figma
    // entries. Figma retains the previous `extend(figma_skills)` precedence for duplicate IDs.
    let merged = merge_bundled_skill_catalog(bundled.clone(), figma.clone());
    let merged_again = merge_bundled_skill_catalog(bundled, figma);
    assert_eq!(merged, merged_again);
    assert_eq!(merged.len(), 3);
    assert_eq!(merged["base"].skill.name, "bundled-base");
    assert_eq!(merged["figma-only"].skill.name, "figma-only");
    assert_eq!(merged["shared"].skill.name, "figma-shared");
    assert_eq!(merged["shared"].icon, Icon::Figma);

    let mut current = HashMap::new();
    assert!(replace_bundled_skill_catalog(&mut current, merged.clone()));
    assert!(!replace_bundled_skill_catalog(&mut current, merged));
    assert!(replace_bundled_skill_catalog(&mut current, HashMap::new()));
    assert!(!replace_bundled_skill_catalog(&mut current, HashMap::new()));
}

#[test]
fn same_path_skill_rename_updates_reverse_index_and_provider_eligibility() {
    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let handle = app.add_singleton_model(SkillManager::new);

        let agents_skill = make_skill("deploy", ".agents");
        let claude_skill = make_skill("deploy", ".claude");
        let renamed_claude_skill = ParsedSkill {
            name: "renamed-deploy".to_owned(),
            description: "renamed-deploy skill".to_owned(),
            content: "# renamed-deploy".to_owned(),
            ..claude_skill.clone()
        };

        handle.update(&mut app, |manager, _| {
            assert!(manager.handle_skills_added(vec![agents_skill.clone(), claude_skill.clone(),]));
        });

        let old_descriptor = SkillDescriptor::from(agents_skill.clone());
        assert_eq!(
            handle.read(&app, |manager, _| manager.skill_paths_by_name("deploy")),
            vec![agents_skill.path.clone(), claude_skill.path.clone()]
        );
        assert!(handle.read(&app, |manager, _| {
            manager.skill_exists_for_any_provider(&old_descriptor, &[SkillProvider::Claude])
        }));

        handle.update(&mut app, |manager, _| {
            assert!(manager.handle_skills_added(vec![renamed_claude_skill.clone()]));
        });

        assert_eq!(
            handle.read(&app, |manager, _| manager.skill_paths_by_name("deploy")),
            vec![agents_skill.path.clone()]
        );
        assert_eq!(
            handle.read(&app, |manager, _| manager
                .skill_paths_by_name("renamed-deploy")),
            vec![claude_skill.path.clone()]
        );
        assert!(!handle.read(&app, |manager, _| {
            manager.skill_exists_for_any_provider(&old_descriptor, &[SkillProvider::Claude])
        }));

        let renamed_descriptor = SkillDescriptor::from(renamed_claude_skill.clone());
        assert!(handle.read(&app, |manager, _| {
            manager.skill_exists_for_any_provider(&renamed_descriptor, &[SkillProvider::Claude])
        }));
        assert_eq!(
            handle.read(&app, |manager, _| manager
                .skill_by_path(&claude_skill.path)
                .map(|skill| skill.name.clone())),
            Some("renamed-deploy".to_owned())
        );
    });
}

fn make_bundled_skill_for_test(
    name: &str,
    activation: BundledSkillActivation,
    icon: Icon,
) -> BundledSkill {
    BundledSkill {
        skill: ParsedSkill {
            path: local(format!("/bundled/{name}/SKILL.md")),
            name: name.to_owned(),
            description: name.to_owned(),
            content: format!("# {name}"),
            line_range: None,
            provider: SkillProvider::Warp,
            scope: SkillScope::Bundled,
        },
        activation,
        icon,
    }
}

// ============================================================================
// Tests for best_supported_provider
// ============================================================================

/// Helper: creates a ParsedSkill under a given provider directory.
fn make_skill(name: &str, provider_dir: &str) -> ParsedSkill {
    let path = PathBuf::from(format!("/repo/{provider_dir}/skills/{name}/SKILL.md"));
    let skill_path = local(path);
    ParsedSkill {
        name: name.to_string(),
        description: format!("{name} skill"),
        path: skill_path.clone(),
        content: format!("# {name}"),
        line_range: None,
        provider: get_provider_for_path(&skill_path).unwrap_or(SkillProvider::Warp),
        scope: SkillScope::Project,
    }
}

#[test]
fn best_supported_provider_fast_path_returns_deduped_provider() {
    // When the deduped provider is already in the supported set, return it immediately.
    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let handle = app.add_singleton_model(SkillManager::new);

        let claude_skill = make_skill("deploy", ".claude");
        handle.update(&mut app, |manager, _| {
            manager.add_skill_for_testing(claude_skill.clone());
        });

        let descriptor = SkillDescriptor::from(claude_skill);
        let result = handle.read(&app, |manager, _| {
            manager.best_supported_provider(&descriptor, &[SkillProvider::Claude])
        });
        assert_eq!(result, SkillProvider::Claude);
    });
}

#[test]
fn best_supported_provider_remaps_to_supported_provider() {
    // Skill exists under both .agents and .claude. Dedup picked Agents (higher priority).
    // When supported set is [Claude], should re-map to Claude.
    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let handle = app.add_singleton_model(SkillManager::new);

        let agents_skill = make_skill("deploy", ".agents");
        let claude_skill = make_skill("deploy", ".claude");
        handle.update(&mut app, |manager, _| {
            manager.add_skill_for_testing(agents_skill.clone());
            manager.add_skill_for_testing(claude_skill.clone());
        });

        // Descriptor has provider = Agents (the dedup winner).
        let descriptor = SkillDescriptor::from(agents_skill);
        assert_eq!(descriptor.provider, SkillProvider::Agents);

        let result = handle.read(&app, |manager, _| {
            manager.best_supported_provider(&descriptor, &[SkillProvider::Claude])
        });
        assert_eq!(result, SkillProvider::Claude);
    });
}

#[test]
fn best_supported_provider_falls_back_when_no_match() {
    // Skill only exists under .agents, but the supported set is [Claude].
    // Should fall back to the original deduped provider (Agents).
    App::test((), |mut app| async move {
        app.add_singleton_model(DirectoryWatcher::new);
        app.add_singleton_model(|_| DetectedRepositories::default());
        app.add_singleton_model(RepoMetadataModel::new);
        app.add_singleton_model(HomeDirectoryWatcher::new_for_test);
        app.add_singleton_model(WarpManagedPathsWatcher::new_for_testing);
        let handle = app.add_singleton_model(SkillManager::new);

        let agents_skill = make_skill("deploy", ".agents");
        handle.update(&mut app, |manager, _| {
            manager.add_skill_for_testing(agents_skill.clone());
        });

        let descriptor = SkillDescriptor::from(agents_skill);
        let result = handle.read(&app, |manager, _| {
            manager.best_supported_provider(&descriptor, &[SkillProvider::Claude])
        });
        // No .claude path exists, so falls back to the deduped provider.
        assert_eq!(result, SkillProvider::Agents);
    });
}
