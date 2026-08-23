use ai::project_context::local_rule_repository::LocalRule;
use warp_core::ui::appearance::Appearance;
use warp_core::ui::theme::color::internal_colors;
use warp_editor::editor::NavigationKey;
use warpui::elements::{
    Border, ChildView, ClippedScrollStateHandle, ClippedScrollable, ConstrainedBox, Container,
    CornerRadius, CrossAxisAlignment, Flex, MainAxisAlignment, MainAxisSize, ParentElement, Radius,
    ScrollbarWidth,
};
use warpui::platform::Cursor;
use warpui::ui_components::components::UiComponent;
use warpui::{
    AppContext, Element, Entity, FocusContext, SingletonEntity, TypedActionView, View, ViewContext,
    ViewHandle,
};

use super::{RuleTarget, style};
use crate::editor::{
    EditorOptions, EditorView, EnterAction, EnterSettings, Event as EditorEvent,
    PropagateAndNoOpNavigationKeys, TextOptions,
};
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;
use crate::view_components::action_button::{ActionButton, DangerSecondaryTheme, PrimaryTheme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleEditorTarget {
    New(RuleTarget),
    Existing(LocalRule),
}

#[derive(Debug, Clone)]
pub enum RuleEditorViewEvent {
    Back,
    Save {
        target: RuleEditorTarget,
        content: String,
    },
    Delete {
        rule: LocalRule,
    },
}

#[derive(Debug, Clone)]
pub enum RuleEditorViewAction {
    Back,
    Save,
    Delete,
    ConfirmDelete,
    ConfirmDiscard,
}

pub struct RuleEditorView {
    target: Option<RuleEditorTarget>,
    initial_content: String,
    content_editor: ViewHandle<EditorView>,
    save_button: ViewHandle<ActionButton>,
    delete_button: ViewHandle<ActionButton>,
    confirm_delete_button: ViewHandle<ActionButton>,
    discard_button: ViewHandle<ActionButton>,
    back_button: warpui::elements::MouseStateHandle,
    clipped_scroll_state: ClippedScrollStateHandle,
    confirm_delete: bool,
    confirm_discard: bool,
}

impl RuleEditorView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let appearance = Appearance::as_ref(ctx);
        let text = TextOptions {
            font_size_override: Some(style::TEXT_FONT_SIZE),
            font_family_override: Some(appearance.ui_font_family()),
            ..Default::default()
        };
        let content_editor = ctx.add_typed_action_view(|ctx| {
            let mut editor = EditorView::new(
                EditorOptions {
                    text,
                    soft_wrap: true,
                    autogrow: true,
                    propagate_and_no_op_vertical_navigation_keys:
                        PropagateAndNoOpNavigationKeys::Always,
                    supports_vim_mode: false,
                    single_line: false,
                    enter_settings: EnterSettings {
                        shift_enter: EnterAction::InsertNewLineIfMultiLine,
                        enter: EnterAction::InsertNewLineIfMultiLine,
                        alt_enter: EnterAction::InsertNewLineIfMultiLine,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ctx,
            );
            editor.set_placeholder_text("Write the local Markdown rule", ctx);
            editor
        });
        ctx.subscribe_to_view(&content_editor, |me, _, event, ctx| {
            me.handle_editor_event(event, ctx)
        });

        let save_button = ctx.add_typed_action_view(|ctx| {
            let mut button = ActionButton::new("Save", PrimaryTheme)
                .with_icon(Icon::Check)
                .on_click(|ctx| ctx.dispatch_typed_action(RuleEditorViewAction::Save));
            button.set_disabled(true, ctx);
            button
        });
        let delete_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Delete rule", DangerSecondaryTheme)
                .with_icon(Icon::Trash)
                .on_click(|ctx| ctx.dispatch_typed_action(RuleEditorViewAction::Delete))
        });
        let confirm_delete_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Confirm delete", DangerSecondaryTheme)
                .with_icon(Icon::Trash)
                .on_click(|ctx| ctx.dispatch_typed_action(RuleEditorViewAction::ConfirmDelete))
        });
        let discard_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Discard changes", DangerSecondaryTheme)
                .on_click(|ctx| ctx.dispatch_typed_action(RuleEditorViewAction::ConfirmDiscard))
        });

        Self {
            target: None,
            initial_content: String::new(),
            content_editor,
            save_button,
            delete_button,
            confirm_delete_button,
            discard_button,
            back_button: Default::default(),
            clipped_scroll_state: Default::default(),
            confirm_delete: false,
            confirm_discard: false,
        }
    }

    pub fn set_target(&mut self, target: RuleEditorTarget, ctx: &mut ViewContext<Self>) {
        let content = match &target {
            RuleEditorTarget::New(_) => String::new(),
            RuleEditorTarget::Existing(rule) => rule.content.clone(),
        };
        self.initial_content = content.clone();
        self.target = Some(target);
        self.confirm_delete = false;
        self.confirm_discard = false;
        self.content_editor.update(ctx, |editor, ctx| {
            editor.set_buffer_text(&content, ctx);
        });
        self.update_save_button(ctx);
        ctx.notify();
    }

    fn update_save_button(&self, ctx: &mut ViewContext<Self>) {
        let content = self.content_editor.as_ref(ctx).buffer_text(ctx);
        let disabled = content.trim().is_empty() || content == self.initial_content;
        self.save_button
            .update(ctx, |button, ctx| button.set_disabled(disabled, ctx));
    }

    fn is_dirty(&self, ctx: &mut ViewContext<Self>) -> bool {
        self.content_editor.as_ref(ctx).buffer_text(ctx) != self.initial_content
    }

    fn handle_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
            EditorEvent::Focused => {}
            EditorEvent::Navigate(NavigationKey::Up) => {
                self.content_editor
                    .update(ctx, |editor, ctx| editor.move_up(ctx));
            }
            EditorEvent::Navigate(NavigationKey::Down) => {
                self.content_editor
                    .update(ctx, |editor, ctx| editor.move_down(ctx));
            }
            EditorEvent::Edited(_) => self.update_save_button(ctx),
            _ => {}
        }
    }

    fn render_back_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        let button = icon_button(appearance, Icon::ArrowLeft, false, self.back_button.clone());
        Container::new(
            button
                .build()
                .on_click(|ctx, _, _| ctx.dispatch_typed_action(RuleEditorViewAction::Back))
                .with_cursor(Cursor::PointingHand)
                .finish(),
        )
        .with_margin_right(style::ICON_MARGIN)
        .finish()
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let title = match &self.target {
            Some(RuleEditorTarget::New(_)) => "Add local rule",
            Some(RuleEditorTarget::Existing(_)) => "Edit local rule",
            None => "Local rule",
        };
        Container::new(
            Flex::row()
                .with_main_axis_size(MainAxisSize::Max)
                .with_main_axis_alignment(MainAxisAlignment::SpaceBetween)
                .with_cross_axis_alignment(CrossAxisAlignment::Center)
                .with_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(self.render_back_button(appearance))
                        .with_child(
                            appearance
                                .ui_builder()
                                .wrappable_text(title, true)
                                .with_style(style::header_text())
                                .build()
                                .finish(),
                        )
                        .finish(),
                )
                .with_child(
                    Container::new(ChildView::new(&self.save_button).finish())
                        .with_margin_left(style::SECTION_MARGIN)
                        .finish(),
                )
                .finish(),
        )
        .with_margin_bottom(style::ITEM_BOTTOM_MARGIN)
        .finish()
    }

    fn render_path(&self, appearance: &Appearance) -> Box<dyn Element> {
        let path = self.target.as_ref().map(|target| match target {
            RuleEditorTarget::New(target) => target.display_path(),
            RuleEditorTarget::Existing(rule) => rule.path.clone(),
        });
        Container::new(
            appearance
                .ui_builder()
                .wrappable_text(
                    path.map(|path| path.to_string_lossy().to_string())
                        .unwrap_or_else(|| "No rule selected".to_string()),
                    true,
                )
                .with_style(style::fact_project_based_row_text(appearance))
                .build()
                .finish(),
        )
        .with_margin_bottom(style::SECTION_MARGIN)
        .finish()
    }

    fn render_editor(&self, appearance: &Appearance) -> Box<dyn Element> {
        ConstrainedBox::new(
            Container::new(
                ClippedScrollable::vertical(
                    self.clipped_scroll_state.clone(),
                    ConstrainedBox::new(ChildView::new(&self.content_editor).finish())
                        .with_min_height(style::EDITOR_MIN_HEIGHT)
                        .finish(),
                    ScrollbarWidth::Auto,
                    appearance.theme().nonactive_ui_detail().into(),
                    appearance.theme().active_ui_detail().into(),
                    warpui::elements::Fill::None,
                )
                .finish(),
            )
            .with_background(appearance.theme().surface_2())
            .with_border(
                Border::all(1.).with_border_color(internal_colors::neutral_4(appearance.theme())),
            )
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
            .with_margin_bottom(style::ITEM_BOTTOM_MARGIN)
            .with_padding_left(style::EDITOR_HORIZONTAL_PADDING)
            .with_vertical_padding(style::EDITOR_VERTICAL_PADDING)
            .finish(),
        )
        .with_max_height(style::EDITOR_MAX_HEIGHT)
        .finish()
    }
}

impl Entity for RuleEditorView {
    type Event = RuleEditorViewEvent;
}

impl View for RuleEditorView {
    fn ui_name() -> &'static str {
        "RuleEditorView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.content_editor);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let mut col = Flex::column()
            .with_child(self.render_header(appearance))
            .with_child(self.render_path(appearance))
            .with_child(self.render_editor(appearance));

        if let Some(RuleEditorTarget::Existing(rule)) = &self.target {
            if rule.writable {
                if self.confirm_delete {
                    col.add_child(
                        Flex::row()
                            .with_cross_axis_alignment(CrossAxisAlignment::Center)
                            .with_child(
                                appearance
                                    .ui_builder()
                                    .wrappable_text("Delete this exact local file?", true)
                                    .build()
                                    .finish(),
                            )
                            .with_child(ChildView::new(&self.confirm_delete_button).finish())
                            .finish(),
                    );
                } else {
                    col.add_child(ChildView::new(&self.delete_button).finish());
                }
            } else {
                col.add_child(
                    appearance
                        .ui_builder()
                        .wrappable_text("This file is read-only.", true)
                        .with_style(style::fact_project_based_row_text(appearance))
                        .build()
                        .finish(),
                );
            }
        }
        if self.confirm_discard {
            col.add_child(
                Flex::row()
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(
                        appearance
                            .ui_builder()
                            .wrappable_text("Discard unsaved changes?", true)
                            .build()
                            .finish(),
                    )
                    .with_child(ChildView::new(&self.discard_button).finish())
                    .finish(),
            );
        }
        col.finish()
    }
}

impl TypedActionView for RuleEditorView {
    type Action = RuleEditorViewAction;

    fn handle_action(&mut self, action: &RuleEditorViewAction, ctx: &mut ViewContext<Self>) {
        match action {
            RuleEditorViewAction::Back => {
                if self.is_dirty(ctx) {
                    self.confirm_discard = true;
                    ctx.notify();
                } else {
                    ctx.emit(RuleEditorViewEvent::Back);
                }
            }
            RuleEditorViewAction::ConfirmDiscard => ctx.emit(RuleEditorViewEvent::Back),
            RuleEditorViewAction::Save => {
                let Some(target) = &self.target else {
                    return;
                };
                let content = self.content_editor.as_ref(ctx).buffer_text(ctx);
                if !content.trim().is_empty() && content != self.initial_content {
                    ctx.emit(RuleEditorViewEvent::Save {
                        target: target.clone(),
                        content,
                    });
                }
            }
            RuleEditorViewAction::Delete => {
                self.confirm_delete = true;
                ctx.notify();
            }
            RuleEditorViewAction::ConfirmDelete => {
                if let Some(RuleEditorTarget::Existing(rule)) = &self.target {
                    ctx.emit(RuleEditorViewEvent::Delete { rule: rule.clone() });
                }
            }
        }
    }
}
