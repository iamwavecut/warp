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

use super::style;
use crate::ai::facts::local_memory::{LocalMemoryRecord, LocalMemoryScope};
use crate::editor::{
    EditorOptions, EditorView, EnterAction, EnterSettings, Event as EditorEvent,
    PropagateAndNoOpNavigationKeys, SingleLineEditorOptions, TextOptions,
};
use crate::ui_components::buttons::icon_button;
use crate::ui_components::icons::Icon;
use crate::view_components::action_button::{ActionButton, DangerSecondaryTheme, PrimaryTheme};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemoryEditorTarget {
    New(LocalMemoryScope),
    Existing(LocalMemoryRecord),
}

#[derive(Debug, Clone)]
pub enum MemoryEditorViewEvent {
    Back,
    Save {
        target: MemoryEditorTarget,
        title: String,
        content: String,
    },
    Delete {
        memory: LocalMemoryRecord,
    },
}

#[derive(Debug, Clone)]
pub enum MemoryEditorViewAction {
    Back,
    Save,
    Delete,
    ConfirmDelete,
    ConfirmDiscard,
}

pub struct MemoryEditorView {
    target: Option<MemoryEditorTarget>,
    initial_title: String,
    initial_content: String,
    title_editor: ViewHandle<EditorView>,
    content_editor: ViewHandle<EditorView>,
    save_button: ViewHandle<ActionButton>,
    delete_button: ViewHandle<ActionButton>,
    confirm_delete_button: ViewHandle<ActionButton>,
    discard_button: ViewHandle<ActionButton>,
    back_button: warpui::elements::MouseStateHandle,
    title_scroll_state: ClippedScrollStateHandle,
    content_scroll_state: ClippedScrollStateHandle,
    confirm_delete: bool,
    confirm_discard: bool,
}

impl MemoryEditorView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let appearance = Appearance::as_ref(ctx);
        let title_editor = {
            let options = SingleLineEditorOptions {
                text: TextOptions::ui_text(None, appearance),
                propagate_and_no_op_vertical_navigation_keys:
                    PropagateAndNoOpNavigationKeys::Always,
                ..Default::default()
            };
            ctx.add_typed_action_view(|ctx| EditorView::single_line(options, ctx))
        };
        title_editor.update(ctx, |editor, ctx| {
            editor.set_placeholder_text("Short memory title", ctx)
        });

        let content_editor = ctx.add_typed_action_view(|ctx| {
            let mut editor = EditorView::new(
                EditorOptions {
                    text: TextOptions {
                        font_size_override: Some(style::TEXT_FONT_SIZE),
                        font_family_override: Some(Appearance::as_ref(ctx).ui_font_family()),
                        ..Default::default()
                    },
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
            editor.set_placeholder_text("Write a fact or preference to remember", ctx);
            editor
        });

        let save_button = ctx.add_typed_action_view(|ctx| {
            let mut button = ActionButton::new("Save", PrimaryTheme)
                .with_icon(Icon::Check)
                .on_click(|ctx| ctx.dispatch_typed_action(MemoryEditorViewAction::Save));
            button.set_disabled(true, ctx);
            button
        });
        let delete_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Delete memory", DangerSecondaryTheme)
                .with_icon(Icon::Trash)
                .on_click(|ctx| ctx.dispatch_typed_action(MemoryEditorViewAction::Delete))
        });
        let confirm_delete_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Confirm delete", DangerSecondaryTheme)
                .with_icon(Icon::Trash)
                .on_click(|ctx| ctx.dispatch_typed_action(MemoryEditorViewAction::ConfirmDelete))
        });
        let discard_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Discard changes", DangerSecondaryTheme)
                .on_click(|ctx| ctx.dispatch_typed_action(MemoryEditorViewAction::ConfirmDiscard))
        });

        let view = Self {
            target: None,
            initial_title: String::new(),
            initial_content: String::new(),
            title_editor,
            content_editor,
            save_button,
            delete_button,
            confirm_delete_button,
            discard_button,
            back_button: Default::default(),
            title_scroll_state: Default::default(),
            content_scroll_state: Default::default(),
            confirm_delete: false,
            confirm_discard: false,
        };
        for editor in [view.title_editor.clone(), view.content_editor.clone()] {
            ctx.subscribe_to_view(&editor, |me, _, event, ctx| {
                me.handle_editor_event(event, ctx)
            });
        }
        view
    }

    pub fn set_target(&mut self, target: MemoryEditorTarget, ctx: &mut ViewContext<Self>) {
        let (title, content) = match &target {
            MemoryEditorTarget::New(_) => (String::new(), String::new()),
            MemoryEditorTarget::Existing(memory) => (memory.title.clone(), memory.content.clone()),
        };
        self.initial_title = title.clone();
        self.initial_content = content.clone();
        self.target = Some(target);
        self.confirm_delete = false;
        self.confirm_discard = false;
        self.title_editor
            .update(ctx, |editor, ctx| editor.set_buffer_text(&title, ctx));
        self.content_editor
            .update(ctx, |editor, ctx| editor.set_buffer_text(&content, ctx));
        self.update_save_button(ctx);
        ctx.notify();
    }

    fn update_save_button(&self, ctx: &mut ViewContext<Self>) {
        let title = self.title_editor.as_ref(ctx).buffer_text(ctx);
        let content = self.content_editor.as_ref(ctx).buffer_text(ctx);
        let disabled = title.trim().is_empty()
            || content.trim().is_empty()
            || (title == self.initial_title && content == self.initial_content);
        self.save_button
            .update(ctx, |button, ctx| button.set_disabled(disabled, ctx));
    }

    fn is_dirty(&self, ctx: &mut ViewContext<Self>) -> bool {
        self.title_editor.as_ref(ctx).buffer_text(ctx) != self.initial_title
            || self.content_editor.as_ref(ctx).buffer_text(ctx) != self.initial_content
    }

    fn handle_editor_event(&mut self, event: &EditorEvent, ctx: &mut ViewContext<Self>) {
        match event {
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

    fn scope(&self) -> Option<&LocalMemoryScope> {
        match self.target.as_ref()? {
            MemoryEditorTarget::New(scope) => Some(scope),
            MemoryEditorTarget::Existing(memory) => Some(&memory.scope),
        }
    }

    fn render_back_button(&self, appearance: &Appearance) -> Box<dyn Element> {
        let button = icon_button(appearance, Icon::ArrowLeft, false, self.back_button.clone());
        Container::new(
            button
                .build()
                .on_click(|ctx, _, _| ctx.dispatch_typed_action(MemoryEditorViewAction::Back))
                .with_cursor(Cursor::PointingHand)
                .finish(),
        )
        .with_margin_right(style::ICON_MARGIN)
        .finish()
    }

    fn render_header(&self, appearance: &Appearance) -> Box<dyn Element> {
        let title = match self.target.as_ref() {
            Some(MemoryEditorTarget::New(_)) => "Add local memory",
            Some(MemoryEditorTarget::Existing(_)) => "Edit local memory",
            None => "Local memory",
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
                .with_child(ChildView::new(&self.save_button).finish())
                .finish(),
        )
        .with_margin_bottom(style::ITEM_BOTTOM_MARGIN)
        .finish()
    }

    fn render_editor(
        &self,
        editor: &ViewHandle<EditorView>,
        scroll_state: ClippedScrollStateHandle,
        min_height: f32,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        Container::new(
            ClippedScrollable::vertical(
                scroll_state,
                ConstrainedBox::new(ChildView::new(editor).finish())
                    .with_min_height(min_height)
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
        .finish()
    }
}

impl Entity for MemoryEditorView {
    type Event = MemoryEditorViewEvent;
}

impl View for MemoryEditorView {
    fn ui_name() -> &'static str {
        "MemoryEditorView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            ctx.focus(&self.title_editor);
        }
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let mut column = Flex::column()
            .with_child(self.render_header(appearance))
            .with_child(
                appearance
                    .ui_builder()
                    .wrappable_text(
                        self.scope()
                            .map(LocalMemoryScope::display_name)
                            .unwrap_or_else(|| "No scope selected".to_string()),
                        true,
                    )
                    .with_style(style::fact_project_based_row_text(appearance))
                    .build()
                    .finish(),
            )
            .with_child(self.render_editor(
                &self.title_editor,
                self.title_scroll_state.clone(),
                44.,
                appearance,
            ))
            .with_child(self.render_editor(
                &self.content_editor,
                self.content_scroll_state.clone(),
                style::EDITOR_MIN_HEIGHT,
                appearance,
            ));

        if matches!(self.target, Some(MemoryEditorTarget::Existing(_))) {
            if self.confirm_delete {
                column.add_child(
                    Flex::row()
                        .with_cross_axis_alignment(CrossAxisAlignment::Center)
                        .with_child(
                            appearance
                                .ui_builder()
                                .wrappable_text("Delete this local memory?", true)
                                .build()
                                .finish(),
                        )
                        .with_child(ChildView::new(&self.confirm_delete_button).finish())
                        .finish(),
                );
            } else {
                column.add_child(ChildView::new(&self.delete_button).finish());
            }
        }
        if self.confirm_discard {
            column.add_child(
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
        column.finish()
    }
}

impl TypedActionView for MemoryEditorView {
    type Action = MemoryEditorViewAction;

    fn handle_action(&mut self, action: &MemoryEditorViewAction, ctx: &mut ViewContext<Self>) {
        match action {
            MemoryEditorViewAction::Back => {
                if self.is_dirty(ctx) {
                    self.confirm_discard = true;
                    ctx.notify();
                } else {
                    ctx.emit(MemoryEditorViewEvent::Back);
                }
            }
            MemoryEditorViewAction::ConfirmDiscard => ctx.emit(MemoryEditorViewEvent::Back),
            MemoryEditorViewAction::Save => {
                let Some(target) = &self.target else {
                    return;
                };
                let title = self.title_editor.as_ref(ctx).buffer_text(ctx);
                let content = self.content_editor.as_ref(ctx).buffer_text(ctx);
                if !title.trim().is_empty() && !content.trim().is_empty() {
                    ctx.emit(MemoryEditorViewEvent::Save {
                        target: target.clone(),
                        title,
                        content,
                    });
                }
            }
            MemoryEditorViewAction::Delete => {
                self.confirm_delete = true;
                ctx.notify();
            }
            MemoryEditorViewAction::ConfirmDelete => {
                if let Some(MemoryEditorTarget::Existing(memory)) = &self.target {
                    ctx.emit(MemoryEditorViewEvent::Delete {
                        memory: memory.clone(),
                    });
                }
            }
        }
    }
}
