use warp_core::ui::appearance::Appearance;
use warp_core::ui::icons::Icon;
use warpui::{
    AppContext, Element, SingletonEntity as _,
    elements::{
        Border, Container, CornerRadius, CrossAxisAlignment, Flex, ParentElement, Radius, Text,
    },
    fonts::{Properties, Weight},
};

use crate::ai::custom_model_routers::{CustomModelRouter, CustomModelRouting};
use crate::ai::llms::{LLMId, LLMPreferences};

/// Render a bounded, non-flexible parse error card for the local router
/// editor. The card is intentionally usable inside an unbounded vertical
/// settings scroll container.
#[cfg(feature = "local_fs")]
pub fn render_router_error_card(
    file_name: impl Into<String>,
    error_message: impl Into<String>,
    appearance: &Appearance,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let error_fill = warp_core::ui::theme::Fill::Solid(theme.ui_error_color());
    let sub = theme.sub_text_color(theme.surface_2());
    let file_name = file_name.into();
    let error_message = error_message.into();

    let name_row = Flex::row()
        .with_cross_axis_alignment(CrossAxisAlignment::Center)
        .with_child(
            Container::new(Icon::AlertTriangle.to_warpui_icon(error_fill).finish())
                .with_margin_right(8.)
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

    Container::new(
        Flex::column()
            .with_child(Container::new(name_row).with_margin_bottom(6.).finish())
            .with_child(error_row)
            .finish(),
    )
    .with_background(theme.surface_2())
    .with_border(Border::new(1.).with_border_fill(error_fill))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
    .with_horizontal_padding(16.)
    .with_vertical_padding(10.)
    .finish()
}

/// Render a local router summary. Targets are resolved to the concrete custom
/// model names known to the local picker; no hosted metadata is consulted.
#[cfg(feature = "local_fs")]
pub fn render_router_card(
    router: &CustomModelRouter,
    appearance: &Appearance,
    app: &AppContext,
) -> Box<dyn Element> {
    let theme = appearance.theme();
    let sub_color = theme.sub_text_color(theme.surface_2());
    let type_label = match &router.routing {
        CustomModelRouting::Complexity(_) => "Complexity-based routing",
        CustomModelRouting::Prompt(_) => "Prompt-based routing",
    };
    let targets = router
        .all_targets()
        .into_iter()
        .enumerate()
        .map(|(index, target)| {
            let label = if index == 0 {
                "Default".to_owned()
            } else {
                format!("Target {index}")
            };
            let display = LLMPreferences::as_ref(app)
                .get_llm_info(&LLMId::from(target))
                .map(|info| info.display_name.clone())
                .unwrap_or_else(|| target.to_owned());
            Flex::row()
                .with_child(
                    Text::new(label, appearance.ui_font_family(), 12.)
                        .with_color(sub_color.into())
                        .finish(),
                )
                .with_child(
                    Container::new(
                        Text::new(display, appearance.ui_font_family(), 12.)
                            .with_color(theme.active_ui_text_color().into())
                            .finish(),
                    )
                    .with_margin_left(8.)
                    .finish(),
                )
                .finish()
        })
        .collect::<Vec<_>>();
    let mut body = Flex::column().with_spacing(4.).with_child(
        Text::new(type_label, appearance.ui_font_family(), 12.)
            .with_color(sub_color.into())
            .finish(),
    );
    for target in targets {
        body.add_child(target);
    }
    Container::new(
        Flex::column()
            .with_spacing(8.)
            .with_child(
                Text::new(
                    router.info.display_name.clone(),
                    appearance.ui_font_family(),
                    14.,
                )
                .with_style(Properties::default().weight(Weight::Medium))
                .with_color(theme.active_ui_text_color().into())
                .finish(),
            )
            .with_child(body.finish())
            .finish(),
    )
    .with_background(theme.surface_2())
    .with_border(Border::new(1.).with_border_fill(theme.outline()))
    .with_corner_radius(CornerRadius::with_all(Radius::Pixels(4.)))
    .with_horizontal_padding(16.)
    .with_vertical_padding(12.)
    .finish()
}

#[cfg(all(test, feature = "local_fs"))]
#[path = "custom_router_view_tests.rs"]
mod tests;
