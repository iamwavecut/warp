use pathfinder_color::ColorU;
use warp_core::ui::{
    appearance::Appearance,
    builder::UiBuilder,
    color::{darken, lighten},
    theme::ColorScheme,
};
use warpui::{
    assets::asset_cache::AssetSource,
    elements::{
        Border, CacheOption, ConstrainedBox, Container, CornerRadius, CrossAxisAlignment, Fill,
        Flex, Image, MouseStateHandle, ParentElement, Radius,
    },
    fonts::Weight,
    ui_components::{
        button::ButtonVariant,
        components::{Coords, UiComponent, UiComponentStyles},
    },
    Action, Element,
};

use crate::themes::theme::ThemeKind;

pub const AUTH_MODAL_GAP: f32 = 16.;
const MODAL_CORNER_RADIUS: Radius = Radius::Pixels(8.);

pub fn action_button_color_and_variant(appearance: &Appearance) -> (ColorU, ButtonVariant) {
    let (button_color, button_variant) = match appearance.theme().name() {
        Some(name) if ThemeKind::Dark.matches(&name) => {
            (ColorU::new(0, 109, 168, 255), ButtonVariant::Basic)
        }
        Some(_) => (appearance.theme().accent().into(), ButtonVariant::Accent),
        None => (appearance.theme().accent().into(), ButtonVariant::Accent),
    };
    (button_color, button_variant)
}

pub fn render_offline_contents<A>(
    appearance: &Appearance,
    ui_builder: &UiBuilder,
    mouse_state_handle: MouseStateHandle,
    action: A,
) -> Box<dyn Element>
where
    A: Action + Clone,
{
    let disclaimer_color = appearance
        .theme()
        .sub_text_color(appearance.theme().background())
        .into();

    let disclaimer_styles = UiComponentStyles {
        font_color: Some(disclaimer_color),
        ..Default::default()
    };

    let text = "You are currently offline. An internet connection is required to use Warp for the first time.";

    let (button_color, button_variant) = action_button_color_and_variant(appearance);
    let button_styles = UiComponentStyles {
        font_size: Some(14.),
        font_family_id: Some(appearance.ui_font_family()),
        font_weight: Some(Weight::Bold),
        background: Some(Fill::Solid(button_color)),
        border_width: Some(2.),
        border_color: Some(Fill::Solid(ColorU::transparent_black())),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
        padding: Some(Coords {
            top: 0.,
            bottom: 0.,
            left: 8.,
            right: 8.,
        }),
        height: Some(40.),
        ..Default::default()
    };

    let hover_button_style = UiComponentStyles {
        border_color: Some(Fill::Solid(lighten(button_color))),
        ..button_styles
    };

    let click_button_style = UiComponentStyles {
        background: Some(Fill::Solid(darken(button_color))),
        ..hover_button_style
    };

    let button = ui_builder
        .button_with_custom_styles(
            button_variant,
            mouse_state_handle.clone(),
            button_styles,
            Some(hover_button_style),
            Some(click_button_style),
            None,
        )
        .with_centered_text_label("Learn more".into())
        .build()
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish();

    Flex::column()
        .with_child(
            Container::new(
                ui_builder
                    .paragraph(text)
                    .with_style(disclaimer_styles)
                    .build()
                    .finish(),
            )
            .with_margin_bottom(AUTH_MODAL_GAP)
            .finish(),
        )
        .with_child(button)
        .finish()
}

pub fn render_square_logo(appearance: &Appearance) -> Box<dyn Element> {
    let image_path = if appearance.theme().inferred_color_scheme() == ColorScheme::LightOnDark {
        "bundled/svg/warp-logo-light.svg"
    } else {
        "bundled/svg/warp-logo-dark.svg"
    };

    ConstrainedBox::new(
        Container::new(
            Image::new(
                AssetSource::Bundled { path: image_path },
                CacheOption::BySize,
            )
            .finish(),
        )
        .with_background(appearance.theme().surface_2())
        .with_corner_radius(CornerRadius::with_all(Radius::Pixels(10.)))
        .with_horizontal_padding(11.)
        .finish(),
    )
    .with_width(64.)
    .with_height(64.)
    .finish()
}

pub fn render_offline_info_overlay_body<A>(
    appearance: &Appearance,
    mouse_state_handle: MouseStateHandle,
    action: A,
) -> Box<dyn Element>
where
    A: Action + Clone,
{
    let header_styles = UiComponentStyles {
        font_family_id: Some(appearance.header_font_family()),
        font_color: Some(appearance.theme().active_ui_text_color().into()),
        font_size: Some(20.),
        font_weight: Some(Weight::Semibold),
        ..Default::default()
    };

    let body_text_color = appearance
        .theme()
        .sub_text_color(appearance.theme().background())
        .into();

    let body_text_styles = UiComponentStyles {
        font_color: Some(body_text_color),
        ..Default::default()
    };

    let paragraph_1 = "This local-first build works without a Warp account.";
    let paragraph_2 = "AI features use local or BYOK providers configured on this device.";
    let paragraph_3 =
        "No anonymous Warp account is created for hosted metering or cloud-object association.";

    Container::new(
        Flex::column()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_child(
                Container::new(render_square_logo(appearance))
                    .with_margin_bottom(AUTH_MODAL_GAP)
                    .finish(),
            )
            .with_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .span("Using Warp Offline")
                        .with_style(header_styles)
                        .build()
                        .finish(),
                )
                .with_margin_bottom(AUTH_MODAL_GAP)
                .finish(),
            )
            .with_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .paragraph(paragraph_1)
                        .with_style(body_text_styles)
                        .build()
                        .finish(),
                )
                .with_margin_bottom(4.)
                .finish(),
            )
            .with_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .paragraph(paragraph_2)
                        .with_style(body_text_styles)
                        .build()
                        .finish(),
                )
                .with_margin_bottom(4.)
                .finish(),
            )
            .with_child(
                Container::new(
                    appearance
                        .ui_builder()
                        .paragraph(paragraph_3)
                        .with_style(body_text_styles)
                        .build()
                        .finish(),
                )
                .with_margin_bottom(AUTH_MODAL_GAP)
                .finish(),
            )
            .with_child(render_close_overlay_button(
                appearance,
                appearance.ui_builder(),
                "Dismiss".into(),
                mouse_state_handle,
                action,
            ))
            .finish(),
    )
    .finish()
}

pub fn render_close_overlay_button<A>(
    appearance: &Appearance,
    ui_builder: &UiBuilder,
    label: String,
    mouse_state_handle: MouseStateHandle,
    action: A,
) -> Box<dyn Element>
where
    A: Action + Clone,
{
    let (button_color, button_variant) = action_button_color_and_variant(appearance);
    let button_styles = UiComponentStyles {
        font_size: Some(14.),
        font_family_id: Some(appearance.ui_font_family()),
        font_weight: Some(Weight::Bold),
        background: Some(Fill::Solid(button_color)),
        border_width: Some(2.),
        border_color: Some(Fill::Solid(ColorU::transparent_black())),
        border_radius: Some(CornerRadius::with_all(Radius::Pixels(4.))),
        padding: Some(Coords {
            top: 0.,
            bottom: 0.,
            left: 8.,
            right: 8.,
        }),
        height: Some(40.),
        ..Default::default()
    };

    let hover_button_style = UiComponentStyles {
        border_color: Some(Fill::Solid(lighten(button_color))),
        ..button_styles
    };

    let click_button_style = UiComponentStyles {
        background: Some(Fill::Solid(darken(button_color))),
        ..hover_button_style
    };

    ui_builder
        .button_with_custom_styles(
            button_variant,
            mouse_state_handle.clone(),
            button_styles,
            Some(hover_button_style),
            Some(click_button_style),
            None,
        )
        .with_centered_text_label(label)
        .build()
        .on_click(move |ctx, _, _| {
            ctx.dispatch_typed_action(action.clone());
        })
        .finish()
}

pub fn render_overlay(overlay_body: Box<dyn Element>, appearance: &Appearance) -> Box<dyn Element> {
    Container::new(overlay_body)
        .with_background(appearance.theme().surface_1())
        .with_border(Border::all(1.).with_border_fill(appearance.theme().outline()))
        .with_corner_radius(CornerRadius::with_all(MODAL_CORNER_RADIUS))
        .with_uniform_padding(32.)
        .finish()
}
