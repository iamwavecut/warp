use std::fmt::Debug;
use std::path::PathBuf;

use ai::project_context::local_rule_repository::{
    LocalRule, LocalRuleError, LocalRuleRepository, ProjectRuleFile,
};
use ai::project_context::model::{ProjectContextModel, ProjectContextModelEvent};
use warp_core::ui::appearance::{Appearance, AppearanceEvent};
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Align, Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    Expanded, Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement,
    Shrinkable,
};
use warpui::platform::{Cursor, FilePickerConfiguration};
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::style;
use crate::editor::{
    EditorView, PropagateAndNoOpNavigationKeys, SingleLineEditorOptions, TextOptions,
};
use crate::search_bar::SearchBar;
use crate::ui_components::icons::Icon;
use crate::view_components::DismissibleToast;
use crate::view_components::action_button::{ActionButton, NakedTheme};
use crate::workspace::ToastStack;

pub const HEADER_TEXT: &str = "Rules";
const DESCRIPTION_TEXT: &str = "Rules enhance the agent by providing structured guidelines that help maintain consistency, enforce best practices, and adapt to specific workflows, including codebases or broader tasks.";
const SEARCH_PLACEHOLDER_TEXT: &str = "Search rules";
const ZERO_STATE_TEXT: &str =
    "Add a rule above, or create ~/.agents/AGENTS.md to apply it across every project.";
const ZERO_STATE_TEXT_PROJECT: &str =
    "Add a WARP.md rule to an indexed project, or open a project to index its rules.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleScope {
    Global,
    ProjectBased,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleTarget {
    Global,
    Project {
        root: PathBuf,
        file: ProjectRuleFile,
    },
}

impl RuleTarget {
    pub fn display_path(&self) -> PathBuf {
        match self {
            Self::Global => dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("~"))
                .join(".agents/AGENTS.md"),
            Self::Project { root, file } => root.join(file.file_name()),
        }
    }
}

#[derive(Debug, Clone)]
pub enum RuleViewEvent {
    AddRule(RuleTarget),
    Edit(LocalRule),
    OpenSettings,
    OpenFile(PathBuf),
    InitializeProject(PathBuf),
}

#[derive(Debug, Clone)]
pub enum RuleViewAction {
    AddRule,
    AddProject {
        root: PathBuf,
        file: ProjectRuleFile,
    },
    InitializeProject,
    Edit(PathBuf),
    OpenSettings,
    SelectScope(RuleScope),
    OpenFile(PathBuf),
}

#[derive(Default, Debug, Clone)]
pub struct MouseStateHandles {
    pub hover: MouseStateHandle,
}

#[derive(Debug, Clone)]
struct FileBackedRow {
    path: PathBuf,
    rule: Option<LocalRule>,
    error: Option<String>,
    mouse_states: MouseStateHandles,
}

impl FileBackedRow {
    fn matches_search_term(&self, search_term: &str) -> bool {
        self.path
            .to_string_lossy()
            .to_lowercase()
            .contains(search_term)
            || self
                .rule
                .as_ref()
                .is_some_and(|rule| rule.content.to_lowercase().contains(search_term))
            || self
                .error
                .as_deref()
                .is_some_and(|error| error.to_lowercase().contains(search_term))
    }
}

pub struct RuleView {
    repository: LocalRuleRepository,
    file_backed_global_rules: Vec<FileBackedRow>,
    project_rules: Vec<FileBackedRow>,
    project_roots: Vec<PathBuf>,
    search_editor: ViewHandle<EditorView>,
    search_bar: ViewHandle<SearchBar>,
    add_button: ViewHandle<ActionButton>,
    initialize_button: ViewHandle<ActionButton>,
    current_scope: RuleScope,
    global_tab_mouse_state: MouseStateHandle,
    project_tab_mouse_state: MouseStateHandle,
}

impl RuleView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let project_context = ProjectContextModel::handle(ctx);
        let mut repository = LocalRuleRepository::new();
        let (file_backed_global_rules, project_rules, project_roots) =
            Self::load_rows(&mut repository, project_context.as_ref(ctx));

        ctx.subscribe_to_model(&project_context, |me, model, event, ctx| match event {
            ProjectContextModelEvent::PathIndexed
            | ProjectContextModelEvent::GlobalRulesChanged(_) => {
                let model = model.as_ref(ctx);
                let global_paths = model.global_rule_paths().collect::<Vec<_>>();
                let project_roots = model.indexed_project_roots().collect::<Vec<_>>();
                me.refresh_local_rules(global_paths, project_roots, ctx);
            }
            ProjectContextModelEvent::KnownRulesChanged(_) => {}
        });

        let appearance = Appearance::handle(ctx);
        ctx.subscribe_to_model(&appearance, move |me, _, event, ctx| {
            if let AppearanceEvent::ThemeChanged = event {
                let styles = style::search_bar(Appearance::as_ref(ctx));
                me.search_bar
                    .update(ctx, |search_bar, _| search_bar.with_style(styles));
            }
        });

        let search_editor = {
            let options = SingleLineEditorOptions {
                text: TextOptions::ui_text(None, appearance.as_ref(ctx)),
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            ctx.add_typed_action_view(|ctx| EditorView::single_line(options, ctx))
        };
        ctx.subscribe_to_view(&search_editor, |_, _, _event, ctx| ctx.notify());
        search_editor.update(ctx, |editor, ctx| {
            editor.clear_buffer_and_reset_undo_stack(ctx);
            editor.set_placeholder_text(SEARCH_PLACEHOLDER_TEXT, ctx);
        });
        let search_bar = ctx.add_typed_action_view(|_| SearchBar::new(search_editor.clone()));

        let add_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Add", NakedTheme)
                .with_icon(Icon::Plus)
                .on_click(|ctx| ctx.dispatch_typed_action(RuleViewAction::AddRule))
        });
        let initialize_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Open project", NakedTheme)
                .with_icon(Icon::Plus)
                .on_click(|ctx| ctx.dispatch_typed_action(RuleViewAction::InitializeProject))
        });
        Self {
            repository,
            file_backed_global_rules,
            project_rules,
            project_roots,
            search_editor,
            search_bar,
            add_button,
            initialize_button,
            current_scope: RuleScope::Global,
            global_tab_mouse_state: Default::default(),
            project_tab_mouse_state: Default::default(),
        }
    }

    fn load_rows(
        repository: &mut LocalRuleRepository,
        model: &ProjectContextModel,
    ) -> (Vec<FileBackedRow>, Vec<FileBackedRow>, Vec<PathBuf>) {
        let global_paths = model.global_rule_paths().collect::<Vec<_>>();
        let project_roots = model.indexed_project_roots().collect::<Vec<_>>();
        Self::load_rows_from_paths(repository, global_paths, project_roots)
    }

    fn load_rows_from_paths(
        repository: &mut LocalRuleRepository,
        global_paths: Vec<PathBuf>,
        project_roots: Vec<PathBuf>,
    ) -> (Vec<FileBackedRow>, Vec<FileBackedRow>, Vec<PathBuf>) {
        repository.set_surfaced_paths(global_paths.clone(), project_roots.clone());
        let mut global_rules = Vec::new();
        let mut project_rules = Vec::new();
        for path in repository.surfaced_paths().cloned().collect::<Vec<_>>() {
            let is_global = repository.is_global_path(&path);
            let row = match repository.read(&path) {
                Ok(rule) => Some(FileBackedRow {
                    path: path.clone(),
                    rule: Some(rule),
                    error: None,
                    mouse_states: Default::default(),
                }),
                Err(LocalRuleError::NotFound { .. }) => None,
                Err(error) => Some(FileBackedRow {
                    path: path.clone(),
                    rule: None,
                    error: Some(error.to_string()),
                    mouse_states: Default::default(),
                }),
            };
            if let Some(row) = row {
                if is_global {
                    global_rules.push(row);
                } else {
                    project_rules.push(row);
                }
            }
        }
        (global_rules, project_rules, project_roots)
    }

    fn refresh_local_rules(
        &mut self,
        global_paths: Vec<PathBuf>,
        project_roots: Vec<PathBuf>,
        ctx: &mut ViewContext<Self>,
    ) {
        let (global, project, roots) =
            Self::load_rows_from_paths(&mut self.repository, global_paths, project_roots);
        self.file_backed_global_rules = global;
        self.project_rules = project;
        self.project_roots = roots;
        ctx.notify();
    }

    fn show_error(&self, error: &LocalRuleError, ctx: &mut ViewContext<Self>) {
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, |toasts, ctx| {
            toasts.add_ephemeral_toast(DismissibleToast::error(error.to_string()), window_id, ctx);
        });
    }

    pub fn save_local_rule(
        &mut self,
        target: &super::rule_editor::RuleEditorTarget,
        content: &str,
        ctx: &mut ViewContext<Self>,
    ) -> Result<LocalRule, LocalRuleError> {
        let result = match target {
            super::rule_editor::RuleEditorTarget::New(target) => match target {
                RuleTarget::Global => self.repository.create_global(content),
                RuleTarget::Project { root, file } => {
                    self.repository.create_project(root, *file, content)
                }
            },
            super::rule_editor::RuleEditorTarget::Existing(rule) => {
                self.repository.update(&rule.path, &rule.revision, content)
            }
        };
        match result {
            Ok(rule) => {
                self.reload_rows_from_repository(ctx);
                Ok(rule)
            }
            Err(error) => {
                self.show_error(&error, ctx);
                Err(error)
            }
        }
    }

    pub fn delete_local_rule(
        &mut self,
        rule: &LocalRule,
        ctx: &mut ViewContext<Self>,
    ) -> Result<(), LocalRuleError> {
        match self.repository.delete(&rule.path, &rule.revision) {
            Ok(()) => {
                self.reload_rows_from_repository(ctx);
                Ok(())
            }
            Err(error) => {
                self.show_error(&error, ctx);
                Err(error)
            }
        }
    }

    fn reload_rows_from_repository(&mut self, ctx: &mut ViewContext<Self>) {
        let paths = self
            .repository
            .surfaced_paths()
            .cloned()
            .collect::<Vec<_>>();
        let mut global = Vec::new();
        let mut project = Vec::new();
        for path in paths {
            let is_global = self.repository.is_global_path(&path);
            let row = match self.repository.read(&path) {
                Ok(rule) => Some(FileBackedRow {
                    path: path.clone(),
                    rule: Some(rule),
                    error: None,
                    mouse_states: Default::default(),
                }),
                Err(LocalRuleError::NotFound { .. }) => None,
                Err(error) => Some(FileBackedRow {
                    path: path.clone(),
                    rule: None,
                    error: Some(error.to_string()),
                    mouse_states: Default::default(),
                }),
            };
            if let Some(row) = row {
                if is_global {
                    global.push(row);
                } else {
                    project.push(row);
                }
            }
        }
        self.file_backed_global_rules = global;
        self.project_rules = project;
        ctx.notify();
    }

    fn select_scope(&mut self, scope: RuleScope, ctx: &mut ViewContext<Self>) {
        self.current_scope = scope;
        ctx.notify();
    }

    fn filtered_rules(&self) -> Vec<FileBackedRow> {
        match self.current_scope {
            RuleScope::Global => self.file_backed_global_rules.clone(),
            RuleScope::ProjectBased => self.project_rules.clone(),
        }
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(
                    ConstrainedBox::new(
                        warpui::elements::Icon::new(
                            Icon::BookOpen.into(),
                            appearance
                                .theme()
                                .main_text_color(appearance.theme().background()),
                        )
                        .finish(),
                    )
                    .with_width(style::ICON_SIZE)
                    .with_height(style::ICON_SIZE)
                    .finish(),
                )
                .with_margin_right(style::ICON_MARGIN)
                .finish(),
            )
            .with_child(
                appearance
                    .ui_builder()
                    .wrappable_text(HEADER_TEXT, true)
                    .with_style(style::header_text())
                    .build()
                    .finish(),
            )
            .finish()
    }

    fn render_scope_tabs(&self, appearance: &Appearance) -> Box<dyn Element> {
        let tab = |title: &str, scope: RuleScope, mouse_state: MouseStateHandle| {
            let selected = self.current_scope == scope;
            let title = title.to_string();
            Hoverable::new(mouse_state, move |state| {
                let color = if selected {
                    appearance
                        .theme()
                        .main_text_color(appearance.theme().background())
                } else {
                    appearance
                        .theme()
                        .sub_text_color(appearance.theme().background())
                };
                let mut container = Container::new(
                    appearance
                        .ui_builder()
                        .wrappable_text(title.clone(), true)
                        .with_style(UiComponentStyles {
                            font_size: Some(style::TEXT_FONT_SIZE),
                            font_color: Some(color.into()),
                            ..Default::default()
                        })
                        .build()
                        .finish(),
                )
                .with_horizontal_padding(style::ROW_HORIZONTAL_PADDING)
                .with_vertical_padding(8.);
                if selected || state.is_hovered() {
                    container = container
                        .with_background(if selected {
                            appearance.theme().surface_2()
                        } else {
                            appearance.theme().surface_1()
                        })
                        .with_corner_radius(CornerRadius::with_all(
                            warpui::elements::Radius::Pixels(4.),
                        ));
                }
                container.finish()
            })
            .with_cursor(Cursor::PointingHand)
            .on_click(move |ctx, _, _| {
                ctx.dispatch_typed_action(RuleViewAction::SelectScope(scope))
            })
            .finish()
        };

        Container::new(
            Flex::row()
                .with_child(tab(
                    "Global",
                    RuleScope::Global,
                    self.global_tab_mouse_state.clone(),
                ))
                .with_child(tab(
                    "Project based",
                    RuleScope::ProjectBased,
                    self.project_tab_mouse_state.clone(),
                ))
                .finish(),
        )
        .with_margin_bottom(style::SECTION_MARGIN)
        .finish()
    }

    fn render_add_buttons(&self, appearance: &Appearance) -> Box<dyn Element> {
        let children = match self.current_scope {
            RuleScope::Global => {
                let mut row = Flex::row();
                if self.repository.global_target_missing() {
                    row.add_child(ChildView::new(&self.add_button).finish());
                }
                row
            }
            RuleScope::ProjectBased if self.project_roots.is_empty() => {
                Flex::row().with_child(ChildView::new(&self.initialize_button).finish())
            }
            RuleScope::ProjectBased => {
                let mut roots = Flex::column();
                for root in &self.project_roots {
                    let mut buttons = Flex::row();
                    for file in [ProjectRuleFile::Warp, ProjectRuleFile::Agents] {
                        let Ok(path) = self.repository.project_rule_path(root, file) else {
                            continue;
                        };
                        let missing = matches!(
                            self.repository.read(&path),
                            Err(LocalRuleError::NotFound { .. })
                        );
                        if !missing {
                            continue;
                        }
                        let root = root.clone();
                        let label = format!("Add {} ({})", file.file_name(), root.display());
                        buttons.add_child(
                            appearance
                                .ui_builder()
                                .button(ButtonVariant::Outlined, Default::default())
                                .with_text_label(label)
                                .build()
                                .on_click(move |ctx, _, _| {
                                    ctx.dispatch_typed_action(RuleViewAction::AddProject {
                                        root: root.clone(),
                                        file,
                                    })
                                })
                                .finish(),
                        );
                    }
                    roots.add_child(buttons.finish());
                }
                roots
            }
        };
        Container::new(children.finish())
            .with_margin_left(style::SECTION_MARGIN)
            .finish()
    }

    fn render_file_backed_row(
        &self,
        row: FileBackedRow,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let path = row.path.clone();
        let path_text = path.to_string_lossy().to_string();
        let edit_path = path.clone();
        let mut controls = Flex::row().with_cross_axis_alignment(CrossAxisAlignment::Center);
        if row.rule.as_ref().is_some_and(|rule| rule.writable) {
            controls.add_child(
                appearance
                    .ui_builder()
                    .button(ButtonVariant::Outlined, row.mouse_states.hover.clone())
                    .with_text_label("Edit".to_string())
                    .build()
                    .on_click(move |ctx, _, _| {
                        ctx.dispatch_typed_action(RuleViewAction::Edit(edit_path.clone()))
                    })
                    .finish(),
            );
        } else {
            controls.add_child(
                appearance
                    .ui_builder()
                    .wrappable_text("Read-only", true)
                    .with_style(style::fact_project_based_row_text(appearance))
                    .build()
                    .finish(),
            );
        }
        if let Some(error) = row.error {
            controls.add_child(
                appearance
                    .ui_builder()
                    .wrappable_text(error, true)
                    .with_style(style::fact_project_based_row_text(appearance))
                    .build()
                    .finish(),
            );
        }

        let open_path = path.clone();
        controls.add_child(
            appearance
                .ui_builder()
                .button(ButtonVariant::Outlined, Default::default())
                .with_text_label("Open file".to_string())
                .build()
                .on_click(move |ctx, _, _| {
                    ctx.dispatch_typed_action(RuleViewAction::OpenFile(open_path.clone()))
                })
                .finish(),
        );

        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.,
                        appearance
                            .ui_builder()
                            .wrappable_text(path_text, true)
                            .with_style(style::fact_project_based_row_text(appearance))
                            .build()
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(controls.finish())
                .finish(),
        )
        .with_background(internal_colors::neutral_1(appearance.theme()))
        .with_corner_radius(CornerRadius::with_all(warpui::elements::Radius::Pixels(4.)))
        .with_border(
            Border::all(1.).with_border_color(internal_colors::neutral_2(appearance.theme())),
        )
        .with_horizontal_padding(style::ROW_HORIZONTAL_PADDING)
        .with_vertical_padding(style::RULE_VERTICAL_PADDING)
        .with_margin_bottom(style::ITEM_BOTTOM_MARGIN)
        .finish()
    }

    fn render_items(
        &self,
        appearance: &Appearance,
        mut rows: Vec<FileBackedRow>,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let search = self
            .search_editor
            .as_ref(app)
            .buffer_text(app)
            .to_lowercase();
        if !search.is_empty() {
            rows.retain(|row| row.matches_search_term(&search));
        }
        rows.sort_by(|a, b| a.path.cmp(&b.path));
        let mut col = Flex::column();
        for row in rows {
            col.add_child(self.render_file_backed_row(row, appearance));
        }
        col.finish()
    }

    fn render_zero_state(&self, appearance: &Appearance) -> Box<dyn Element> {
        let text = match self.current_scope {
            RuleScope::Global => ZERO_STATE_TEXT,
            RuleScope::ProjectBased => ZERO_STATE_TEXT_PROJECT,
        };
        Container::new(
            ConstrainedBox::new(
                Align::new(
                    Flex::column()
                        .with_main_axis_size(MainAxisSize::Max)
                        .with_main_axis_alignment(MainAxisAlignment::Center)
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            appearance
                                .ui_builder()
                                .wrappable_text(text, true)
                                .with_style(style::description_text(appearance))
                                .build()
                                .finish(),
                        )
                        .with_child(self.render_add_buttons(appearance))
                        .finish(),
                )
                .finish(),
            )
            .with_height(style::ZERO_STATE_HEIGHT)
            .finish(),
        )
        .with_horizontal_padding(style::ROW_HORIZONTAL_PADDING)
        .with_border(
            Border::all(1.).with_border_color(internal_colors::neutral_2(appearance.theme())),
        )
        .with_margin_bottom(style::SECTION_MARGIN)
        .finish()
    }
}

impl Entity for RuleView {
    type Event = RuleViewEvent;
}

impl View for RuleView {
    fn ui_name() -> &'static str {
        "RuleView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.search_editor);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let rows = self.filtered_rules();
        let mut col = Flex::column()
            .with_child(self.render_header(appearance))
            .with_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .wrappable_text(DESCRIPTION_TEXT, true)
                        .with_style(style::description_text(appearance))
                        .build()
                        .finish(),
                )
                .with_vertical_margin(style::ITEM_BOTTOM_MARGIN)
                .finish(),
            )
            .with_child(self.render_scope_tabs(appearance));

        if rows.is_empty() {
            col.add_child(self.render_zero_state(appearance));
        } else {
            col.add_child(
                Flex::column()
                    .with_child(
                        Container::new(
                            Flex::row()
                                .with_child(
                                    Expanded::new(1., ChildView::new(&self.search_bar).finish())
                                        .finish(),
                                )
                                .with_child(self.render_add_buttons(appearance))
                                .finish(),
                        )
                        .with_margin_bottom(style::SECTION_MARGIN)
                        .finish(),
                    )
                    .with_child(self.render_items(appearance, rows, app))
                    .finish(),
            );
        }
        col.finish()
    }
}

impl TypedActionView for RuleView {
    type Action = RuleViewAction;

    fn handle_action(&mut self, action: &RuleViewAction, ctx: &mut ViewContext<Self>) {
        match action {
            RuleViewAction::AddRule => {
                if self.current_scope == RuleScope::Global
                    && self.repository.global_target_missing()
                {
                    ctx.emit(RuleViewEvent::AddRule(RuleTarget::Global));
                }
            }
            RuleViewAction::AddProject { root, file } => {
                if let Ok(path) = self.repository.project_rule_path(root, *file)
                    && matches!(
                        self.repository.read(&path),
                        Err(LocalRuleError::NotFound { .. })
                    )
                {
                    ctx.emit(RuleViewEvent::AddRule(RuleTarget::Project {
                        root: root.clone(),
                        file: *file,
                    }));
                }
            }
            RuleViewAction::InitializeProject => {
                let window_id = ctx.window_id();
                ctx.open_file_picker(
                    move |result, ctx| match result {
                        Ok(paths) => {
                            if let Some(path) = paths.first() {
                                ctx.emit(RuleViewEvent::InitializeProject(PathBuf::from(path)));
                            }
                        }
                        Err(error) => ToastStack::handle(ctx).update(ctx, |toasts, ctx| {
                            toasts.add_ephemeral_toast(
                                DismissibleToast::error(format!("{error}")),
                                window_id,
                                ctx,
                            );
                        }),
                    },
                    FilePickerConfiguration::new().folders_only(),
                );
            }
            RuleViewAction::Edit(path) => match self.repository.read(path) {
                Ok(rule) => ctx.emit(RuleViewEvent::Edit(rule)),
                Err(error) => self.show_error(&error, ctx),
            },
            RuleViewAction::OpenSettings => ctx.emit(RuleViewEvent::OpenSettings),
            RuleViewAction::SelectScope(scope) => self.select_scope(*scope, ctx),
            RuleViewAction::OpenFile(path) => ctx.emit(RuleViewEvent::OpenFile(path.clone())),
        }
    }
}
