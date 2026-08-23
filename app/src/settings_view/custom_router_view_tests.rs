use pathfinder_geometry::vector::vec2f;
use warp_core::ui::appearance::Appearance;
use warpui::elements::{Flex, ParentElement};
use warpui::platform::WindowStyle;
use warpui::{
    App, Element, Entity, Presenter, SingletonEntity, TypedActionView, View, WindowInvalidation,
};

use crate::ai::custom_model_routers::{
    ComplexityRouting, CustomModelRouter, CustomModelRouting, LocalCustomModelRouterRepository,
    LocalCustomModelRouterRepositoryError, concrete_custom_model_ids,
};
use crate::settings::{CustomApiType, CustomProviderCapabilities, CustomProviderConfig};

use super::render_router_error_card;

fn provider(name: &str, models: &[&str]) -> CustomProviderConfig {
    CustomProviderConfig {
        name: name.to_owned(),
        base_url: format!("http://{name}.test/v1"),
        models: models.iter().map(|model| (*model).to_owned()).collect(),
        api_type: CustomApiType::OpenAiCompatible,
        capabilities: CustomProviderCapabilities::default(),
        ..Default::default()
    }
}

/// Root view that lays a custom-router error card inside a bare
/// `Flex::column()`. This mirrors the Custom Routers settings section, which
/// renders its cards inside a vertically-scrollable container that passes an
/// **unbounded** (infinite) vertical constraint down to each card: a
/// `Flex::column()` lays out its non-flexible child with an infinite main-axis
/// constraint, exactly like the real settings page.
struct ErrorCardTestView;

impl Entity for ErrorCardTestView {
    type Event = ();
}

impl View for ErrorCardTestView {
    fn ui_name() -> &'static str {
        "CustomRouterErrorCardTestView"
    }

    fn render(&self, app: &warpui::AppContext) -> Box<dyn warpui::Element> {
        let appearance = Appearance::as_ref(app);
        Flex::column()
            .with_child(render_router_error_card(
                "broken_router.yaml",
                "`My Router`: complexity type requires a `default` model",
                appearance,
            ))
            .finish()
    }
}

impl TypedActionView for ErrorCardTestView {
    type Action = ();
}

/// Regression test for the Custom Routers crash: deleting every model name in a
/// router `.yaml` makes it fail to parse, so the settings page renders an error
/// card. Before the fix, that card wrapped its message in a `Shrinkable`
/// (a *flexible* flex child) inside a `Flex::column()`, so laying it out under
/// the settings page's unbounded vertical constraint panicked in flex layout
/// with "flex contains flexible children but has an infinite constraint along
/// the flex axis". Building the scene here must not panic.
#[test]
fn error_card_lays_out_under_unbounded_vertical_constraint_without_panicking() {
    App::test((), |mut app| async move {
        let app = &mut app;
        app.add_singleton_model(|_| Appearance::mock());

        let (window_id, _view) = app.add_window(WindowStyle::NotStealFocus, |_| ErrorCardTestView);
        let root_view_id = app
            .root_view_id(window_id)
            .expect("window should have a root view");

        let mut presenter = Presenter::new(window_id);
        let invalidation = WindowInvalidation {
            updated: [root_view_id].into_iter().collect(),
            ..Default::default()
        };

        app.update(move |ctx| {
            presenter.invalidate(invalidation, ctx);
            // Panicked here before the fix.
            presenter.build_scene(vec2f(400., 400.), 1., None, ctx);
        });
    });
}

#[test]
fn create_edit_rename_delete_actions_preserve_stable_file_id() {
    let directory = tempfile::tempdir().expect("temporary router directory");
    let repository = LocalCustomModelRouterRepository::new(directory.path());
    let router = CustomModelRouter::new_local(
        "First router".to_owned(),
        CustomModelRouting::Complexity(ComplexityRouting {
            default: "custom/local/fast".to_owned(),
            easy: None,
            medium: None,
            hard: None,
        }),
        None,
    );

    let created = repository
        .create("first-router.yaml", &router)
        .expect("create action writes one managed file");
    let stable_id = created.router.llm_id();
    let current = repository.read(&created.path).expect("read created router");
    let renamed = router.with_display_name("Renamed router".to_owned());
    let updated = repository
        .update(&created.path, &current.revision, &renamed)
        .expect("edit and rename action updates with CAS");
    assert_eq!(updated.router.llm_id(), stable_id);
    assert_eq!(updated.router.info.display_name, "Renamed router");

    repository
        .delete_checked(&updated.path, &updated.revision)
        .expect("confirmed delete action removes the validated file");
    assert!(repository.list().expect("list managed routers").is_empty());
}

#[test]
fn dirty_cancel_confirmation_and_cas_errors_retain_draft() {
    let directory = tempfile::tempdir().expect("temporary router directory");
    let repository = LocalCustomModelRouterRepository::new(directory.path());
    let router = CustomModelRouter::new_local(
        "Draft".to_owned(),
        CustomModelRouting::Complexity(ComplexityRouting {
            default: "custom/local/fast".to_owned(),
            easy: None,
            medium: None,
            hard: None,
        }),
        None,
    );
    let created = repository
        .create("draft.yaml", &router)
        .expect("create draft file");
    let stale = repository.read(&created.path).expect("read draft revision");
    let external = router.with_display_name("External edit".to_owned());
    repository
        .update(&created.path, &stale.revision, &external)
        .expect("simulate an external edit");

    let error = repository
        .update(&created.path, &stale.revision, &router)
        .expect_err("stale editor must fail compare-and-swap");
    assert!(matches!(
        error,
        LocalCustomModelRouterRepositoryError::Conflict { .. }
    ));
    let message = super::save_error_message(error);
    assert!(message.contains("draft is retained"));
    assert_eq!(
        repository
            .read(&created.path)
            .unwrap()
            .router
            .info
            .display_name,
        "External edit"
    );
}

#[test]
fn model_list_filtering_excludes_invalid_duplicate_and_non_concrete_targets() {
    let mut invalid = provider("invalid", &["ok"]);
    invalid.base_url.clear();
    let providers = vec![
        provider("local", &["fast", "router:nested", "auto"]),
        provider("duplicate", &["one"]),
        provider("duplicate", &["two"]),
        invalid,
    ];
    assert_eq!(
        concrete_custom_model_ids(&providers),
        vec!["custom/local/auto", "custom/local/fast"]
    );
}
