use std::path::PathBuf;

use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    Align, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, Flex, Hoverable, MainAxisSize, MouseStateHandle, ParentElement, ScrollbarWidth,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent;
use warpui::ui_components::components::UiComponentStyles;
use warpui::{
    AppContext, Element, Entity, FocusContext, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::ai::facts::manager::AIFactManager;
use crate::ai::facts::view::rule_editor::RuleEditorTarget;
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view;
use crate::pane_group::{BackingView, PaneConfiguration, PaneEvent};

pub mod memory;
pub mod memory_editor;
pub mod rule;
pub mod rule_editor;
mod style;
use crate::view_components::DismissibleToast;
use crate::workspace::ToastStack;
use memory::{MemoryView, MemoryViewEvent};
use memory_editor::{MemoryEditorTarget, MemoryEditorView, MemoryEditorViewEvent};
use rule::{RuleTarget, RuleView, RuleViewEvent};
use rule_editor::{RuleEditorView, RuleEditorViewEvent};

const HEADER_TEXT: &str = "Knowledge";
const OFFLINE_TEXT: &str = "Local rules and memory remain available without a provider or network.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AIFactPage {
    Rules,
    RuleEditor { target: RuleEditorTarget },
    Memory,
    MemoryEditor { target: MemoryEditorTarget },
}

impl Default for AIFactPage {
    fn default() -> Self {
        Self::Rules
    }
}

impl std::fmt::Display for AIFactPage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rules => write!(f, "Rules"),
            Self::RuleEditor { .. } => write!(f, "Rule Editor"),
            Self::Memory => write!(f, "Memory"),
            Self::MemoryEditor { .. } => write!(f, "Memory Editor"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum AIFactViewEvent {
    Pane(PaneEvent),
    OpenSettings,
    OpenFile(PathBuf),
    InitializeProject(PathBuf),
}

#[derive(Debug, Clone)]
pub enum AIFactViewAction {
    AddRule,
    UpdatePage(AIFactPage),
}

pub struct AIFactView {
    pane_configuration: ModelHandle<PaneConfiguration>,
    focus_handle: Option<PaneFocusHandle>,
    current_page: AIFactPage,
    rule_view: ViewHandle<RuleView>,
    rule_editor_view: ViewHandle<RuleEditorView>,
    memory_view: ViewHandle<MemoryView>,
    memory_editor_view: ViewHandle<MemoryEditorView>,
    rules_tab_mouse_state: MouseStateHandle,
    memory_tab_mouse_state: MouseStateHandle,
    clipped_scroll_state: ClippedScrollStateHandle,
}

impl AIFactView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let pane_configuration = ctx.add_model(|_ctx| PaneConfiguration::new(HEADER_TEXT));
        let rule_view = ctx.add_typed_action_view(RuleView::new);
        ctx.subscribe_to_view(&rule_view, |me, _, event, ctx| {
            me.handle_rule_view_event(event, ctx)
        });
        let rule_editor_view = ctx.add_typed_action_view(RuleEditorView::new);
        ctx.subscribe_to_view(&rule_editor_view, |me, _, event, ctx| {
            me.handle_rule_editor_view_event(event, ctx)
        });
        let memory_view = ctx.add_typed_action_view(MemoryView::new);
        ctx.subscribe_to_view(&memory_view, |me, _, event, ctx| {
            me.handle_memory_view_event(event, ctx)
        });
        let memory_editor_view = ctx.add_typed_action_view(MemoryEditorView::new);
        ctx.subscribe_to_view(&memory_editor_view, |me, _, event, ctx| {
            me.handle_memory_editor_view_event(event, ctx)
        });
        Self {
            pane_configuration,
            focus_handle: None,
            current_page: AIFactPage::default(),
            rule_view,
            rule_editor_view,
            memory_view,
            memory_editor_view,
            rules_tab_mouse_state: Default::default(),
            memory_tab_mouse_state: Default::default(),
            clipped_scroll_state: Default::default(),
        }
    }

    pub fn pane_configuration(&self) -> ModelHandle<PaneConfiguration> {
        self.pane_configuration.clone()
    }

    pub fn current_page(&self) -> AIFactPage {
        self.current_page.clone()
    }

    pub fn focus(&mut self, ctx: &mut ViewContext<Self>) {
        match &self.current_page {
            AIFactPage::Rules => ctx.focus(&self.rule_view),
            AIFactPage::RuleEditor { .. } => ctx.focus(&self.rule_editor_view),
            AIFactPage::Memory => ctx.focus(&self.memory_view),
            AIFactPage::MemoryEditor { .. } => ctx.focus(&self.memory_editor_view),
        }
    }

    fn handle_rule_view_event(&mut self, event: &RuleViewEvent, ctx: &mut ViewContext<Self>) {
        match event {
            RuleViewEvent::AddRule(target) => self.update_page(
                AIFactPage::RuleEditor {
                    target: RuleEditorTarget::New(target.clone()),
                },
                ctx,
            ),
            RuleViewEvent::Edit(rule) => self.update_page(
                AIFactPage::RuleEditor {
                    target: RuleEditorTarget::Existing(rule.clone()),
                },
                ctx,
            ),
            RuleViewEvent::OpenSettings => ctx.emit(AIFactViewEvent::OpenSettings),
            RuleViewEvent::OpenFile(path) => ctx.emit(AIFactViewEvent::OpenFile(path.clone())),
            RuleViewEvent::InitializeProject(path) => {
                ctx.emit(AIFactViewEvent::InitializeProject(path.clone()))
            }
        }
    }

    fn handle_rule_editor_view_event(
        &mut self,
        event: &RuleEditorViewEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        match event {
            RuleEditorViewEvent::Back => self.update_page(AIFactPage::Rules, ctx),
            RuleEditorViewEvent::Save { target, content } => {
                let result = self.rule_view.update(ctx, |rule_view, ctx| {
                    rule_view.save_local_rule(target, content, ctx)
                });
                if result.is_ok() {
                    self.update_page(AIFactPage::Rules, ctx);
                }
            }
            RuleEditorViewEvent::Delete { rule } => {
                let result = self
                    .rule_view
                    .update(ctx, |rule_view, ctx| rule_view.delete_local_rule(rule, ctx));
                if result.is_ok() {
                    self.update_page(AIFactPage::Rules, ctx);
                }
            }
        }
    }

    fn handle_memory_view_event(&mut self, event: &MemoryViewEvent, ctx: &mut ViewContext<Self>) {
        let target = match event {
            MemoryViewEvent::Add(scope) => MemoryEditorTarget::New(scope.clone()),
            MemoryViewEvent::Edit(memory) => MemoryEditorTarget::Existing(memory.clone()),
        };
        self.update_page(AIFactPage::MemoryEditor { target }, ctx);
    }

    fn handle_memory_editor_view_event(
        &mut self,
        event: &MemoryEditorViewEvent,
        ctx: &mut ViewContext<Self>,
    ) {
        let result = match event {
            MemoryEditorViewEvent::Back => {
                self.update_page(AIFactPage::Memory, ctx);
                return;
            }
            MemoryEditorViewEvent::Save {
                target,
                title,
                content,
            } => AIFactManager::handle(ctx).update(ctx, |manager, ctx| match target {
                MemoryEditorTarget::New(scope) => manager
                    .create_memory(scope.clone(), title, content, ctx)
                    .map(|_| ()),
                MemoryEditorTarget::Existing(memory) => manager
                    .update_memory(
                        memory.id,
                        memory.revision,
                        memory.scope.clone(),
                        title,
                        content,
                        ctx,
                    )
                    .map(|_| ()),
            }),
            MemoryEditorViewEvent::Delete { memory } => AIFactManager::handle(ctx)
                .update(ctx, |manager, ctx| {
                    manager.delete_memory(memory.id, memory.revision, ctx)
                }),
        };
        match result {
            Ok(()) => self.update_page(AIFactPage::Memory, ctx),
            Err(error) => {
                let window_id = ctx.window_id();
                ToastStack::handle(ctx).update(ctx, |toasts, ctx| {
                    toasts.add_ephemeral_toast(
                        DismissibleToast::error(error.to_string()),
                        window_id,
                        ctx,
                    )
                });
            }
        }
    }

    pub fn update_page(&mut self, page: AIFactPage, ctx: &mut ViewContext<Self>) {
        if let AIFactPage::RuleEditor { target } = &page {
            self.rule_editor_view
                .update(ctx, |editor, ctx| editor.set_target(target.clone(), ctx));
        }
        if let AIFactPage::MemoryEditor { target } = &page {
            self.memory_editor_view
                .update(ctx, |editor, ctx| editor.set_target(target.clone(), ctx));
        }
        self.current_page = page;
        self.focus(ctx);
        ctx.notify();
    }

    fn render_offline_banner(&self, appearance: &Appearance) -> Box<dyn Element> {
        Container::new(
            appearance
                .ui_builder()
                .wrappable_text(OFFLINE_TEXT, true)
                .with_style(style::description_text(appearance))
                .build()
                .finish(),
        )
        .with_background(appearance.theme().surface_2())
        .with_vertical_padding(4.)
        .with_margin_bottom(style::ITEM_BOTTOM_MARGIN)
        .finish()
    }

    fn render_knowledge_tabs(&self, appearance: &Appearance) -> Box<dyn Element> {
        let tab = |title: &str, page: AIFactPage, selected: bool, mouse_state: MouseStateHandle| {
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
                ctx.dispatch_typed_action(AIFactViewAction::UpdatePage(page.clone()))
            })
            .finish()
        };
        Container::new(
            Flex::row()
                .with_child(tab(
                    "Rules",
                    AIFactPage::Rules,
                    matches!(&self.current_page, AIFactPage::Rules),
                    self.rules_tab_mouse_state.clone(),
                ))
                .with_child(tab(
                    "Memory",
                    AIFactPage::Memory,
                    matches!(&self.current_page, AIFactPage::Memory),
                    self.memory_tab_mouse_state.clone(),
                ))
                .finish(),
        )
        .with_margin_bottom(style::SECTION_MARGIN)
        .finish()
    }
}

impl Entity for AIFactView {
    type Event = AIFactViewEvent;
}

impl View for AIFactView {
    fn ui_name() -> &'static str {
        "AIFactView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.focus(ctx);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let mut col = Flex::column().with_main_axis_size(MainAxisSize::Min);
        col.add_child(self.render_offline_banner(appearance));
        if matches!(&self.current_page, AIFactPage::Rules | AIFactPage::Memory) {
            col.add_child(self.render_knowledge_tabs(appearance));
        }
        match &self.current_page {
            AIFactPage::Rules => col.add_child(ChildView::new(&self.rule_view).finish()),
            AIFactPage::RuleEditor { .. } => {
                col.add_child(ChildView::new(&self.rule_editor_view).finish())
            }
            AIFactPage::Memory => col.add_child(ChildView::new(&self.memory_view).finish()),
            AIFactPage::MemoryEditor { .. } => {
                col.add_child(ChildView::new(&self.memory_editor_view).finish())
            }
        }
        ClippedScrollable::vertical(
            self.clipped_scroll_state.clone(),
            Align::new(
                Container::new(
                    ConstrainedBox::new(col.finish())
                        .with_max_width(style::PANE_WIDTH)
                        .finish(),
                )
                .with_uniform_padding(style::PANE_PADDING)
                .finish(),
            )
            .top_center()
            .finish(),
            ScrollbarWidth::Auto,
            appearance.theme().nonactive_ui_detail().into(),
            appearance.theme().active_ui_detail().into(),
            warpui::elements::Fill::None,
        )
        .finish()
    }
}

pub(super) fn truncate_display_text(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated = text
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push('…');
    truncated
}

impl TypedActionView for AIFactView {
    type Action = AIFactViewAction;

    fn handle_action(&mut self, action: &AIFactViewAction, ctx: &mut ViewContext<Self>) {
        match action {
            AIFactViewAction::AddRule => self.update_page(
                AIFactPage::RuleEditor {
                    target: RuleEditorTarget::New(RuleTarget::Global),
                },
                ctx,
            ),
            AIFactViewAction::UpdatePage(page) => self.update_page(page.clone(), ctx),
        }
    }
}

impl BackingView for AIFactView {
    type PaneHeaderOverflowMenuAction = AIFactViewAction;
    type CustomAction = ();
    type AssociatedData = ();

    fn handle_pane_header_overflow_menu_action(
        &mut self,
        action: &Self::PaneHeaderOverflowMenuAction,
        ctx: &mut warpui::ViewContext<Self>,
    ) {
        self.handle_action(action, ctx)
    }

    fn close(&mut self, ctx: &mut warpui::ViewContext<Self>) {
        ctx.emit(AIFactViewEvent::Pane(PaneEvent::Close));
    }

    fn focus_contents(&mut self, ctx: &mut warpui::ViewContext<Self>) {
        self.focus(ctx);
    }

    fn render_header_content(
        &self,
        _ctx: &view::HeaderRenderContext<'_>,
        _app: &AppContext,
    ) -> view::HeaderContent {
        view::HeaderContent::simple(HEADER_TEXT)
    }

    fn set_focus_handle(&mut self, focus_handle: PaneFocusHandle, _ctx: &mut ViewContext<Self>) {
        self.focus_handle = Some(focus_handle);
    }
}

#[cfg(test)]
mod local_rule_tests {
    use std::fs;

    use ai::project_context::local_rule_repository::{
        LocalRuleError, LocalRuleRepository, ProjectRuleFile,
    };
    use tempfile::tempdir;

    #[test]
    fn local_rule_add_edit_delete_uses_the_exact_managed_path() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("one");
        fs::create_dir(&root).unwrap();
        let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);
        let created = repository
            .create_project(&root, ProjectRuleFile::Warp, "one")
            .unwrap();
        let updated = repository
            .update(&created.path, &created.revision, "two")
            .unwrap();
        assert_eq!(
            updated.path,
            fs::canonicalize(&root).unwrap().join("WARP.md")
        );
        repository.delete(&updated.path, &updated.revision).unwrap();
        assert!(!updated.path.exists());
        let recreated = repository
            .create_project(&root, ProjectRuleFile::Warp, "three")
            .unwrap();
        assert_eq!(recreated.path, updated.path);
        assert_eq!(recreated.content, "three");
    }

    #[test]
    fn local_rule_conflict_does_not_overwrite_external_content() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("one");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("WARP.md"), "original").unwrap();
        let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);
        let opened = repository.read(&root.join("WARP.md")).unwrap();
        fs::write(root.join("WARP.md"), "external").unwrap();
        assert!(matches!(
            repository.update(&opened.path, &opened.revision, "draft"),
            Err(LocalRuleError::Conflict { .. })
        ));
        assert_eq!(
            fs::read_to_string(root.join("WARP.md")).unwrap(),
            "external"
        );
    }

    #[test]
    fn local_rule_read_failure_keeps_open_file_path_visible() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("one");
        fs::create_dir(&root).unwrap();
        let path = root.join("WARP.md");
        fs::write(&path, [0xff, 0xfe]).unwrap();
        let repository = LocalRuleRepository::new_for_test(Vec::new(), [root]);
        assert!(matches!(
            repository.read(&path),
            Err(LocalRuleError::InvalidUtf8 { .. })
        ));
        assert_eq!(path.file_name().unwrap(), "WARP.md");
    }

    #[test]
    fn local_rule_dirty_error_keeps_compare_and_swap_revision() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("one");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("WARP.md"), "original").unwrap();
        let mut repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);
        let opened = repository.read(&root.join("WARP.md")).unwrap();
        assert!(
            repository
                .update(&opened.path, &opened.revision, "draft")
                .is_ok()
        );
        assert!(matches!(
            repository.update(&opened.path, &opened.revision, "stale"),
            Err(LocalRuleError::Conflict { .. })
        ));
    }

    #[test]
    fn local_rule_watcher_refresh_preserves_both_managed_filenames() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("one");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("AGENTS.md"), "agents").unwrap();
        let repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);
        let surfaced = repository.surfaced_paths().cloned().collect::<Vec<_>>();
        let root = fs::canonicalize(root).unwrap();
        assert!(surfaced.contains(&root.join("WARP.md")));
        assert!(surfaced.contains(&root.join("AGENTS.md")));
    }

    #[test]
    fn local_rule_precedence_remains_file_based_without_a_provider() {
        let temp = tempdir().unwrap();
        let root = temp.path().join("one");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("AGENTS.md"), "agents").unwrap();
        fs::write(root.join("WARP.md"), "warp").unwrap();
        let repository = LocalRuleRepository::new_for_test(Vec::new(), [root.clone()]);
        assert_eq!(
            repository.read(&root.join("WARP.md")).unwrap().content,
            "warp"
        );
        assert_eq!(
            repository.read(&root.join("AGENTS.md")).unwrap().content,
            "agents"
        );
    }
}
