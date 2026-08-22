use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;

use warpui::{App, SingletonEntity};

use super::super::zero_state::{
    GuiZeroStateDataSource, is_zero_state_skill_event, is_zero_state_workflow_event,
    should_show_local_prompts, should_show_local_skills,
};
use super::prefix_match_bonus;
use ai::skills::{ParsedSkill, SkillProvider, SkillScope};
use warp_util::local_or_remote_path::LocalOrRemotePath;

use crate::ai::skills::{SkillManager, SkillManagerEvent};
use crate::search::mixer::SearchMixerEvent;
use crate::search::slash_command_menu::fuzzy_match::SlashCommandFuzzyMatchResult;
use crate::terminal::input::slash_commands::mixer::{
    build_slash_command_mixer, rerun_current_slash_command_query,
};
use crate::terminal::input::tests::{add_window_with_bootstrapped_terminal, initialize_app};
use crate::user_config::{WarpConfig, WarpConfigUpdateEvent};

#[test]
fn local_zero_state_sources_prompts_are_available_without_cloud_mode() {
    assert!(should_show_local_prompts(true));
}

#[test]
fn local_zero_state_sources_skills_are_available_without_cloud_mode() {
    assert!(should_show_local_skills(true, true));
}

#[test]
fn local_zero_state_sources_actions_require_ai() {
    assert!(!should_show_local_prompts(false));
    assert!(!should_show_local_skills(false, true));
}

#[test]
fn local_zero_state_sources_list_skills_only_gates_skills() {
    assert!(should_show_local_prompts(true));
    assert!(!should_show_local_skills(true, false));
}

#[test]
fn local_zero_state_sources_refresh_for_workflow_and_skill_changes() {
    assert!(is_zero_state_workflow_event(
        &WarpConfigUpdateEvent::LocalUserWorkflows
    ));
    assert!(!is_zero_state_workflow_event(
        &WarpConfigUpdateEvent::Themes
    ));
    assert!(is_zero_state_skill_event(&SkillManagerEvent::SkillsChanged));
}

#[test]
fn local_zero_state_sources_requery_when_local_backing_stores_change() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);
        let terminal = add_window_with_bootstrapped_terminal(&mut app, None, None).await;
        let input = terminal.read(&app, |terminal, _| terminal.input().clone());
        let slash_commands_source =
            input.read(&app, |input, _| input.slash_command_data_source.clone());
        let zero_state_source =
            app.add_model(|ctx| GuiZeroStateDataSource::new(&slash_commands_source, ctx));
        let mixer = app.add_model(|ctx| {
            build_slash_command_mixer(
                slash_commands_source.clone(),
                zero_state_source.clone(),
                ctx,
            )
        });
        let agent_skill_path = LocalOrRemotePath::Local(PathBuf::from(
            "/tmp/warp-task3-agent-environment/.agents/skills/requery/SKILL.md",
        ));
        let agent_skill = ParsedSkill {
            path: agent_skill_path.clone(),
            name: "requery".to_owned(),
            description: "Refresh zero-state after agent skill load".to_owned(),
            content: "# Requery".to_owned(),
            line_range: None,
            provider: SkillProvider::Agents,
            scope: SkillScope::Project,
        };
        let source_events = Rc::new(Cell::new(0));
        let source_events_for_subscription = source_events.clone();
        let reruns = Rc::new(Cell::new(0));
        let reruns_for_subscription = reruns.clone();
        let zero_state_for_requery = zero_state_source.clone();
        let mixer_for_requery = mixer.clone();
        app.update(|ctx| {
            ctx.subscribe_to_model(
                &zero_state_for_requery,
                move |_, _: &super::super::zero_state::UpdatedZeroState, ctx| {
                    mixer_for_requery.update(ctx, rerun_current_slash_command_query);
                },
            );
            ctx.subscribe_to_model(
                &zero_state_source,
                move |_, _: &super::super::zero_state::UpdatedZeroState, _| {
                    source_events_for_subscription.set(source_events_for_subscription.get() + 1);
                },
            );
            ctx.subscribe_to_model(&mixer, move |_, _: &SearchMixerEvent, _| {
                reruns_for_subscription.set(reruns_for_subscription.get() + 1);
            });
        });

        let before_workflow_update = reruns.get();
        WarpConfig::handle(&app).update(&mut app, |_, ctx| {
            ctx.emit(WarpConfigUpdateEvent::LocalUserWorkflows);
        });
        assert!(
            reruns.get() > before_workflow_update,
            "local workflow changes should re-run the open zero-state query"
        );
        assert_eq!(
            source_events.get(),
            1,
            "local workflow changes should emit one zero-state source event"
        );

        let before_skill_update = reruns.get();
        SkillManager::handle(&app).update(&mut app, |manager, ctx| {
            manager.add_agent_environment_skills(vec![agent_skill.clone()], ctx);
        });
        assert!(
            reruns.get() > before_skill_update,
            "local skill changes should re-run the open zero-state query"
        );
        assert_eq!(
            source_events.get(),
            2,
            "local skill changes should emit one additional zero-state source event"
        );
        assert!(SkillManager::handle(&app).read(&app, |manager, _| {
            manager.skill_by_path(&agent_skill_path).is_some()
        }));

        let before_duplicate_skill = reruns.get();
        SkillManager::handle(&app).update(&mut app, |manager, ctx| {
            manager.add_agent_environment_skills(vec![agent_skill.clone()], ctx);
        });
        assert_eq!(
            reruns.get(),
            before_duplicate_skill,
            "re-adding an unchanged agent skill must not refresh the menu"
        );
        assert_eq!(source_events.get(), 2);

        let before_unrelated_update = reruns.get();
        WarpConfig::handle(&app).update(&mut app, |_, ctx| {
            ctx.emit(WarpConfigUpdateEvent::Themes);
        });
        assert_eq!(
            reruns.get(),
            before_unrelated_update,
            "unrelated WarpConfig changes must not re-run the zero-state query"
        );
    });
}

#[test]
fn exact_match_returns_full_bonus() {
    // Query "new" exactly matches the name "/new" (after stripping '/').
    let bonus = prefix_match_bonus("new", "/new");
    assert!((bonus - 100.0).abs() < f64::EPSILON);
}

#[test]
fn partial_prefix_returns_proportional_bonus() {
    // "for" is a prefix of "fork" → coverage 3/4 = 75.
    let bonus = prefix_match_bonus("for", "/fork");
    assert!((bonus - 75.0).abs() < f64::EPSILON);
}

#[test]
fn non_prefix_returns_zero() {
    // "new" is NOT a prefix of "create-new-project".
    let bonus = prefix_match_bonus("new", "/create-new-project");
    assert!((bonus - 0.0).abs() < f64::EPSILON);
}

#[test]
fn case_insensitive() {
    let bonus = prefix_match_bonus("new", "/New");
    assert!((bonus - 100.0).abs() < f64::EPSILON);
}

#[test]
fn name_without_slash_prefix() {
    // Skills don't have the '/' prefix in their name.
    let bonus = prefix_match_bonus("figma", "figma-create-new-file");
    let coverage = 5.0 / 21.0 * 100.0;
    assert!((bonus - coverage).abs() < f64::EPSILON);
}

#[test]
fn short_prefix_match_ranks_above_longer_fuzzy_match() {
    // Simulates the reported issue: query "new" should give /new a much
    // higher combined score than /figma-create-new-file.
    let short_match = SlashCommandFuzzyMatchResult::try_match("new", "/new", None).unwrap();
    let long_match =
        SlashCommandFuzzyMatchResult::try_match("new", "/figma-create-new-file", None).unwrap();

    const SCORE_MULTIPLIER: f64 = 1000.0;

    let short_score = short_match.score() * SCORE_MULTIPLIER
        + prefix_match_bonus("new", "/new") * SCORE_MULTIPLIER
        + 1.0 / "/new".len() as f64;
    let long_score = long_match.score() * SCORE_MULTIPLIER
        + prefix_match_bonus("new", "/figma-create-new-file") * SCORE_MULTIPLIER
        + 1.0 / "/figma-create-new-file".len() as f64;

    assert!(
        short_score > long_score,
        "/new score ({short_score}) should be greater than /figma-create-new-file score ({long_score})"
    );
}
