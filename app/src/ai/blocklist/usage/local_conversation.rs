use std::collections::BTreeMap;

use warpui::elements::{Border, ConstrainedBox, Container, Flex, ParentElement, Text};
use warpui::{AppContext, Element, SingletonEntity};

use crate::ai::agent::conversation::AIConversation;
use crate::appearance::Appearance;

#[derive(Default)]
struct Summary {
    exchanges: usize,
    models: BTreeMap<String, usize>,
    measured_responses: usize,
    response_ms: i64,
}

impl Summary {
    fn from_requests<'a>(requests: impl IntoIterator<Item = (&'a str, Option<i64>)>) -> Self {
        let mut summary = Self::default();
        for (model, duration) in requests {
            summary.exchanges += 1;
            *summary.models.entry(model.to_owned()).or_default() += 1;
            if let Some(ms) = duration.filter(|ms| *ms >= 0) {
                summary.measured_responses += 1;
                summary.response_ms = summary.response_ms.saturating_add(ms);
            }
        }
        summary
    }
}

/// Derived from the active local transcript; no hosted usage or billing metadata.
pub(crate) fn render(conversation: &AIConversation, app: &AppContext) -> Box<dyn Element> {
    let summary = Summary::from_requests(conversation.root_task_exchanges().map(|exchange| {
        (
            exchange.model_id.as_str(),
            exchange.duration().map(|d| d.num_milliseconds()),
        )
    }));
    let appearance = Appearance::as_ref(app);
    let theme = appearance.theme();
    let mut rows = vec![
        "Conversation (local)".to_string(),
        format!("Exchanges: {}", summary.exchanges),
    ];
    for (model, count) in &summary.models {
        let label = model
            .strip_prefix("custom/")
            .and_then(|id| id.split_once('/'))
            .map(|(provider, model)| format!("{provider} / {model}"))
            .unwrap_or_else(|| model.clone());
        rows.push(format!("{label}: {count}"));
    }
    if summary.measured_responses > 0 {
        rows.push(format!(
            "Measured response time: {} ms ({} exchanges)",
            summary.response_ms, summary.measured_responses
        ));
    }
    rows.push("Token breakdown: unavailable".to_string());
    ConstrainedBox::new(
        Container::new(
            Flex::column()
                .with_spacing(6.)
                .with_children(
                    rows.into_iter()
                        .map(|row| {
                            Text::new(row, appearance.ui_font_family(), appearance.ui_font_size())
                                .with_color(theme.main_text_color(theme.surface_2()).into())
                                .finish()
                        })
                        .collect::<Vec<_>>(),
                )
                .finish(),
        )
        .with_uniform_padding(12.)
        .with_background(theme.surface_2())
        .with_border(Border::all(1.).with_border_fill(theme.outline()))
        .finish(),
    )
    .with_width(386.)
    .finish()
}

#[cfg(test)]
#[path = "local_conversation_tests.rs"]
mod tests;
