//! Local named-agent management embedded in the agent-management surface.
//!
//! This view deliberately exposes only the non-secret bundle summary. The
//! YAML document remains the source of truth and is opened for create/edit;
//! prompts and secret references are never copied into the list or details
//! text.

use uuid::Uuid;
use warp_cli::agent::Harness;
use warp_core::ui::icons::Icon;
use warpui::{
    AppContext, Element, Entity, SingletonEntity, TypedActionView, View, ViewContext, ViewHandle,
    elements::{
        Border, ChildView, Container, CornerRadius, CrossAxisAlignment, Expanded, Flex,
        ParentElement, Radius, Text,
    },
    fonts::{Properties, Weight},
};

use crate::ai::local_named_agents::{
    LocalNamedAgentRepository, NamedAgentBundle, NamedAgentFileError, NamedAgentRecord,
};
use crate::appearance::Appearance;
use crate::view_components::action_button::{
    ActionButton, ButtonSize, DangerNakedTheme, NakedTheme, PrimaryTheme, SecondaryTheme,
};
use crate::warp_managed_paths_watcher::{
    WarpManagedPathsWatcher, WarpManagedPathsWatcherEvent, repository_update_touches_prefix,
};
use crate::workspace::WorkspaceAction;

const CARD_RADIUS: f32 = 4.;
const CARD_PADDING: f32 = 10.;

#[derive(Debug, Clone, PartialEq)]
pub enum LocalNamedAgentsAction {
    Create,
    Reload,
    Select(Uuid),
    Edit(Uuid),
    Run(Uuid),
    AskDelete(Uuid),
    ConfirmDelete,
    CancelDelete,
}

struct NamedAgentRowHandles {
    select: ViewHandle<ActionButton>,
    edit: ViewHandle<ActionButton>,
    run: ViewHandle<ActionButton>,
    delete: ViewHandle<ActionButton>,
}

struct PendingDelete {
    id: Uuid,
    revision: String,
    name: String,
    confirm: ViewHandle<ActionButton>,
    cancel: ViewHandle<ActionButton>,
}

/// A local-only list and details surface for persisted named agents.
pub struct LocalNamedAgentsView {
    records: Vec<NamedAgentRecord>,
    errors: Vec<NamedAgentFileError>,
    rows: Vec<NamedAgentRowHandles>,
    selected_id: Option<Uuid>,
    pending_delete: Option<PendingDelete>,
    operation_error: Option<String>,
    create_button: ViewHandle<ActionButton>,
    reload_button: ViewHandle<ActionButton>,
}

impl LocalNamedAgentsView {
    pub fn new(ctx: &mut ViewContext<Self>) -> Self {
        let create_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Create agent", PrimaryTheme)
                .with_icon(Icon::Plus)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(LocalNamedAgentsAction::Create))
        });
        let reload_button = ctx.add_typed_action_view(|_| {
            ActionButton::new("Reload", NakedTheme)
                .with_icon(Icon::Refresh)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(LocalNamedAgentsAction::Reload))
        });

        let mut view = Self {
            records: Vec::new(),
            errors: Vec::new(),
            rows: Vec::new(),
            selected_id: None,
            pending_delete: None,
            operation_error: None,
            create_button,
            reload_button,
        };
        let watched_directory = Self::repository().directory().to_path_buf();
        ctx.subscribe_to_model(
            &WarpManagedPathsWatcher::handle(ctx),
            move |me, _, event, ctx| {
                let WarpManagedPathsWatcherEvent::FilesChanged(update) = event;
                if repository_update_touches_prefix(update, &watched_directory) {
                    me.reload(ctx);
                }
            },
        );
        view.reload(ctx);
        view
    }

    fn repository() -> LocalNamedAgentRepository {
        LocalNamedAgentRepository::for_user()
    }

    fn reload(&mut self, ctx: &mut ViewContext<Self>) {
        match Self::repository().list_with_errors() {
            Ok(list) => {
                self.records = list.agents;
                self.errors = list.errors;
                self.operation_error = None;
            }
            Err(error) => {
                self.records.clear();
                self.errors.clear();
                self.operation_error = Some(error.to_string());
            }
        }

        if self
            .selected_id
            .is_some_and(|id| !self.records.iter().any(|record| record.id() == id))
        {
            self.selected_id = None;
        }
        if self
            .pending_delete
            .as_ref()
            .is_some_and(|pending| !self.records.iter().any(|record| record.id() == pending.id))
        {
            self.pending_delete = None;
        }

        self.rows = self
            .records
            .iter()
            .map(|record| self.make_row_handles(record.id(), ctx))
            .collect();
        ctx.notify();
    }

    fn make_row_handles(&self, id: Uuid, ctx: &mut ViewContext<Self>) -> NamedAgentRowHandles {
        let select = ctx.add_typed_action_view(move |_| {
            ActionButton::new("Details", NakedTheme)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| ctx.dispatch_typed_action(LocalNamedAgentsAction::Select(id)))
        });
        let edit = ctx.add_typed_action_view(move |_| {
            ActionButton::new("Edit", SecondaryTheme)
                .with_icon(Icon::Pencil)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| ctx.dispatch_typed_action(LocalNamedAgentsAction::Edit(id)))
        });
        let run = ctx.add_typed_action_view(move |_| {
            ActionButton::new("Run", SecondaryTheme)
                .with_icon(Icon::Play)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| ctx.dispatch_typed_action(LocalNamedAgentsAction::Run(id)))
        });
        let delete = ctx.add_typed_action_view(move |_| {
            ActionButton::new("Delete", DangerNakedTheme)
                .with_icon(Icon::Trash)
                .with_size(ButtonSize::Small)
                .on_click(move |ctx| {
                    ctx.dispatch_typed_action(LocalNamedAgentsAction::AskDelete(id))
                })
        });
        NamedAgentRowHandles {
            select,
            edit,
            run,
            delete,
        }
    }

    fn create(&mut self, ctx: &mut ViewContext<Self>) {
        let name = format!("Local agent {}", self.records.len() + 1);
        let bundle = NamedAgentBundle {
            name,
            description: Some("Local named agent".to_owned()),
            base_prompt: Some(String::new()),
            model_id: "custom/local/code".to_owned(),
            profile_id: None,
            skills: Vec::new(),
            mcp_servers: None,
            harness: Harness::Oz,
            computer_use_enabled: None,
            secret_refs: None,
        };
        match Self::repository().create(bundle) {
            Ok(record) => {
                let path = record.path().to_path_buf();
                self.reload(ctx);
                self.selected_id = Some(record.id());
                // The local YAML editor is the create/edit form. It keeps the
                // complete document local while this summary stays redacted.
                ctx.open_file_path(&path);
            }
            Err(error) => self.set_error(error.to_string(), ctx),
        }
    }

    fn edit(&mut self, id: Uuid, ctx: &mut ViewContext<Self>) {
        match Self::repository().get(id) {
            Ok(record) => {
                self.operation_error = None;
                ctx.open_file_path(record.path());
            }
            Err(error) => self.set_error(error.to_string(), ctx),
        }
    }

    fn run(&mut self, id: Uuid, ctx: &mut ViewContext<Self>) {
        match Self::repository().get(id) {
            Ok(_) => {
                self.operation_error = None;
                // Keep execution on the local CLI path. The UUID is generated
                // by this repository and therefore safe to pass as one token.
                ctx.dispatch_typed_action(&WorkspaceAction::RunCommand(format!(
                    "warp agent run --agent {id}"
                )));
            }
            Err(error) => self.set_error(error.to_string(), ctx),
        }
    }

    fn ask_delete(&mut self, id: Uuid, ctx: &mut ViewContext<Self>) {
        let record = match Self::repository().get(id) {
            Ok(record) => record,
            Err(error) => {
                self.set_error(error.to_string(), ctx);
                return;
            }
        };
        let confirm = ctx.add_typed_action_view(|_| {
            ActionButton::new("Confirm delete", DangerNakedTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(LocalNamedAgentsAction::ConfirmDelete))
        });
        let cancel = ctx.add_typed_action_view(|_| {
            ActionButton::new("Cancel", SecondaryTheme)
                .with_size(ButtonSize::Small)
                .on_click(|ctx| ctx.dispatch_typed_action(LocalNamedAgentsAction::CancelDelete))
        });
        self.pending_delete = Some(PendingDelete {
            id,
            revision: record.revision().to_owned(),
            name: record.bundle().name.clone(),
            confirm,
            cancel,
        });
        self.operation_error = None;
        ctx.notify();
    }

    fn confirm_delete(&mut self, ctx: &mut ViewContext<Self>) {
        let Some(pending) = self.pending_delete.take() else {
            return;
        };
        let result = Self::repository().delete(&pending.id.to_string(), Some(&pending.revision));
        match result {
            Ok(()) => {
                if self.selected_id == Some(pending.id) {
                    self.selected_id = None;
                }
                self.operation_error = None;
                self.reload(ctx);
            }
            Err(error) => {
                self.operation_error = Some(error.to_string());
                self.pending_delete = Some(pending);
                ctx.notify();
            }
        }
    }

    fn set_error(&mut self, error: String, ctx: &mut ViewContext<Self>) {
        self.operation_error = Some(error);
        ctx.notify();
    }

    fn render_record_row(
        &self,
        index: usize,
        record: &NamedAgentRecord,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let selected = self.selected_id == Some(record.id());
        let row = &self.rows[index];
        let metadata = format!(
            "Model: {} • Harness: {}",
            record.bundle().model_id,
            harness_label(record.bundle().harness)
        );
        let mut body = Flex::column()
            .with_spacing(3.)
            .with_child(
                Text::new_inline(
                    record.bundle().name.clone(),
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_style(Properties::default().weight(Weight::Semibold))
                .with_color(theme.active_ui_text_color().into())
                .finish(),
            )
            .with_child(
                Text::new(
                    metadata,
                    appearance.ui_font_family(),
                    appearance.ui_font_size(),
                )
                .with_color(theme.nonactive_ui_text_color().into())
                .finish(),
            );
        let actions = Flex::row()
            .with_spacing(3.)
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(ChildView::new(&row.select).finish())
            .with_child(ChildView::new(&row.run).finish())
            .with_child(ChildView::new(&row.edit).finish())
            .with_child(ChildView::new(&row.delete).finish())
            .finish();
        body.add_child(actions);

        Container::new(body.finish())
            .with_background(if selected {
                theme.surface_3()
            } else {
                theme.surface_2()
            })
            .with_border(Border::all(1.).with_border_fill(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
            .with_uniform_padding(CARD_PADDING)
            .finish()
    }

    fn render_details(
        &self,
        appearance: &Appearance,
        selected: Option<&NamedAgentRecord>,
    ) -> Box<dyn Element> {
        let theme = appearance.theme();
        let mut details = Flex::column().with_spacing(5.).with_child(
            Text::new_inline(
                "Details",
                appearance.ui_font_family(),
                appearance.ui_font_size() + 1.,
            )
            .with_style(Properties::default().weight(Weight::Semibold))
            .with_color(theme.active_ui_text_color().into())
            .finish(),
        );
        if let Some(record) = selected {
            let bundle = record.bundle();
            details.add_child(detail_text(format!("Name: {}", bundle.name), appearance));
            details.add_child(detail_text(
                format!("Model: {}", bundle.model_id),
                appearance,
            ));
            details.add_child(detail_text(
                format!("Harness: {}", harness_label(bundle.harness)),
                appearance,
            ));
            if let Some(profile) = bundle.profile_id.as_deref() {
                details.add_child(detail_text(format!("Profile: {profile}"), appearance));
            }
            if !bundle.skills.is_empty() {
                details.add_child(detail_text(
                    format!("Skills: {}", bundle.skills.join(", ")),
                    appearance,
                ));
            }
            if let Some(mcp_servers) = bundle.mcp_servers.as_ref()
                && !mcp_servers.is_empty()
            {
                details.add_child(detail_text(
                    format!(
                        "MCP servers: {}",
                        mcp_servers.keys().cloned().collect::<Vec<_>>().join(", ")
                    ),
                    appearance,
                ));
            }
            if bundle.computer_use_enabled == Some(true) {
                details.add_child(detail_text("Computer use: enabled".to_owned(), appearance));
            }
            details.add_child(
                Text::new(
                    "Prompts and secret references are kept in the local YAML and are not shown here.",
                    appearance.ui_font_family(),
                    11.,
                )
                .with_color(theme.nonactive_ui_text_color().into())
                .soft_wrap(true)
                .finish(),
            );
        } else {
            details.add_child(detail_text(
                "Select a local named agent to inspect its safe summary.".to_owned(),
                appearance,
            ));
        }
        Container::new(details.finish())
            .with_background(theme.surface_2())
            .with_border(Border::all(1.).with_border_fill(theme.outline()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
            .with_uniform_padding(CARD_PADDING)
            .finish()
    }
}

fn detail_text(text: String, appearance: &Appearance) -> Box<dyn Element> {
    Text::new(text, appearance.ui_font_family(), appearance.ui_font_size())
        .with_color(appearance.theme().nonactive_ui_text_color().into())
        .soft_wrap(true)
        .finish()
}

fn harness_label(harness: Harness) -> &'static str {
    match harness {
        Harness::Oz => "oz",
        Harness::Claude => "claude",
        Harness::OpenCode => "opencode",
        Harness::Gemini => "gemini",
        Harness::Codex => "codex",
        Harness::Unknown => "unknown",
    }
}

impl Entity for LocalNamedAgentsView {
    type Event = ();
}

impl View for LocalNamedAgentsView {
    fn ui_name() -> &'static str {
        "LocalNamedAgentsView"
    }

    fn render(&self, app: &AppContext) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let mut rows = Flex::column().with_spacing(5.);
        for (index, record) in self.records.iter().enumerate() {
            rows.add_child(self.render_record_row(index, record, appearance));
        }
        if self.records.is_empty() {
            rows.add_child(detail_text(
                "No local named agents yet. Create one to start a reusable local bundle."
                    .to_owned(),
                appearance,
            ));
        }
        for error in &self.errors {
            rows.add_child(detail_text(
                format!(
                    "Invalid local agent file {}: {}",
                    error.safe_label(),
                    error.message
                ),
                appearance,
            ));
        }

        let selected = self
            .selected_id
            .and_then(|id| self.records.iter().find(|record| record.id() == id));
        let body = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Stretch)
            .with_spacing(8.)
            .with_child(Expanded::new(2., rows.finish()).finish())
            .with_child(Expanded::new(1., self.render_details(appearance, selected)).finish())
            .finish();

        let title = Text::new_inline(
            "Local named agents",
            appearance.ui_font_family(),
            appearance.ui_font_size() + 3.,
        )
        .with_style(Properties::default().weight(Weight::Semibold))
        .with_color(appearance.theme().active_ui_text_color().into())
        .finish();
        let subtitle = Text::new(
            "Reusable bundles stored on this device",
            appearance.ui_font_family(),
            11.,
        )
        .with_color(appearance.theme().nonactive_ui_text_color().into())
        .finish();
        let header = Flex::row()
            .with_cross_axis_alignment(CrossAxisAlignment::Center)
            .with_child(
                Flex::column()
                    .with_spacing(2.)
                    .with_child(title)
                    .with_child(subtitle)
                    .finish(),
            )
            .with_child(Expanded::new(1., Container::new(Flex::row().finish()).finish()).finish())
            .with_child(ChildView::new(&self.reload_button).finish())
            .with_child(ChildView::new(&self.create_button).finish())
            .finish();

        let mut content = Flex::column()
            .with_spacing(8.)
            .with_child(header)
            .with_child(body);
        if let Some(pending) = &self.pending_delete {
            content.add_child(
                Flex::row()
                    .with_spacing(6.)
                    .with_cross_axis_alignment(CrossAxisAlignment::Center)
                    .with_child(detail_text(
                        format!("Delete local agent '{}' ?", pending.name),
                        appearance,
                    ))
                    .with_child(ChildView::new(&pending.confirm).finish())
                    .with_child(ChildView::new(&pending.cancel).finish())
                    .finish(),
            );
        }
        if let Some(error) = &self.operation_error {
            content.add_child(detail_text(error.clone(), appearance));
        }

        Container::new(content.finish())
            .with_background(appearance.theme().surface_1())
            .with_border(Border::all(1.).with_border_fill(appearance.theme().surface_2()))
            .with_corner_radius(CornerRadius::with_all(Radius::Pixels(CARD_RADIUS)))
            .with_uniform_padding(CARD_PADDING)
            .finish()
    }
}

impl TypedActionView for LocalNamedAgentsView {
    type Action = LocalNamedAgentsAction;

    fn handle_action(&mut self, action: &Self::Action, ctx: &mut ViewContext<Self>) {
        match action {
            LocalNamedAgentsAction::Create => self.create(ctx),
            LocalNamedAgentsAction::Reload => self.reload(ctx),
            LocalNamedAgentsAction::Select(id) => {
                self.selected_id = Some(*id);
                ctx.notify();
            }
            LocalNamedAgentsAction::Edit(id) => self.edit(*id, ctx),
            LocalNamedAgentsAction::Run(id) => self.run(*id, ctx),
            LocalNamedAgentsAction::AskDelete(id) => self.ask_delete(*id, ctx),
            LocalNamedAgentsAction::ConfirmDelete => self.confirm_delete(ctx),
            LocalNamedAgentsAction::CancelDelete => {
                self.pending_delete = None;
                ctx.notify();
            }
        }
    }
}
