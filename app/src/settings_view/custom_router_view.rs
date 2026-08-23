use std::path::PathBuf;

use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, SingletonEntity as _, TypedActionView, View, ViewContext,
    ViewHandle,
    elements::{
        Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Expanded,
        Flex, MainAxisSize, ParentElement, Radius, Text,
    },
    fonts::{Properties, Weight},
};

use crate::ai::agent::api::direct_openai::effective_capabilities_for_config;
use crate::ai::custom_model_routers::{
    ComplexityRouting, CustomModelRouter, CustomModelRouting, LocalCustomModelRouterRepository,
    LocalCustomModelRouterRepositoryError, ModelConfigError, PromptRouting, PromptRule,
    RouterFileRevision, concrete_custom_model_ids, router_catalog_entry,
};
use crate::ai::llms::LLMPreferences;
use crate::editor::{EditorView, Event as EditorEvent, SingleLineEditorOptions, TextOptions};
use crate::settings::AISettings;
use crate::ui_components::icons::Icon as ActionIcon;
use crate::user_config::{WarpConfig, WarpConfigUpdateEvent, custom_model_routers_dir};
use crate::view_components::action_button::{
    ActionButton, ButtonSize, DangerNakedTheme, PrimaryTheme, SecondaryTheme,
};
use crate::view_components::{DropdownItem, FilterableDropdown};

/// Render a bounded, non-flexible parse error card for the local router
/// editor. The card is intentionally usable inside an unbounded vertical
/// settings scroll container.
#[cfg(feature = "local_fs")]
pub fn render_router_error_card(
    file_name: impl Into<String>,
    error_message: impl Into<String>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    render_router_error_card_with_actions(file_name, error_message, None, appearance)
}

struct RouterErrorHandles {
    open_button: ViewHandle<ActionButton>,
    delete_button: ViewHandle<ActionButton>,
}

#[cfg(feature = "local_fs")]
fn render_router_error_card_with_actions(
    file_name: impl Into<String>,
    error_message: impl Into<String>,
    handles: Option<&RouterErrorHandles>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let error_fill = warp_core::ui::theme::Fill::Solid(theme.ui_error_color());
    let sub = theme.sub_text_color(theme.surface_2());
    let file_name = file_name.into();
    let error_message = error_message.into();

    let name_row = Flex::row()
        .with_constrain_horizontal_bounds_to_parent(true)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            ConstrainedBox::new(
                Container::new(Icon::AlertTriangle.to_warpui_icon(error_fill).finish())
                    .with_margin_right(8.)
                    .finish(),
            )
            .with_width(24.)
            .with_height(16.)
            .finish(),
        )
        .with_child(
            Text::new(file_name, appearance.ui_font_family(), 13.)
                .with_style(Properties::default().weight(Weight::Medium))
                .with_color(theme.active_ui_text_color().into())
                .finish(),
        )
        .finish();

    let truncated = if error_message.chars().count() > 200 {
        format!("{}…", error_message.chars().take(200).collect::<String>())
    } else {
        error_message
    };
    let error_row = Text::new(truncated, appearance.ui_font_family(), 11.)
        .with_color(sub.into())
        .soft_wrap(true)
        .finish();

    let actions = handles.map(|handles| {
        Flex::row()
            .with_spacing(8.)
            .with_child(ChildView::new(&handles.open_button).finish())
            .with_child(ChildView::new(&handles.delete_button).finish())
            .finish()
    });

    ConstrainedBox::new(
        Container::new({
            let mut card = Flex::column()
                .with_child(
                    ConstrainedBox::new(Container::new(name_row).with_margin_bottom(6.).finish())
                        .with_max_width(568.)
                        .with_max_height(32.)
                        .finish(),
                )
                .with_child(
                    ConstrainedBox::new(error_row)
                        .with_max_width(568.)
                        .with_max_height(160.)
                        .finish(),
                );
            if let Some(actions) = actions {
                card = card.with_child(Container::new(actions).with_margin_top(8.).finish());
            }
            card.finish()
        })
        .with_background(theme.surface_2())
        .with_border(Border::new(1.).with_border_fill(error_fill))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
        .with_horizontal_padding(16.)
        .with_vertical_padding(10.)
        .finish(),
    )
    // A bare settings test (and some scroll containers) can leave the cross
    // axis unbounded. Give the card a finite preferred width; a parent with a
    // smaller width still clamps this through ConstrainedBox.
    .with_width(600.)
    .finish()
}

#[cfg(all(test, feature = "local_fs"))]
#[path = "custom_router_view_tests.rs"]
mod tests;

/// Actions handled by the local custom-router manager and its inline editor.
/// These actions never leave the settings view and are intentionally separate
/// from the hosted model/settings action surface.
#[derive(Clone, Debug, PartialEq)]
pub enum CustomRouterManagerAction {
    Add,
    Edit(PathBuf),
    AskDelete(PathBuf),
    ConfirmDelete,
    CancelDelete,
    OpenErrorFile(PathBuf),
    AskDeleteError(PathBuf),
    ConfirmDeleteError,
    CancelDeleteError,
    SetRouterType(RouterEditorType),
    SetComplexityDefault(String),
    SetComplexityEasy(String),
    SetComplexityMedium(String),
    SetComplexityHard(String),
    SetPromptDefault(String),
    SetPromptRuleModel { index: usize, model_id: String },
    AddPromptRule,
    RemovePromptRule(usize),
    MovePromptRuleUp(usize),
    MovePromptRuleDown(usize),
    Save,
    CloseEditor,
    KeepEditing,
    DiscardChanges,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RouterEditorType {
    Complexity,
    Prompt,
}

struct RouterRowHandles {
    edit_button: ViewHandle<ActionButton>,
    delete_button: ViewHandle<ActionButton>,
}

struct PendingDelete {
    path: PathBuf,
    revision: RouterFileRevision,
    display_name: String,
    confirm_button: ViewHandle<ActionButton>,
    cancel_button: ViewHandle<ActionButton>,
}

struct PendingErrorDelete {
    path: PathBuf,
    confirm_button: ViewHandle<ActionButton>,
    cancel_button: ViewHandle<ActionButton>,
}

struct PromptRuleRow {
    description_editor: ViewHandle<EditorView>,
    model_dropdown: ViewHandle<FilterableDropdown<CustomRouterManagerAction>>,
    current_model: String,
    move_up_button: ViewHandle<ActionButton>,
    move_down_button: ViewHandle<ActionButton>,
    remove_button: ViewHandle<ActionButton>,
}

struct RouterEditorState {
    path: Option<PathBuf>,
    revision: Option<RouterFileRevision>,
    name_editor: ViewHandle<EditorView>,
    router_type: RouterEditorType,
    complexity_type_button: ViewHandle<ActionButton>,
    prompt_type_button: ViewHandle<ActionButton>,
    complexity_default_dropdown: ViewHandle<FilterableDropdown<CustomRouterManagerAction>>,
    complexity_easy_dropdown: ViewHandle<FilterableDropdown<CustomRouterManagerAction>>,
    complexity_medium_dropdown: ViewHandle<FilterableDropdown<CustomRouterManagerAction>>,
    complexity_hard_dropdown: ViewHandle<FilterableDropdown<CustomRouterManagerAction>>,
    complexity_default: String,
    complexity_easy: Option<String>,
    complexity_medium: Option<String>,
    complexity_hard: Option<String>,
    prompt_default_dropdown: ViewHandle<FilterableDropdown<CustomRouterManagerAction>>,
    prompt_default_model: String,
    prompt_rules: Vec<PromptRuleRow>,
    save_button: ViewHandle<ActionButton>,
    cancel_button: ViewHandle<ActionButton>,
    add_rule_button: ViewHandle<ActionButton>,
    keep_editing_button: ViewHandle<ActionButton>,
    discard_changes_button: ViewHandle<ActionButton>,
    dirty: bool,
    close_confirmation: bool,
    save_error: Option<String>,
}

pub struct CustomRouterManagerView {
    routers: Vec<CustomModelRouter>,
    errors: Vec<ModelConfigError>,
    rows: Vec<RouterRowHandles>,
    error_rows: Vec<RouterErrorHandles>,
    add_button: ViewHandle<ActionButton>,
    editor: Option<RouterEditorState>,
    pending_delete: Option<PendingDelete>,
    pending_error_delete: Option<PendingErrorDelete>,
    operation_error: Option<String>,
}

impl CustomRouterManagerView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let add_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Add router", SecondaryTheme)
                .with_icon(ActionIcon::Plus)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(CustomRouterManagerAction::Add))
        });
        let mut view = Self {
            routers: Vec::new(),
            errors: Vec::new(),
            rows: Vec::new(),
            error_rows: Vec::new(),
            add_button,
            editor: None,
            pending_delete: None,
            pending_error_delete: None,
            operation_error: None,
        };
        view.reload(ctx);
        ctx.subscribe_to_model(&WarpConfig::handle(ctx), |me, _, event, ctx| {
            // ModelConfigs contains the complete atomically reloaded snapshot.
            // Ignore the follow-up error event so a watcher change refreshes
            // the effective list and active-selection reconciliation once.
            if matches!(event, WarpConfigUpdateEvent::ModelConfigs) {
                me.reload(ctx);
            }
        });
        ctx.subscribe_to_model(&LLMPreferences::handle(ctx), |me, _, event, ctx| {
            if matches!(
                event,
                crate::ai::llms::LLMPreferencesEvent::UpdatedAvailableLLMs
            ) {
                me.refresh_editor_models(ctx);
            }
        });
        view
    }

    fn reload(&mut self, ctx: &mut ViewContext<Self>) {
        let config = WarpConfig::as_ref(ctx);
        self.routers = config.custom_model_routers().to_vec();
        self.errors = config.custom_model_router_errors().to_vec();
        self.rows = self
            .routers
            .iter()
            .filter_map(|router| router.source_path.clone())
            .map(|path| self.make_row_handles(path, ctx))
            .collect();
        self.error_rows = self
            .errors
            .iter()
            .map(|error| self.make_error_row_handles(&error.file_path, ctx))
            .collect();
        if let Some(pending) = &self.pending_delete
            && !self
                .routers
                .iter()
                .any(|router| router.source_path.as_ref() == Some(&pending.path))
        {
            self.pending_delete = None;
        }
        if let Some(pending) = &self.pending_error_delete
            && !self
                .errors
                .iter()
                .any(|error| error.file_path == pending.path)
        {
            self.pending_error_delete = None;
        }
        self.refresh_editor_models(ctx);
        ctx.notify();
    }

    fn make_row_handles(&self, path: PathBuf, ctx: &mut ViewContext<Self>) -> RouterRowHandles {
        let edit_path = path.clone();
        let delete_path = path.clone();
        let edit_button = ctx.add_typed_action_view(move |_| {
            ActionButton::new("Edit", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::Edit(edit_path.clone()));
                })
        });
        let delete_button = ctx.add_typed_action_view(move |_| {
            ActionButton::new("Delete", DangerNakedTheme)
                .with_icon(ActionIcon::Trash)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::AskDelete(
                        delete_path.clone(),
                    ));
                })
        });
        RouterRowHandles {
            edit_button,
            delete_button,
        }
    }

    fn make_error_row_handles(
        &self,
        path: &PathBuf,
        ctx: &mut ViewContext<Self>,
    ) -> RouterErrorHandles {
        let open_path = path.clone();
        let delete_path = path.clone();
        let open_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Open file", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::OpenErrorFile(
                        open_path.clone(),
                    ));
                })
        });
        let delete_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Delete and fix", DangerNakedTheme)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::AskDeleteError(
                        delete_path.clone(),
                    ));
                })
        });
        RouterErrorHandles {
            open_button,
            delete_button,
        }
    }

    fn open_editor(&mut self, requested_path: Option<PathBuf>, ctx: &mut ViewContext<Self>) {
        let (existing, path, revision) = match requested_path {
            Some(path) => {
                let repository = LocalCustomModelRouterRepository::new(custom_model_routers_dir());
                match repository.read(&path) {
                    Ok(stored) => (
                        Some(stored.router),
                        Some(stored.path),
                        Some(stored.revision),
                    ),
                    Err(error) => {
                        self.operation_error = Some(format!(
                            "Could not open router file {}: {error}",
                            path.display()
                        ));
                        ctx.notify();
                        return;
                    }
                }
            }
            None => (None, None, None),
        };
        self.operation_error = None;
        self.pending_delete = None;
        self.editor = Some(RouterEditorState::new(existing, path, revision, ctx));
        if let Some(editor) = self.editor.as_ref() {
            update_type_buttons(editor, ctx);
        }
        ctx.notify();
    }

    fn ask_delete(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
        let repository = LocalCustomModelRouterRepository::new(custom_model_routers_dir());
        let stored = match repository.read(&path) {
            Ok(stored) => stored,
            Err(error) => {
                self.operation_error = Some(format!(
                    "Could not validate router file {}: {error}",
                    path.display()
                ));
                ctx.notify();
                return;
            }
        };
        let confirm_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Delete router", DangerNakedTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(CustomRouterManagerAction::ConfirmDelete))
        });
        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(CustomRouterManagerAction::CancelDelete))
        });
        self.pending_delete = Some(PendingDelete {
            path: stored.path,
            revision: stored.revision,
            display_name: stored.router.info.display_name,
            confirm_button,
            cancel_button,
        });
        self.operation_error = None;
        ctx.notify();
    }

    fn confirm_delete(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };
        let repository = LocalCustomModelRouterRepository::new(custom_model_routers_dir());
        match repository.delete_checked(&pending.path, &pending.revision) {
            Ok(()) => {
                self.operation_error = None;
                // The filesystem watcher is the single source of effective
                // refresh/reconciliation. Keep the stale snapshot until it
                // reports the committed delete.
            }
            Err(error) => {
                self.operation_error = Some(format!(
                    "Could not delete {}: {error}",
                    pending.path.display()
                ));
            }
        }
        ctx.notify();
    }

    fn open_error_file(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
        let repository = LocalCustomModelRouterRepository::new(custom_model_routers_dir());
        match repository.validate_managed_path(&path) {
            Ok(path) => {
                ctx.dispatch_global_action("root_view:open_new_with_file_notebook", path);
                self.operation_error = None;
            }
            Err(error) => {
                self.operation_error = Some(format!(
                    "Could not open router file {}: {error}",
                    path.display()
                ));
            }
        }
        ctx.notify();
    }

    fn ask_delete_error(&mut self, path: PathBuf, ctx: &mut ViewContext<Self>) {
        let repository = LocalCustomModelRouterRepository::new(custom_model_routers_dir());
        let path = match repository.validate_managed_path(&path) {
            Ok(path) => path,
            Err(error) => {
                self.operation_error = Some(format!(
                    "Could not validate malformed router file {}: {error}",
                    path.display()
                ));
                ctx.notify();
                return;
            }
        };
        let confirm_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Delete file", DangerNakedTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::ConfirmDeleteError)
                })
        });
        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::CancelDeleteError)
                })
        });
        self.pending_error_delete = Some(PendingErrorDelete {
            path,
            confirm_button,
            cancel_button,
        });
        self.operation_error = None;
        ctx.notify();
    }

    fn confirm_delete_error(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(pending) = self.pending_error_delete.take() else {
            return;
        };
        let repository = LocalCustomModelRouterRepository::new(custom_model_routers_dir());
        if let Err(error) = repository.delete_invalid(&pending.path) {
            self.operation_error = Some(format!(
                "Could not delete malformed router {}: {error}",
                pending.path.display()
            ));
            self.pending_error_delete = Some(pending);
        } else {
            self.operation_error = None;
        }
        ctx.notify();
    }

    fn request_close(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.dirty {
            editor.close_confirmation = true;
            ctx.notify();
        } else {
            self.editor = None;
            ctx.notify();
        }
    }

    fn refresh_editor_models(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let providers = AISettings::as_ref(ctx).custom_providers.clone();
        set_model_dropdown_items(
            &editor.complexity_default_dropdown,
            &providers,
            &editor.complexity_default,
            CustomRouterManagerAction::SetComplexityDefault,
            ctx,
        );
        set_model_dropdown_items(
            &editor.complexity_easy_dropdown,
            &providers,
            editor.complexity_easy.as_deref().unwrap_or_default(),
            CustomRouterManagerAction::SetComplexityEasy,
            ctx,
        );
        set_model_dropdown_items(
            &editor.complexity_medium_dropdown,
            &providers,
            editor.complexity_medium.as_deref().unwrap_or_default(),
            CustomRouterManagerAction::SetComplexityMedium,
            ctx,
        );
        set_model_dropdown_items(
            &editor.complexity_hard_dropdown,
            &providers,
            editor.complexity_hard.as_deref().unwrap_or_default(),
            CustomRouterManagerAction::SetComplexityHard,
            ctx,
        );
        set_model_dropdown_items(
            &editor.prompt_default_dropdown,
            &providers,
            &editor.prompt_default_model,
            CustomRouterManagerAction::SetPromptDefault,
            ctx,
        );
        for (index, row) in editor.prompt_rules.iter_mut().enumerate() {
            set_model_dropdown_items(
                &row.model_dropdown,
                &providers,
                &row.current_model,
                move |model_id| CustomRouterManagerAction::SetPromptRuleModel { index, model_id },
                ctx,
            );
        }
        ctx.notify();
    }

    fn save_editor(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(editor) = self.editor.as_ref() else {
            return;
        };
        let router = match router_from_editor(editor, ctx) {
            Ok(router) => router,
            Err(error) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.save_error = Some(error);
                }
                ctx.notify();
                return;
            }
        };
        let providers = AISettings::as_ref(ctx).custom_providers.clone();
        if let Err(error) = router_catalog_entry(&router, &providers) {
            if let Some(editor) = self.editor.as_mut() {
                editor.save_error = Some(format!(
                    "Save blocked locally: {error}. Configure a concrete custom provider model before saving."
                ));
            }
            ctx.notify();
            return;
        }
        let repository = LocalCustomModelRouterRepository::new(custom_model_routers_dir());
        let result = match self.editor.as_ref().and_then(|editor| editor.path.as_ref()) {
            Some(path) => {
                let Some(expected) = self
                    .editor
                    .as_ref()
                    .and_then(|editor| editor.revision.as_ref())
                else {
                    if let Some(editor) = self.editor.as_mut() {
                        editor.save_error = Some(
                            "Save blocked: the original router revision is unavailable. Reopen the file and try again."
                                .to_owned(),
                        );
                    }
                    ctx.notify();
                    return;
                };
                repository.update(path, expected, &router)
            }
            None => repository.create(
                router_file_name(&router.info.display_name, &repository),
                &router,
            ),
        };
        match result {
            Ok(_) => {
                self.editor = None;
                self.operation_error = None;
                // The watcher reload is the single effective refresh path.
            }
            Err(error) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.save_error = Some(save_error_message(error));
                }
            }
        }
        ctx.notify();
    }

    fn add_prompt_rule(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        let index = editor.prompt_rules.len();
        let row = PromptRuleRow::new(index, "", "", ctx);
        subscribe_editor(&row.description_editor, ctx);
        editor.prompt_rules.push(row);
        editor.dirty = true;
        ctx.notify();
    }

    fn remove_prompt_rule(&mut self, index: usize, ctx: &mut ViewContext<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if editor.prompt_rules.len() <= 1 || index >= editor.prompt_rules.len() {
            return;
        }
        let values = editor
            .prompt_rules
            .iter()
            .enumerate()
            .filter(|(row_index, _)| *row_index != index)
            .map(|(_, row)| {
                (
                    row.description_editor.as_ref(ctx).buffer_text(ctx),
                    row.current_model.clone(),
                )
            })
            .collect::<Vec<_>>();
        editor.prompt_rules = values
            .into_iter()
            .enumerate()
            .map(|(index, (description, model))| {
                let row = PromptRuleRow::new(index, &description, &model, ctx);
                subscribe_editor(&row.description_editor, ctx);
                row
            })
            .collect();
        editor.dirty = true;
        self.refresh_editor_models(ctx);
        ctx.notify();
    }

    fn move_prompt_rule(&mut self, index: usize, target: usize, ctx: &mut ViewContext<Self>) {
        let Some(editor) = self.editor.as_mut() else {
            return;
        };
        if index >= editor.prompt_rules.len() || target >= editor.prompt_rules.len() {
            return;
        }
        let mut values = editor
            .prompt_rules
            .iter()
            .map(|row| {
                (
                    row.description_editor.as_ref(ctx).buffer_text(ctx),
                    row.current_model.clone(),
                )
            })
            .collect::<Vec<_>>();
        values.swap(index, target);
        editor.prompt_rules = values
            .into_iter()
            .enumerate()
            .map(|(index, (description, model))| {
                let row = PromptRuleRow::new(index, &description, &model, ctx);
                subscribe_editor(&row.description_editor, ctx);
                row
            })
            .collect();
        editor.dirty = true;
        self.refresh_editor_models(ctx);
        ctx.notify();
    }

    fn handle_action(&mut self, action: &CustomRouterManagerAction, ctx: &mut ViewContext<Self>) {
        match action {
            CustomRouterManagerAction::Add => self.open_editor(None, ctx),
            CustomRouterManagerAction::Edit(path) => self.open_editor(Some(path.clone()), ctx),
            CustomRouterManagerAction::AskDelete(path) => self.ask_delete(path.clone(), ctx),
            CustomRouterManagerAction::ConfirmDelete => self.confirm_delete(ctx),
            CustomRouterManagerAction::CancelDelete => {
                self.pending_delete = None;
                ctx.notify();
            }
            CustomRouterManagerAction::OpenErrorFile(path) => {
                self.open_error_file(path.clone(), ctx)
            }
            CustomRouterManagerAction::AskDeleteError(path) => {
                self.ask_delete_error(path.clone(), ctx)
            }
            CustomRouterManagerAction::ConfirmDeleteError => self.confirm_delete_error(ctx),
            CustomRouterManagerAction::CancelDeleteError => {
                self.pending_error_delete = None;
                ctx.notify();
            }
            CustomRouterManagerAction::SetRouterType(router_type) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.router_type = *router_type;
                    if *router_type == RouterEditorType::Prompt && editor.prompt_rules.is_empty() {
                        let row = PromptRuleRow::new(0, "", "", ctx);
                        subscribe_editor(&row.description_editor, ctx);
                        editor.prompt_rules.push(row);
                    }
                    editor.dirty = true;
                    update_type_buttons(editor, ctx);
                    ctx.notify();
                }
            }
            CustomRouterManagerAction::SetComplexityDefault(model) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.complexity_default = model.clone();
                    editor.dirty = true;
                    ctx.notify();
                }
            }
            CustomRouterManagerAction::SetComplexityEasy(model) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.complexity_easy = (!model.is_empty()).then_some(model.clone());
                    editor.dirty = true;
                    ctx.notify();
                }
            }
            CustomRouterManagerAction::SetComplexityMedium(model) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.complexity_medium = (!model.is_empty()).then_some(model.clone());
                    editor.dirty = true;
                    ctx.notify();
                }
            }
            CustomRouterManagerAction::SetComplexityHard(model) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.complexity_hard = (!model.is_empty()).then_some(model.clone());
                    editor.dirty = true;
                    ctx.notify();
                }
            }
            CustomRouterManagerAction::SetPromptDefault(model) => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.prompt_default_model = model.clone();
                    editor.dirty = true;
                    ctx.notify();
                }
            }
            CustomRouterManagerAction::SetPromptRuleModel { index, model_id } => {
                if let Some(editor) = self.editor.as_mut()
                    && let Some(row) = editor.prompt_rules.get_mut(*index)
                {
                    row.current_model = model_id.clone();
                    editor.dirty = true;
                    ctx.notify();
                }
            }
            CustomRouterManagerAction::AddPromptRule => self.add_prompt_rule(ctx),
            CustomRouterManagerAction::RemovePromptRule(index) => {
                self.remove_prompt_rule(*index, ctx)
            }
            CustomRouterManagerAction::MovePromptRuleUp(index) => {
                if *index > 0 {
                    self.move_prompt_rule(*index, *index - 1, ctx);
                }
            }
            CustomRouterManagerAction::MovePromptRuleDown(index) => {
                self.move_prompt_rule(*index, *index + 1, ctx)
            }
            CustomRouterManagerAction::Save => self.save_editor(ctx),
            CustomRouterManagerAction::CloseEditor => self.request_close(ctx),
            CustomRouterManagerAction::KeepEditing => {
                if let Some(editor) = self.editor.as_mut() {
                    editor.close_confirmation = false;
                    ctx.notify();
                }
            }
            CustomRouterManagerAction::DiscardChanges => {
                self.editor = None;
                ctx.notify();
            }
        }
    }
}

impl RouterEditorState {
    fn new(
        existing: Option<CustomModelRouter>,
        path: Option<PathBuf>,
        revision: Option<RouterFileRevision>,
        ctx: &mut ViewContext<CustomRouterManagerView>,
    ) -> Self {
        let providers = AISettings::as_ref(ctx).custom_providers.clone();
        let first_model = concrete_custom_model_ids(&providers)
            .into_iter()
            .next()
            .unwrap_or_default();
        let router_type = match existing.as_ref().map(|router| &router.routing) {
            Some(CustomModelRouting::Prompt(_)) => RouterEditorType::Prompt,
            _ => RouterEditorType::Complexity,
        };
        let (complexity_default, complexity_easy, complexity_medium, complexity_hard) =
            match existing.as_ref().map(|router| &router.routing) {
                Some(CustomModelRouting::Complexity(routing)) => (
                    routing.default.clone(),
                    routing.easy.clone(),
                    routing.medium.clone(),
                    routing.hard.clone(),
                ),
                _ => (first_model.clone(), None, None, None),
            };
        let (prompt_default_model, initial_rules) =
            match existing.as_ref().map(|router| &router.routing) {
                Some(CustomModelRouting::Prompt(routing)) => {
                    (routing.default_model.clone(), routing.rules.clone())
                }
                _ => (first_model, Vec::new()),
            };
        let initial_name = existing
            .as_ref()
            .map(|router| router.info.display_name.clone())
            .unwrap_or_default();
        let name_editor = make_text_editor(&initial_name, "My custom router", ctx);

        let complexity_type_button =
            make_type_button("Complexity", RouterEditorType::Complexity, ctx);
        let prompt_type_button = make_type_button("Rules", RouterEditorType::Prompt, ctx);

        let complexity_default_dropdown = make_model_dropdown(
            &complexity_default,
            CustomRouterManagerAction::SetComplexityDefault,
            ctx,
        );
        let complexity_easy_dropdown = make_model_dropdown(
            complexity_easy.as_deref().unwrap_or_default(),
            CustomRouterManagerAction::SetComplexityEasy,
            ctx,
        );
        let complexity_medium_dropdown = make_model_dropdown(
            complexity_medium.as_deref().unwrap_or_default(),
            CustomRouterManagerAction::SetComplexityMedium,
            ctx,
        );
        let complexity_hard_dropdown = make_model_dropdown(
            complexity_hard.as_deref().unwrap_or_default(),
            CustomRouterManagerAction::SetComplexityHard,
            ctx,
        );
        let prompt_default_dropdown = make_model_dropdown(
            &prompt_default_model,
            CustomRouterManagerAction::SetPromptDefault,
            ctx,
        );

        let mut prompt_rules = initial_rules
            .iter()
            .enumerate()
            .map(|(index, rule)| PromptRuleRow::new(index, &rule.description, &rule.model, ctx))
            .collect::<Vec<_>>();
        if prompt_rules.is_empty() {
            prompt_rules.push(PromptRuleRow::new(0, "", "", ctx));
        }

        let save_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Save", PrimaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(CustomRouterManagerAction::Save))
        });
        let cancel_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(CustomRouterManagerAction::CloseEditor))
        });
        let add_rule_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Add rule", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(CustomRouterManagerAction::AddPromptRule))
        });
        let keep_editing_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Keep editing", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(CustomRouterManagerAction::KeepEditing))
        });
        let discard_changes_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Discard changes", DangerNakedTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::DiscardChanges)
                })
        });

        let state = Self {
            path,
            revision,
            name_editor,
            router_type,
            complexity_type_button,
            prompt_type_button,
            complexity_default_dropdown,
            complexity_easy_dropdown,
            complexity_medium_dropdown,
            complexity_hard_dropdown,
            complexity_default,
            complexity_easy,
            complexity_medium,
            complexity_hard,
            prompt_default_dropdown,
            prompt_default_model,
            prompt_rules,
            save_button,
            cancel_button,
            add_rule_button,
            keep_editing_button,
            discard_changes_button,
            dirty: false,
            close_confirmation: false,
            save_error: None,
        };
        subscribe_editor(&state.name_editor, ctx);
        for row in &state.prompt_rules {
            subscribe_editor(&row.description_editor, ctx);
        }
        state
    }
}

impl PromptRuleRow {
    fn new(
        index: usize,
        description: &str,
        model: &str,
        ctx: &mut ViewContext<CustomRouterManagerView>,
    ) -> Self {
        let description_editor =
            make_text_editor(description, "Describe when to use this model", ctx);
        let model_dropdown = make_model_dropdown(
            model,
            move |model_id| CustomRouterManagerAction::SetPromptRuleModel { index, model_id },
            ctx,
        );
        let move_up_button = ctx.add_typed_action_view(move |_| {
            ActionButton::new("↑", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .with_tooltip("Move rule up")
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::MovePromptRuleUp(index))
                })
        });
        let move_down_button = ctx.add_typed_action_view(move |_| {
            ActionButton::new("↓", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .with_tooltip("Move rule down")
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::MovePromptRuleDown(index))
                })
        });
        let remove_button = ctx.add_typed_action_view(move |_| {
            ActionButton::new("Remove", DangerNakedTheme)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(CustomRouterManagerAction::RemovePromptRule(index))
                })
        });
        Self {
            description_editor,
            model_dropdown,
            current_model: model.to_owned(),
            move_up_button,
            move_down_button,
            remove_button,
        }
    }
}

fn make_text_editor(
    initial: &str,
    placeholder: &'static str,
    ctx: &mut ViewContext<CustomRouterManagerView>,
) -> ViewHandle<EditorView> {
    let initial = initial.to_owned();
    ctx.add_typed_action_view(move |ctx| {
        let appearance = Appearance::as_ref(ctx);
        let mut editor = EditorView::single_line(
            SingleLineEditorOptions {
                text: TextOptions::ui_text(Some(appearance.ui_font_size()), appearance),
                ..Default::default()
            },
            ctx,
        );
        editor.set_placeholder_text(placeholder, ctx);
        editor.set_buffer_text(&initial, ctx);
        editor
    })
}

fn make_type_button(
    label: &'static str,
    router_type: RouterEditorType,
    ctx: &mut ViewContext<CustomRouterManagerView>,
) -> ViewHandle<ActionButton> {
    ctx.add_typed_action_view(move |_| {
        ActionButton::new(label, SecondaryTheme)
            .with_size(ButtonSize::Small)
            .on_click(move |ctx| {
                ctx.dispatch_typed_action(CustomRouterManagerAction::SetRouterType(router_type))
            })
    })
}

fn make_model_dropdown<F>(
    selected: &str,
    make_action: F,
    ctx: &mut ViewContext<CustomRouterManagerView>,
) -> ViewHandle<FilterableDropdown<CustomRouterManagerAction>>
where
    F: Fn(String) -> CustomRouterManagerAction + Clone + 'static,
{
    let selected = selected.to_owned();
    ctx.add_typed_action_view(move |ctx| {
        let mut dropdown = FilterableDropdown::new(ctx);
        dropdown.set_menu_width(340., ctx);
        let providers = AISettings::as_ref(ctx).custom_providers.clone();
        set_model_dropdown_items_inner(
            &mut dropdown,
            &providers,
            &selected,
            make_action.clone(),
            ctx,
        );
        dropdown
    })
}

fn set_model_dropdown_items<F>(
    dropdown: &ViewHandle<FilterableDropdown<CustomRouterManagerAction>>,
    providers: &[crate::settings::CustomProviderConfig],
    selected: &str,
    make_action: F,
    ctx: &mut ViewContext<CustomRouterManagerView>,
) where
    F: Fn(String) -> CustomRouterManagerAction + Clone + 'static,
{
    let providers = providers.to_owned();
    let selected = selected.to_owned();
    dropdown.update(ctx, move |dropdown, ctx| {
        set_model_dropdown_items_inner(dropdown, &providers, &selected, make_action, ctx);
    });
}

fn set_model_dropdown_items_inner<F>(
    dropdown: &mut FilterableDropdown<CustomRouterManagerAction>,
    providers: &[crate::settings::CustomProviderConfig],
    selected: &str,
    make_action: F,
    ctx: &mut ViewContext<FilterableDropdown<CustomRouterManagerAction>>,
) where
    F: Fn(String) -> CustomRouterManagerAction + Clone + 'static,
{
    let ids = concrete_custom_model_ids(providers);
    let items = ids
        .iter()
        .map(|id| DropdownItem::new(model_display_label(id, providers), make_action(id.clone())))
        .collect();
    dropdown.set_items(items, ctx);
    if ids.iter().any(|id| id == selected) {
        dropdown.set_selected_by_action(make_action(selected.to_owned()), ctx);
    }
}

fn model_display_label(
    model_id: &str,
    providers: &[crate::settings::CustomProviderConfig],
) -> String {
    let Some(rest) = model_id.strip_prefix("custom/") else {
        return model_id.to_owned();
    };
    let Some((provider_name, model)) = rest.split_once('/') else {
        return model_id.to_owned();
    };
    let Some(provider) = providers
        .iter()
        .find(|provider| provider.name == provider_name)
    else {
        return format!("{model_id} · unavailable");
    };
    let capabilities = effective_capabilities_for_config(&provider.capabilities);
    let mut tags = Vec::new();
    if capabilities.chat {
        tags.push("chat");
    }
    if capabilities.tools {
        tags.push("tools");
    }
    if capabilities.vision {
        tags.push("vision");
    }
    format!("{provider_name} / {model} · {} · ready", tags.join(", "))
}

fn subscribe_editor(
    editor: &ViewHandle<EditorView>,
    ctx: &mut ViewContext<CustomRouterManagerView>,
) {
    ctx.subscribe_to_view(editor, |me, _, event, ctx| match event {
        EditorEvent::Edited(_) | EditorEvent::BufferReplaced | EditorEvent::BufferReinitialized => {
            if let Some(editor) = me.editor.as_mut() {
                editor.dirty = true;
                ctx.notify();
            }
        }
        EditorEvent::Escape => me.request_close(ctx),
        _ => {}
    });
}

fn update_type_buttons(editor: &RouterEditorState, ctx: &mut ViewContext<CustomRouterManagerView>) {
    editor.complexity_type_button.update(ctx, |button, ctx| {
        if editor.router_type == RouterEditorType::Complexity {
            button.set_theme(PrimaryTheme, ctx);
        } else {
            button.set_theme(SecondaryTheme, ctx);
        }
    });
    editor.prompt_type_button.update(ctx, |button, ctx| {
        if editor.router_type == RouterEditorType::Prompt {
            button.set_theme(PrimaryTheme, ctx);
        } else {
            button.set_theme(SecondaryTheme, ctx);
        }
    });
}

fn router_from_editor(
    editor: &RouterEditorState,
    app: &AppContext,
) -> Result<CustomModelRouter, String> {
    let name = editor
        .name_editor
        .as_ref(app)
        .buffer_text(app)
        .trim()
        .to_owned();
    if name.is_empty() {
        return Err("Router name is required.".to_owned());
    }
    let routing = match editor.router_type {
        RouterEditorType::Complexity => CustomModelRouting::Complexity(ComplexityRouting {
            default: editor.complexity_default.trim().to_owned(),
            easy: editor
                .complexity_easy
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            medium: editor
                .complexity_medium
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            hard: editor
                .complexity_hard
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
        }),
        RouterEditorType::Prompt => CustomModelRouting::Prompt(PromptRouting {
            default_model: editor.prompt_default_model.trim().to_owned(),
            rules: editor
                .prompt_rules
                .iter()
                .filter_map(|row| {
                    let description = row
                        .description_editor
                        .as_ref(app)
                        .buffer_text(app)
                        .trim()
                        .to_owned();
                    let model = row.current_model.trim().to_owned();
                    if description.is_empty() || model.is_empty() {
                        None
                    } else {
                        Some(PromptRule::new(description, model))
                    }
                })
                .collect(),
        }),
    };
    let router = CustomModelRouter::new_local(name, routing, editor.path.as_deref());
    router.validate()?;
    Ok(router)
}

fn router_file_name(name: &str, repository: &LocalCustomModelRouterRepository) -> String {
    let mut stem = name
        .chars()
        .flat_map(char::to_lowercase)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    stem = stem.trim_matches('_').to_owned();
    if stem.is_empty() {
        stem = "router".to_owned();
    }
    let candidate = format!("{stem}.yaml");
    if !repository.directory().join(&candidate).exists() {
        return candidate;
    }
    for suffix in 2..=u32::MAX {
        let candidate = format!("{stem}_{suffix}.yaml");
        if !repository.directory().join(&candidate).exists() {
            return candidate;
        }
    }
    // The bounded loop above cannot realistically exhaust; keep a safe
    // deterministic fallback for static analysis and corrupted filesystems.
    "router.yaml".to_owned()
}

fn save_error_message(error: LocalCustomModelRouterRepositoryError) -> String {
    match error {
        LocalCustomModelRouterRepositoryError::Conflict { path, .. } => format!(
            "Save conflict for {}: the file changed outside this editor. Your draft is retained; reopen it to merge the external change.",
            path.display()
        ),
        LocalCustomModelRouterRepositoryError::Parse { path, message } => format!(
            "The router file {} is no longer valid YAML: {message}. Your draft is retained.",
            path.display()
        ),
        other => format!("Could not save router: {other}. Your draft is retained."),
    }
}

impl Entity for CustomRouterManagerView {
    type Event = ();
}

impl View for CustomRouterManagerView {
    fn ui_name() -> &'static str {
        "CustomRouterManagerView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let mut content = Flex::column().with_spacing(12.);
        if let Some(editor) = &self.editor {
            content.add_child(render_editor(editor, appearance, app));
        } else {
            content.add_child(render_router_list(self, appearance, app));
        }
        Container::new(content.finish())
            .with_margin_top(8.)
            .with_margin_bottom(8.)
            .finish()
    }
}

impl TypedActionView for CustomRouterManagerView {
    type Action = CustomRouterManagerAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        self.handle_action(action, ctx);
    }
}

fn render_router_list(
    manager: &CustomRouterManagerView,
    appearance: &Appearance,
    app: &AppContext,
) -> Box<dyn Element> {
    let mut content = Flex::column().with_spacing(10.);
    let header = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Expanded::new(
                1.,
                Text::new("Custom model routers", appearance.ui_font_family(), 16.)
                    .with_style(Properties::default().weight(Weight::Semibold))
                    .with_color(appearance.theme().active_ui_text_color().into())
                    .finish(),
            )
            .finish(),
        )
        .with_child(ChildView::new(&manager.add_button).finish())
        .finish();
    content.add_child(header);
    content.add_child(
        Text::new(
            "Choose a deterministic local router over configured custom provider models. No hosted models or classifiers are available here.",
            appearance.ui_font_family(),
            12.,
        )
        .with_color(
            appearance
                .theme()
                .sub_text_color(appearance.theme().surface_1())
                .into(),
        )
        .soft_wrap(true)
        .finish(),
    );
    if let Some(error) = &manager.operation_error {
        content.add_child(error_text(error, appearance));
    }
    for (index, error) in manager.errors.iter().enumerate() {
        let Some(handles) = manager.error_rows.get(index) else {
            continue;
        };
        content.add_child(render_router_error_card_with_actions(
            error.file_name.clone(),
            error.error_message.clone(),
            Some(handles),
            appearance,
        ));
    }
    let providers = AISettings::as_ref(app).custom_providers.clone();
    if manager.routers.is_empty() && manager.errors.is_empty() {
        content.add_child(
            Text::new(
                "No local routers configured.",
                appearance.ui_font_family(),
                12.,
            )
            .with_color(
                appearance
                    .theme()
                    .sub_text_color(appearance.theme().surface_1())
                    .into(),
            )
            .finish(),
        );
    }
    for (index, router) in manager.routers.iter().enumerate() {
        let Some(row) = manager.rows.get(index) else {
            continue;
        };
        content.add_child(render_router_row(router, row, &providers, appearance));
    }
    if let Some(pending) = &manager.pending_delete {
        content.add_child(render_delete_confirmation(pending, appearance));
    }
    if let Some(pending) = &manager.pending_error_delete {
        content.add_child(render_error_delete_confirmation(pending, appearance));
    }
    content.finish()
}

fn render_router_row(
    router: &CustomModelRouter,
    row: &RouterRowHandles,
    providers: &[crate::settings::CustomProviderConfig],
    appearance: &Appearance,
) -> Box<dyn Element> {
    let status = router_catalog_entry(router, providers)
        .err()
        .map(|error| format!("Unavailable: {error}"));
    let mut details = Flex::column().with_spacing(4.);
    details.add_child(
        Text::new(
            router.info.display_name.clone(),
            appearance.ui_font_family(),
            14.,
        )
        .with_style(Properties::default().weight(Weight::Medium))
        .with_color(appearance.theme().active_ui_text_color().into())
        .finish(),
    );
    let path = router
        .source_path
        .as_deref()
        .map(warp_core::paths::home_relative_path)
        .unwrap_or_else(|| "local file not found".to_owned());
    details.add_child(
        Text::new(path, appearance.ui_font_family(), 11.)
            .with_color(
                appearance
                    .theme()
                    .sub_text_color(appearance.theme().surface_1())
                    .into(),
            )
            .finish(),
    );
    if let Some(status) = status {
        details.add_child(error_text(&status, appearance));
    } else {
        let model_count = router.all_targets().len();
        details.add_child(
            Text::new(
                format!(
                    "{} · {} configured concrete target{}",
                    routing_type_label(&router.routing),
                    model_count,
                    if model_count == 1 { "" } else { "s" }
                ),
                appearance.ui_font_family(),
                11.,
            )
            .with_color(
                appearance
                    .theme()
                    .sub_text_color(appearance.theme().surface_1())
                    .into(),
            )
            .finish(),
        );
    }
    let actions = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(ChildView::new(&row.edit_button).finish())
        .with_child(
            Container::new(ChildView::new(&row.delete_button).finish())
                .with_margin_left(8.)
                .finish(),
        )
        .finish();
    Container::new(
        Flex::row()
            .with_main_axis_size(MainAxisSize::Max)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(Expanded::new(1., details.finish()).finish())
            .with_child(actions)
            .finish(),
    )
    .with_uniform_padding(12.)
    .with_background(appearance.theme().surface_1())
    .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .finish()
}

fn render_delete_confirmation(
    pending: &PendingDelete,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_spacing(8.)
            .with_child(
                Text::new(
                    format!(
                        "Delete router {:?}? This removes only {}.",
                        pending.display_name,
                        warp_core::paths::home_relative_path(&pending.path)
                    ),
                    appearance.ui_font_family(),
                    12.,
                )
                .with_color(appearance.theme().active_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            )
            .with_child(
                Flex::row()
                    .with_child(ChildView::new(&pending.confirm_button).finish())
                    .with_child(
                        Container::new(ChildView::new(&pending.cancel_button).finish())
                            .with_margin_left(8.)
                            .finish(),
                    )
                    .finish(),
            )
            .finish(),
    )
    .with_background(appearance.theme().surface_2())
    .with_border(Border::all(1.).with_border_fill(appearance.theme().ui_error_color()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .with_uniform_padding(12.)
    .finish()
}

fn render_error_delete_confirmation(
    pending: &PendingErrorDelete,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Container::new(
        Flex::column()
            .with_spacing(8.)
            .with_child(
                Text::new(
                    format!(
                        "Delete malformed router file? This removes only {}.",
                        warp_core::paths::home_relative_path(&pending.path)
                    ),
                    appearance.ui_font_family(),
                    12.,
                )
                .with_color(appearance.theme().active_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            )
            .with_child(
                Flex::row()
                    .with_child(ChildView::new(&pending.confirm_button).finish())
                    .with_child(
                        Container::new(ChildView::new(&pending.cancel_button).finish())
                            .with_margin_left(8.)
                            .finish(),
                    )
                    .finish(),
            )
            .finish(),
    )
    .with_background(appearance.theme().surface_2())
    .with_border(Border::all(1.).with_border_fill(appearance.theme().ui_error_color()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
    .with_uniform_padding(12.)
    .finish()
}

fn routing_type_label(routing: &CustomModelRouting) -> &'static str {
    match routing {
        CustomModelRouting::Complexity(_) => "Complexity",
        CustomModelRouting::Prompt(_) => "Ordered prompt rules",
    }
}

fn error_text(message: &str, appearance: &Appearance) -> Box<dyn Element> {
    Text::new(message.to_owned(), appearance.ui_font_family(), 12.)
        .with_color(appearance.theme().ui_error_color().into())
        .soft_wrap(true)
        .finish()
}

fn render_editor(
    editor: &RouterEditorState,
    appearance: &Appearance,
    app: &AppContext,
) -> Box<dyn Element> {
    let mut content = Flex::column().with_spacing(12.);
    let title = if editor.path.is_some() {
        "Edit custom model router"
    } else {
        "Add custom model router"
    };
    let dirty_suffix = if editor.dirty {
        " · unsaved changes"
    } else {
        ""
    };
    content.add_child(
        Text::new(
            format!("{title}{dirty_suffix}"),
            appearance.ui_font_family(),
            16.,
        )
        .with_style(Properties::default().weight(Weight::Semibold))
        .with_color(appearance.theme().active_ui_text_color().into())
        .finish(),
    );
    content.add_child(labeled_editor(
        "Display name",
        &editor.name_editor,
        appearance,
    ));
    content.add_child(
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(ChildView::new(&editor.complexity_type_button).finish())
            .with_child(
                Container::new(ChildView::new(&editor.prompt_type_button).finish())
                    .with_margin_left(8.)
                    .finish(),
            )
            .finish(),
    );
    match editor.router_type {
        RouterEditorType::Complexity => {
            content.add_child(labeled_model_dropdown(
                "Default model (required)",
                &editor.complexity_default_dropdown,
                &editor.complexity_default,
                appearance,
                app,
            ));
            content.add_child(labeled_model_dropdown(
                "Easy bucket (optional; falls back to default)",
                &editor.complexity_easy_dropdown,
                editor.complexity_easy.as_deref().unwrap_or_default(),
                appearance,
                app,
            ));
            content.add_child(labeled_model_dropdown(
                "Medium bucket (optional; falls back to default)",
                &editor.complexity_medium_dropdown,
                editor.complexity_medium.as_deref().unwrap_or_default(),
                appearance,
                app,
            ));
            content.add_child(labeled_model_dropdown(
                "Hard bucket (optional; falls back to default)",
                &editor.complexity_hard_dropdown,
                editor.complexity_hard.as_deref().unwrap_or_default(),
                appearance,
                app,
            ));
        }
        RouterEditorType::Prompt => {
            content.add_child(labeled_model_dropdown(
                "Default model (required)",
                &editor.prompt_default_dropdown,
                &editor.prompt_default_model,
                appearance,
                app,
            ));
            content.add_child(
                Text::new(
                    "Rules are evaluated in order; the first normalized token match wins.",
                    appearance.ui_font_family(),
                    11.,
                )
                .with_color(
                    appearance
                        .theme()
                        .sub_text_color(appearance.theme().surface_1())
                        .into(),
                )
                .soft_wrap(true)
                .finish(),
            );
            for (index, row) in editor.prompt_rules.iter().enumerate() {
                content.add_child(render_prompt_rule_row(
                    index,
                    row,
                    editor.prompt_rules.len(),
                    appearance,
                    app,
                ));
            }
            content.add_child(ChildView::new(&editor.add_rule_button).finish());
        }
    }
    if let Some(error) = &editor.save_error {
        content.add_child(error_text(error, appearance));
    }
    let buttons = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_child(
            Expanded::new(
                1.,
                Container::new(Text::new("", appearance.ui_font_family(), 1.).finish()).finish(),
            )
            .finish(),
        )
        .with_child(ChildView::new(&editor.cancel_button).finish())
        .with_child(
            Container::new(ChildView::new(&editor.save_button).finish())
                .with_margin_left(8.)
                .finish(),
        )
        .finish();
    content.add_child(buttons);
    if editor.close_confirmation {
        content.add_child(
            Container::new(
                Flex::column()
                    .with_spacing(8.)
                    .with_child(
                        Text::new(
                            "Discard unsaved router changes?",
                            appearance.ui_font_family(),
                            12.,
                        )
                        .with_color(appearance.theme().active_ui_text_color().into())
                        .finish(),
                    )
                    .with_child(
                        Flex::row()
                            .with_child(ChildView::new(&editor.keep_editing_button).finish())
                            .with_child(
                                Container::new(
                                    ChildView::new(&editor.discard_changes_button).finish(),
                                )
                                .with_margin_left(8.)
                                .finish(),
                            )
                            .finish(),
                    )
                    .finish(),
            )
            .with_background(appearance.theme().surface_2())
            .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
            .with_uniform_padding(12.)
            .finish(),
        );
    }
    Container::new(content.finish())
        .with_background(appearance.theme().surface_1())
        .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(6.)))
        .with_uniform_padding(16.)
        .finish()
}

fn labeled_editor(
    label: &str,
    editor: &ViewHandle<EditorView>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    Flex::column()
        .with_spacing(4.)
        .with_child(
            Text::new(label.to_owned(), appearance.ui_font_family(), 11.)
                .with_color(
                    appearance
                        .theme()
                        .sub_text_color(appearance.theme().surface_1())
                        .into(),
                )
                .finish(),
        )
        .with_child(
            ConstrainedBox::new(
                appearance
                    .ui_builder()
                    .text_input(editor.clone())
                    .with_style(warpui::ui_components::components::UiComponentStyles {
                        padding: Some(warpui::ui_components::components::Coords {
                            top: 8.,
                            bottom: 8.,
                            left: 12.,
                            right: 12.,
                        }),
                        background: Some(appearance.theme().surface_2().into()),
                        ..Default::default()
                    })
                    .build()
                    .finish(),
            )
            .with_width(420.)
            .finish(),
        )
        .finish()
}

fn labeled_model_dropdown(
    label: &str,
    dropdown: &ViewHandle<FilterableDropdown<CustomRouterManagerAction>>,
    selected: &str,
    appearance: &Appearance,
    _app: &AppContext,
) -> Box<dyn Element> {
    let mut column = Flex::column().with_spacing(4.);
    column.add_child(
        Text::new(label.to_owned(), appearance.ui_font_family(), 11.)
            .with_color(
                appearance
                    .theme()
                    .sub_text_color(appearance.theme().surface_1())
                    .into(),
            )
            .finish(),
    );
    column.add_child(
        ConstrainedBox::new(ChildView::new(dropdown).finish())
            .with_width(420.)
            .finish(),
    );
    if !selected.is_empty()
        && !concrete_custom_model_ids(&AISettings::as_ref(_app).custom_providers)
            .iter()
            .any(|model| model == selected)
    {
        column.add_child(
            Text::new(
                format!(
                    "Current target {selected} is not configured locally; save will be rejected."
                ),
                appearance.ui_font_family(),
                11.,
            )
            .with_color(appearance.theme().ui_error_color().into())
            .soft_wrap(true)
            .finish(),
        );
    }
    column.finish()
}

fn render_prompt_rule_row(
    index: usize,
    row: &PromptRuleRow,
    rule_count: usize,
    appearance: &Appearance,
    app: &AppContext,
) -> Box<dyn Element> {
    let mut fields = Flex::row()
        .with_main_axis_size(MainAxisSize::Max)
        .with_cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(
            Expanded::new(
                1.,
                labeled_editor("Rule description", &row.description_editor, appearance),
            )
            .finish(),
        )
        .with_child(
            Container::new(labeled_model_dropdown(
                "Model",
                &row.model_dropdown,
                &row.current_model,
                appearance,
                app,
            ))
            .with_margin_left(8.)
            .finish(),
        );
    let mut actions = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
    if index > 0 {
        actions.add_child(ChildView::new(&row.move_up_button).finish());
    }
    if index + 1 < rule_count {
        actions.add_child(ChildView::new(&row.move_down_button).finish());
    }
    actions.add_child(
        Container::new(ChildView::new(&row.remove_button).finish())
            .with_margin_left(4.)
            .finish(),
    );
    fields.add_child(
        Container::new(actions.finish())
            .with_margin_left(8.)
            .finish(),
    );
    fields.finish()
}
