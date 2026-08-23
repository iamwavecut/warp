use std::path::PathBuf;

use warp_core::ui::appearance::Appearance;
use warpui::elements::{
    Align, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container, Flex,
    MainAxisSize, ParentElement, ScrollbarWidth,
};
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, FocusContext, ModelHandle, SingletonEntity, TypedActionView, View,
    ViewContext, ViewHandle,
};

use crate::ai::facts::view::rule_editor::RuleEditorTarget;
use crate::pane_group::focus_state::PaneFocusHandle;
use crate::pane_group::pane::view;
use crate::pane_group::{BackingView, PaneConfiguration, PaneEvent};

pub mod rule;
pub mod rule_editor;
mod style;
use rule::*;
use rule_editor::*;

const OFFLINE_TEXT: &str = "Local rules remain available without a provider or network.";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AIFactPage {
    Rules,
    RuleEditor { target: RuleEditorTarget },
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
        Self {
            pane_configuration,
            focus_handle: None,
            current_page: AIFactPage::default(),
            rule_view,
            rule_editor_view,
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

    pub fn update_page(&mut self, page: AIFactPage, ctx: &mut ViewContext<Self>) {
        if let AIFactPage::RuleEditor { target } = &page {
            self.rule_editor_view
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
        match &self.current_page {
            AIFactPage::Rules => col.add_child(ChildView::new(&self.rule_view).finish()),
            AIFactPage::RuleEditor { .. } => {
                col.add_child(ChildView::new(&self.rule_editor_view).finish())
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
