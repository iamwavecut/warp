use crate::appearance::Appearance;
use crate::root_view::unthemed_window_border;

use crate::util::bindings::CustomAction;
use anyhow::{anyhow, Result};
use pathfinder_color::ColorU;
use pathfinder_geometry::vector::vec2f;
use url::Url;
use warp_core::errors::ErrorExt;
use warpui::elements::ChildAnchor;
use warpui::elements::Container;
use warpui::elements::Fill;
use warpui::elements::HighlightedHyperlink;
use warpui::elements::MouseStateHandle;
use warpui::elements::OffsetPositioning;
use warpui::elements::ParentAnchor;
use warpui::elements::ParentElement;
use warpui::elements::ParentOffsetBounds;
use warpui::elements::Stack;
use warpui::keymap::FixedBinding;
use warpui::AppContext;
use warpui::FocusContext;
use warpui::SingletonEntity;
use warpui::TypedActionView;

use crate::auth::auth_view_body::AuthViewBody;
use crate::modal::Modal;
use warpui::elements::ChildView;
use warpui::ui_components::components::{Coords, UiComponentStyles};
use warpui::{Element, Entity, View, ViewContext, ViewHandle};

use super::auth_manager::AuthManager;
use super::auth_manager::AuthManagerEvent;
use super::auth_view_body::AuthStep;
use super::auth_view_body::AuthViewBodyEvent;
use super::credentials::RefreshToken;
use super::login_failure_notification::{self, LoginFailureReason};
use super::UserUid;
use warpui::actions::StandardAction;

pub fn init(app: &mut AppContext) {
    use warpui::keymap::macros::*;

    app.register_fixed_bindings([
        // Bindings for paste require the StandardAction and CustomAction binding to work on all platforms.
        FixedBinding::custom(
            CustomAction::Paste,
            AuthViewAction::PasteAuthUrl,
            "Paste",
            id!(AuthView::ui_name()),
        ),
        FixedBinding::standard(
            StandardAction::Paste,
            AuthViewAction::PasteAuthUrl,
            id!(AuthView::ui_name()),
        ),
    ]);

    // For linux and Windows, default paste binding is ctrl+shift+v for PTY reasons.
    // This can be confusing for users in some cases (and we might want
    // to solve it in a more general way later). In the meantime, we
    // add a basic ctrl+v binding for the auth view, since there is no
    // terminal to interact with yet.
    #[cfg(any(target_os = "linux", target_os = "freebsd", target_os = "windows"))]
    app.register_fixed_bindings([FixedBinding::new(
        "cmdorctrl-v",
        AuthViewAction::PasteAuthUrl,
        id!(AuthView::ui_name()),
    )]);
}

#[derive(Clone, Debug)]
pub enum AuthViewAction {
    /// Triggered when the user attempts to paste something while the auth view
    /// modal is visible.
    PasteAuthUrl,
    DismissErrorNotification,
}

pub struct AuthView {
    auth_screen_modal: ViewHandle<Modal<AuthViewBody>>,

    // Reason for failing the most recent attempt to login, if any. When this is set, a
    // notification containing the reason's error message is shown to the user.
    pub last_login_failure_reason: Option<LoginFailureReason>,
    close_login_notification_mouse_state: MouseStateHandle,
    highlighted_hyperlink_state: HighlightedHyperlink,
    auth_view_variant: AuthViewVariant,
}

// `AuthRedirectPayload` is returned from the incoming redirect url.
#[derive(Debug, Clone)]
pub struct AuthRedirectPayload {
    pub refresh_token: RefreshToken,
    pub user_uid: Option<UserUid>,
    pub deleted_anonymous_user: Option<bool>,
    pub state: Option<String>,
}

impl AuthRedirectPayload {
    /// Remote auth redirects are disabled in this local-first build.
    pub fn from_url(url: Url) -> Result<Self> {
        let _ = url;
        Err(anyhow!(
            "Remote auth redirects are disabled in this local-first build"
        ))
    }

    /// Like [`from_url()`], except first parses the given [`raw_url`] into a [`Url`] struct.
    pub fn from_raw_url(raw_url: String) -> Result<Self> {
        match Url::parse(&raw_url) {
            Ok(parsed_url) => AuthRedirectPayload::from_url(parsed_url),
            Err(error) => Err(anyhow!(error)),
        }
    }
}

const MODAL_WIDTH: f32 = 352.;

#[derive(Clone, Copy, Debug)]
pub enum AuthViewVariant {
    Initial,
    RequireLoginCloseable,
    HitDriveObjectLimitCloseable,
    ShareRequirementCloseable,
}

impl AuthView {
    pub fn new(variant: AuthViewVariant, ctx: &mut ViewContext<Self>) -> Self {
        let auth_screen_view = ctx.add_typed_action_view(|ctx| AuthViewBody::new(variant, ctx));
        ctx.subscribe_to_view(&auth_screen_view, |me, _, event, ctx| match event {
            AuthViewBodyEvent::Close => me.close(ctx),
            AuthViewBodyEvent::SignUpButtonClicked => {
                me.dismiss_error_notification(ctx);
            }
            AuthViewBodyEvent::AuthTokenEntered(token) => {
                me.last_login_failure_reason = None;
                me.handle_pasted_auth_url(token.clone(), ctx);
                ctx.notify();
            }
            AuthViewBodyEvent::LoginLaterClicked => {
                me.handle_login_later(ctx);
            }
        });

        let auth_screen_modal = ctx.add_typed_action_view(|ctx| {
            Modal::new(None, auth_screen_view, ctx)
                .with_body_style(UiComponentStyles {
                    padding: Some(Coords::uniform(0.)),
                    ..Default::default()
                })
                .with_modal_style(UiComponentStyles {
                    width: Some(MODAL_WIDTH),
                    border_color: Some(Fill::from(ColorU::transparent_black())), // override default modal border color
                    ..Default::default()
                })
        });

        let auth_manager = AuthManager::handle(ctx);
        ctx.subscribe_to_model(&auth_manager, |me, _, event, ctx| {
            me.handle_auth_manager_event(event, ctx);
        });

        Self {
            auth_screen_modal,
            last_login_failure_reason: None,
            close_login_notification_mouse_state: Default::default(),
            highlighted_hyperlink_state: Default::default(),
            auth_view_variant: variant,
        }
    }

    pub fn set_variant(&mut self, ctx: &mut ViewContext<Self>, variant: AuthViewVariant) {
        self.auth_view_variant = variant;
        self.update_auth_body(
            ctx,
            |body: &mut AuthViewBody, _: &mut ViewContext<'_, AuthViewBody>| {
                body.set_variant(variant)
            },
        );
    }

    fn set_auth_step(&mut self, ctx: &mut ViewContext<Self>, step: AuthStep) {
        self.update_auth_body(
            ctx,
            |body: &mut AuthViewBody, _: &mut ViewContext<'_, AuthViewBody>| {
                body.set_auth_step(step)
            },
        );
    }

    pub fn skip_to_browser_open_step(&mut self, ctx: &mut ViewContext<Self>) {
        self.set_auth_step(ctx, AuthStep::BrowserOpen);
    }

    fn focus(&self, ctx: &mut ViewContext<Self>) {
        ctx.focus(&self.auth_screen_modal);
        ctx.notify();
    }

    fn dismiss_error_notification(&mut self, ctx: &mut ViewContext<Self>) {
        self.last_login_failure_reason = None;
        ctx.notify();
    }

    fn close(&mut self, ctx: &mut ViewContext<Self>) {
        self.update_auth_body(
            ctx,
            |body: &mut AuthViewBody, ctx: &mut ViewContext<'_, AuthViewBody>| {
                body.reset_login_screen(ctx)
            },
        );
        self.dismiss_error_notification(ctx);
        ctx.emit(AuthViewEvent::Close);
    }

    /// Ignores pasted remote auth URLs. Local-first builds do not accept hosted auth tokens.
    fn handle_pasted_auth_url(&mut self, pasted_url: String, ctx: &mut ViewContext<Self>) {
        let _ = pasted_url;
        self.set_auth_token_input_editable(false, ctx);
        log::info!("Ignoring pasted remote auth URL in local workflow");
        AuthManager::handle(ctx).update(ctx, |_, ctx| {
            ctx.emit(AuthManagerEvent::SkippedLogin);
        });
    }

    fn set_auth_token_input_editable(&mut self, is_editable: bool, ctx: &mut ViewContext<Self>) {
        self.update_auth_body(ctx, |body, ctx| body.set_input_editable(is_editable, ctx))
    }

    fn update_auth_body<S, F>(&mut self, ctx: &mut ViewContext<Self>, cb: F) -> S
    where
        F: FnOnce(&mut AuthViewBody, &mut ViewContext<'_, AuthViewBody>) -> S,
    {
        self.auth_screen_modal
            .update(ctx, |modal, ctx| modal.body().update(ctx, cb))
    }

    pub fn handle_login_later(&mut self, ctx: &mut ViewContext<Self>) {
        AuthManager::handle(ctx).update(ctx, |_, ctx| {
            ctx.emit(AuthManagerEvent::SkippedLogin);
        });
    }

    fn handle_auth_manager_event(&mut self, event: &AuthManagerEvent, ctx: &mut ViewContext<Self>) {
        match event {
            AuthManagerEvent::AuthComplete | AuthManagerEvent::SkippedLogin => {
                self.close(ctx);
            }
            AuthManagerEvent::AuthFailed(err) => {
                if err.is_actionable() {
                    log::error!("Failed to log in user: {err:#}");
                }

                self.last_login_failure_reason =
                    Some(LoginFailureReason::FailedUserAuthentication);

                self.set_auth_token_input_editable(true, ctx);
            }
            _ => {}
        }
        ctx.notify();
    }
}

#[derive(PartialEq, Eq)]
pub enum AuthViewEvent {
    Close,
}

impl Entity for AuthView {
    type Event = AuthViewEvent;
}

impl View for AuthView {
    fn ui_name() -> &'static str {
        "AuthView"
    }

    fn on_focus(&mut self, focus_ctx: &FocusContext, ctx: &mut ViewContext<Self>) {
        if focus_ctx.is_self_focused() {
            self.focus(ctx);
        }
    }

    fn render(&self, ctx: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(ctx);
        let mut stack = Stack::new();
        stack.add_child(ChildView::new(&self.auth_screen_modal).finish());

        if let Some(login_failure_reason) = &self.last_login_failure_reason {
            let login_failure_notification = login_failure_notification::render(
                login_failure_reason,
                self.close_login_notification_mouse_state.clone(),
                self.highlighted_hyperlink_state.clone(),
                AuthViewAction::DismissErrorNotification,
                ctx,
            );
            stack.add_positioned_overlay_child(
                login_failure_notification,
                OffsetPositioning::offset_from_parent(
                    vec2f(0., 40.),
                    ParentOffsetBounds::ParentBySize,
                    ParentAnchor::TopMiddle,
                    ChildAnchor::TopMiddle,
                ),
            );
        }

        let background_color = match self.auth_view_variant {
            AuthViewVariant::Initial => appearance.theme().background().into(),
            AuthViewVariant::RequireLoginCloseable
            | AuthViewVariant::HitDriveObjectLimitCloseable
            | AuthViewVariant::ShareRequirementCloseable => ColorU::transparent_black(),
        };

        // TODO(liam): use theme colors for background and window border
        Container::new(stack.finish())
            .with_background_color(background_color)
            .with_corner_radius(ctx.windows().window_corner_radius())
            .with_border(unthemed_window_border())
            .finish()
    }
}

impl TypedActionView for AuthView {
    type Action = AuthViewAction;

    fn handle_action(&mut self, action: &AuthViewAction, ctx: &mut ViewContext<Self>) {
        match action {
            AuthViewAction::PasteAuthUrl => {
                self.last_login_failure_reason = None;
                self.update_auth_body(ctx, |body, ctx| body.handle_paste(ctx));

                ctx.notify();
            }
            AuthViewAction::DismissErrorNotification => {
                self.dismiss_error_notification(ctx);
            }
        }
    }
}
