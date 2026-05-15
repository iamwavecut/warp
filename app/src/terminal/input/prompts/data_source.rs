use fuzzy_match::FuzzyMatchResult;
use ordered_float::OrderedFloat;
use warp_core::ui::icons::Icon;
use warpui::elements::{ConstrainedBox, Container, Highlight, Text};
use warpui::fonts::{Properties, Weight};
use warpui::text_layout::ClipConfig;
use warpui::{AppContext, Element, Entity, ModelContext, SingletonEntity as _};

use crate::appearance::Appearance;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::DataSourceRunErrorWrapper;
use crate::search::result_renderer::ItemHighlightState;
use crate::search::FuzzyMatchWorkflowResult;
use crate::search::{SearchItem, SyncDataSource};
use crate::terminal::input::inline_menu::styles as inline_styles;
use crate::terminal::input::inline_menu::{
    default_navigation_message_items, InlineMenuAction, InlineMenuMessageArgs, InlineMenuType,
};
use crate::terminal::input::message_bar::Message;
use crate::user_config::WarpConfig;
use crate::workflows::workflow::Workflow;

#[derive(Clone, Debug)]
pub struct AcceptPrompt {
    pub workflow: Workflow,
}

impl InlineMenuAction for AcceptPrompt {
    const MENU_TYPE: InlineMenuType = InlineMenuType::PromptsMenu;

    fn produce_inline_menu_message<T>(args: InlineMenuMessageArgs<'_, Self, T>) -> Option<Message> {
        Some(Message::new(default_navigation_message_items(&args)))
    }
}

pub struct PromptsMenuDataSource;

impl PromptsMenuDataSource {
    pub fn new(_ctx: &mut ModelContext<Self>) -> Self {
        Self
    }
}

impl SyncDataSource for PromptsMenuDataSource {
    type Action = AcceptPrompt;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        let query_text = query.text.trim();
        let local_workflows = WarpConfig::as_ref(app).local_user_workflows();

        if query_text.is_empty() {
            return Ok(local_workflows
                .iter()
                .filter(|workflow| !workflow.is_command_workflow())
                .map(|workflow| QueryResult::from(PromptSearchItem::from_workflow(workflow)))
                .collect());
        }

        // For single-character queries, use prefix matching on the name instead of fuzzy
        // search to avoid missing valid results while still filtering the list.
        if query_text.chars().count() == 1 {
            let query_char = query_text.chars().next().unwrap();

            return Ok(local_workflows
                .iter()
                .filter(|workflow| {
                    !workflow.is_command_workflow()
                        && workflow.name_starts_with_char_ignore_case(query_char)
                })
                .map(|workflow| QueryResult::from(PromptSearchItem::from_workflow(workflow)))
                .collect());
        }

        Ok(local_workflows
            .iter()
            .filter(|workflow| !workflow.is_command_workflow())
            .filter_map(|workflow| {
                let match_result = FuzzyMatchWorkflowResult::try_match(query_text, workflow, "")?;
                let score = match_result.score();
                if score <= OrderedFloat(25.0) {
                    return None;
                }
                Some(QueryResult::from(
                    PromptSearchItem::from_workflow(workflow)
                        .with_name_match_result(match_result.name_match_result)
                        .with_score(score),
                ))
            })
            .collect())
    }
}

impl Entity for PromptsMenuDataSource {
    type Event = ();
}

#[derive(Clone)]
struct PromptSearchItem {
    workflow: Workflow,
    name: String,
    name_match_result: Option<FuzzyMatchResult>,
    score: OrderedFloat<f64>,
}

impl PromptSearchItem {
    fn from_workflow(workflow: &Workflow) -> Self {
        Self {
            workflow: workflow.clone(),
            name: workflow.name().to_owned(),
            name_match_result: None,
            score: OrderedFloat(f64::MIN),
        }
    }

    fn with_name_match_result(mut self, result: Option<FuzzyMatchResult>) -> Self {
        self.name_match_result = result;
        self
    }

    fn with_score(mut self, score: OrderedFloat<f64>) -> Self {
        self.score = score;
        self
    }
}

impl SearchItem for PromptSearchItem {
    type Action = AcceptPrompt;

    fn render_icon(
        &self,
        _highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Box<dyn Element> {
        let icon_color = inline_styles::icon_color(appearance);
        let icon_size = inline_styles::font_size(appearance);

        let icon = Icon::Prompt.to_warpui_icon(icon_color).finish();

        Container::new(
            ConstrainedBox::new(icon)
                .with_width(icon_size)
                .with_height(icon_size)
                .finish(),
        )
        .with_margin_right(inline_styles::ICON_MARGIN)
        .finish()
    }

    fn render_item(
        &self,
        _highlight_state: ItemHighlightState,
        app: &AppContext,
    ) -> Box<dyn Element> {
        let appearance = Appearance::as_ref(app);
        let theme = appearance.theme();

        let font_size = inline_styles::font_size(appearance);
        let background_color = inline_styles::menu_background_color(app);
        let primary_text_color = inline_styles::primary_text_color(theme, background_color.into());

        let mut name_text =
            Text::new_inline(self.name.clone(), appearance.ui_font_family(), font_size)
                .with_color(primary_text_color.into())
                .with_clip(ClipConfig::ellipsis());

        if let Some(name_match) = &self.name_match_result {
            if !name_match.matched_indices.is_empty() {
                name_text = name_text.with_single_highlight(
                    Highlight::new().with_properties(Properties::default().weight(Weight::Bold)),
                    name_match.matched_indices.clone(),
                );
            }
        }

        name_text.finish()
    }

    fn item_background(
        &self,
        highlight_state: ItemHighlightState,
        appearance: &Appearance,
    ) -> Option<warp_core::ui::theme::Fill> {
        inline_styles::item_background(highlight_state, appearance)
    }

    fn score(&self) -> OrderedFloat<f64> {
        self.score
    }

    fn accept_result(&self) -> Self::Action {
        AcceptPrompt {
            workflow: self.workflow.clone(),
        }
    }

    fn execute_result(&self) -> Self::Action {
        self.accept_result()
    }

    fn accessibility_label(&self) -> String {
        format!("Prompt: {}", self.name)
    }
}
