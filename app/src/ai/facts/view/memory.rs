use std::path::PathBuf;

use ai::project_context::model::{ProjectContextModel, ProjectContextModelEvent};
use warp_core::ui::appearance::{Appearance, AppearanceEvent};
use warp_core::ui::theme::color::internal_colors;
use warpui::elements::{
    Align, Border, ChildView, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment,
    Expanded, Flex, Hoverable, MainAxisAlignment, MainAxisSize, MouseStateHandle, ParentElement,
    Shrinkable,
};
use warpui::platform::Cursor;
use warpui::ui_components::button::ButtonVariant;
use warpui::ui_components::components::{UiComponent, UiComponentStyles};
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::style;
use crate::ai::facts::local_memory::{LocalMemoryRecord, LocalMemoryScope};
use crate::ai::facts::manager::{AIFactManager, AIFactManagerEvent};
use crate::editor::{
    EditorView, PropagateAndNoOpNavigationKeys, SingleLineEditorOptions, TextOptions,
};
use crate::search_bar::SearchBar;
use crate::ui_components::icons::Icon;
use crate::view_components::DismissibleToast;
use crate::view_components::action_button::{ActionButton, NakedTheme};
use crate::workspace::ToastStack;

pub const HEADER_TEXT: &str = "Memory";
const DESCRIPTION_TEXT: &str = "Memory stores facts and preferences locally. Only keyword-relevant global entries and entries scoped to the current project are attached to a request.";
const SEARCH_PLACEHOLDER_TEXT: &str = "Search memory";
const ZERO_STATE_GLOBAL: &str = "Add a global memory to make it available in every local project.";
const ZERO_STATE_PROJECT: &str = "Open an indexed project, then add memory scoped to that project.";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemoryScopeTab {
    Global,
    Project,
}

#[derive(Debug, Clone)]
pub enum MemoryViewEvent {
    Add(LocalMemoryScope),
    Edit(LocalMemoryRecord),
}

#[derive(Debug, Clone)]
pub enum MemoryViewAction {
    AddGlobal,
    AddProject(PathBuf),
    Edit(uuid::Uuid),
    SelectGlobal,
    SelectProject,
}

pub struct MemoryView {
    memories: Vec<LocalMemoryRecord>,
    project_roots: Vec<PathBuf>,
    load_error: Option<String>,
    search_editor: ViewHandle<EditorView>,
    search_bar: ViewHandle<SearchBar>,
    add_global_button: ViewHandle<ActionButton>,
    current_scope: MemoryScopeTab,
    global_tab_mouse_state: MouseStateHandle,
    project_tab_mouse_state: MouseStateHandle,
}

impl MemoryView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let project_context = ProjectContextModel::handle(ctx);
        let project_roots = project_context
            .as_ref(ctx)
            .indexed_project_roots()
            .collect::<Vec<_>>();
        ctx.subscribe_to_model(&project_context, |me, model, event, ctx| {
            if matches!(event, ProjectContextModelEvent::PathIndexed) {
                me.project_roots = model.as_ref(ctx).indexed_project_roots().collect();
                ctx.notify();
            }
        });

        let manager = AIFactManager::handle(ctx);
        let (memories, load_error) = match manager.as_ref(ctx).list_memories() {
            Ok(memories) => (memories, manager.as_ref(ctx).memory_startup_error()),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };
        ctx.subscribe_to_model(&manager, |me, model, event, ctx| {
            if matches!(event, AIFactManagerEvent::MemoriesChanged) {
                match model.as_ref(ctx).list_memories() {
                    Ok(memories) => {
                        me.memories = memories;
                        me.load_error = None;
                    }
                    Err(error) => me.load_error = Some(error.to_string()),
                }
                ctx.notify();
            }
        });

        let appearance = Appearance::handle(ctx);
        ctx.subscribe_to_model(&appearance, |me, _, event, ctx| {
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
        ctx.subscribe_to_view(&search_editor, |_, _, _, ctx| ctx.notify());
        search_editor.update(ctx, |editor, ctx| {
            editor.set_placeholder_text(SEARCH_PLACEHOLDER_TEXT, ctx)
        });
        let search_bar = ctx.add_typed_action_view(|_| SearchBar::new(search_editor.clone()));
        let add_global_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Add", NakedTheme)
                .with_icon(Icon::Plus)
                .on_click(|ctx| ctx.dispatch_typed_action(MemoryViewAction::AddGlobal))
        });

        Self {
            memories,
            project_roots,
            load_error,
            search_editor,
            search_bar,
            add_global_button,
            current_scope: MemoryScopeTab::Global,
            global_tab_mouse_state: Default::default(),
            project_tab_mouse_state: Default::default(),
        }
    }

    fn show_error(&self, message: impl Into<String>, ctx: &mut ViewContext<Self>) {
        let message = message.into();
        let window_id = ctx.window_id();
        ToastStack::handle(ctx).update(ctx, move |toasts, ctx| {
            toasts.add_ephemeral_toast(DismissibleToast::error(message), window_id, ctx)
        });
    }

    fn filtered_memories(&self, app: &AppContext) -> Vec<LocalMemoryRecord> {
        let search = self
            .search_editor
            .as_ref(app)
            .buffer_text(app)
            .trim()
            .to_lowercase();
        let mut memories = self
            .memories
            .iter()
            .filter(|memory| {
                matches!(
                    (self.current_scope, &memory.scope),
                    (MemoryScopeTab::Global, LocalMemoryScope::Global)
                        | (MemoryScopeTab::Project, LocalMemoryScope::Project { .. })
                )
            })
            .filter(|memory| {
                search.is_empty()
                    || memory.title.to_lowercase().contains(&search)
                    || memory.content.to_lowercase().contains(&search)
                    || memory.scope.display_name().to_lowercase().contains(&search)
            })
            .cloned()
            .collect::<Vec<_>>();
        memories.sort_by(|left, right| {
            right
                .updated_at
                .cmp(&left.updated_at)
                .then_with(|| left.title.cmp(&right.title))
                .then_with(|| left.id.cmp(&right.id))
        });
        memories
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Container::new(
                    ConstrainedBox::new(
                        warpui::elements::Icon::new(
                            Icon::Lightbulb.into(),
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
        let tab = |title: &str,
                   scope: MemoryScopeTab,
                   action: MemoryViewAction,
                   mouse_state: MouseStateHandle| {
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
            .on_click(move |ctx, _, _| ctx.dispatch_typed_action(action.clone()))
            .finish()
        };
        Container::new(
            Flex::row()
                .with_child(tab(
                    "Global",
                    MemoryScopeTab::Global,
                    MemoryViewAction::SelectGlobal,
                    self.global_tab_mouse_state.clone(),
                ))
                .with_child(tab(
                    "Project based",
                    MemoryScopeTab::Project,
                    MemoryViewAction::SelectProject,
                    self.project_tab_mouse_state.clone(),
                ))
                .finish(),
        )
        .with_margin_bottom(style::SECTION_MARGIN)
        .finish()
    }

    fn render_add_buttons(&self, appearance: &Appearance) -> Box<dyn Element> {
        match self.current_scope {
            MemoryScopeTab::Global => ChildView::new(&self.add_global_button).finish(),
            MemoryScopeTab::Project => {
                let mut column = Flex::column();
                for root in &self.project_roots {
                    let root = root.clone();
                    let label = format!("Add memory ({})", root.display());
                    column.add_child(
                        appearance
                            .ui_builder()
                            .button(ButtonVariant::Outlined, Default::default())
                            .with_text_label(label)
                            .build()
                            .on_click(move |ctx, _, _| {
                                ctx.dispatch_typed_action(MemoryViewAction::AddProject(
                                    root.clone(),
                                ))
                            })
                            .finish(),
                    );
                }
                column.finish()
            }
        }
    }

    fn render_row(&self, memory: LocalMemoryRecord, appearance: &Appearance) -> Box<dyn Element> {
        let id = memory.id;
        let content = super::truncate_display_text(&memory.content, 240);
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Shrinkable::new(
                        1.,
                        Flex::column()
                            .with_child(
                                appearance
                                    .ui_builder()
                                    .wrappable_text(memory.title, true)
                                    .with_style(style::fact_row_text(appearance))
                                    .build()
                                    .finish(),
                            )
                            .with_child(
                                appearance
                                    .ui_builder()
                                    .wrappable_text(memory.scope.display_name(), true)
                                    .with_style(style::fact_row_subtext(appearance))
                                    .build()
                                    .finish(),
                            )
                            .with_child(
                                appearance
                                    .ui_builder()
                                    .wrappable_text(content, true)
                                    .with_style(style::fact_project_based_row_text(appearance))
                                    .build()
                                    .finish(),
                            )
                            .finish(),
                    )
                    .finish(),
                )
                .with_child(
                    appearance
                        .ui_builder()
                        .button(ButtonVariant::Outlined, Default::default())
                        .with_text_label("Edit".to_string())
                        .build()
                        .on_click(move |ctx, _, _| {
                            ctx.dispatch_typed_action(MemoryViewAction::Edit(id))
                        })
                        .finish(),
                )
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

    fn render_zero_state(&self, appearance: &Appearance) -> Box<dyn Element> {
        let text = match self.current_scope {
            MemoryScopeTab::Global => ZERO_STATE_GLOBAL,
            MemoryScopeTab::Project => ZERO_STATE_PROJECT,
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

impl Entity for MemoryView {
    type Event = MemoryViewEvent;
}

impl View for MemoryView {
    fn ui_name() -> &'static str {
        "MemoryView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.search_editor);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let memories = self.filtered_memories(app);
        let mut column = Flex::column()
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
        if let Some(error) = &self.load_error {
            column.add_child(
                appearance
                    .ui_builder()
                    .wrappable_text(error.clone(), true)
                    .with_style(style::fact_project_based_row_text(appearance))
                    .build()
                    .finish(),
            );
        }
        if memories.is_empty() {
            column.add_child(self.render_zero_state(appearance));
        } else {
            column.add_child(
                Container::new(
                    Flex::row()
                        .with_child(
                            Expanded::new(1., ChildView::new(&self.search_bar).finish()).finish(),
                        )
                        .with_child(self.render_add_buttons(appearance))
                        .finish(),
                )
                .with_margin_bottom(style::SECTION_MARGIN)
                .finish(),
            );
            for memory in memories {
                column.add_child(self.render_row(memory, appearance));
            }
        }
        column.finish()
    }
}

impl TypedActionView for MemoryView {
    type Action = MemoryViewAction;

    fn handle_action(&mut self, action: &MemoryViewAction, ctx: &mut ViewContext<Self>) {
        match action {
            MemoryViewAction::AddGlobal => ctx.emit(MemoryViewEvent::Add(LocalMemoryScope::Global)),
            MemoryViewAction::AddProject(root) => {
                ctx.emit(MemoryViewEvent::Add(LocalMemoryScope::Project {
                    root: root.clone(),
                }))
            }
            MemoryViewAction::Edit(id) => {
                if let Some(memory) = self.memories.iter().find(|memory| memory.id == *id) {
                    ctx.emit(MemoryViewEvent::Edit(memory.clone()));
                } else {
                    self.show_error("Local memory changed; reopen it and try again", ctx);
                }
            }
            MemoryViewAction::SelectGlobal => {
                self.current_scope = MemoryScopeTab::Global;
                ctx.notify();
            }
            MemoryViewAction::SelectProject => {
                self.current_scope = MemoryScopeTab::Project;
                ctx.notify();
            }
        }
    }
}
