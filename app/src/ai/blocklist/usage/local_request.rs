use warpui::elements::{Empty, Text};
use warpui::{AppContext, Element, SingletonEntity};

use crate::ai::blocklist::AIBlock;
use crate::ai::blocklist::model::{AIBlockModel, AIBlockModelHelper};
use crate::appearance::Appearance;

fn request_label(model: &str, first_token_ms: Option<i64>, duration_ms: Option<i64>) -> String {
    let mut label = model
        .strip_prefix("custom/")
        .unwrap_or(model)
        .replacen('/', " / ", 1);
    if let Some(ms) = first_token_ms.filter(|ms| *ms >= 0) {
        label.push_str(&format!(" · First token {ms} ms"));
    }
    if let Some(ms) = duration_ms.filter(|ms| *ms >= 0) {
        label.push_str(&format!(" · Response {ms} ms"));
    }
    label
}

/// Request-scoped diagnostics from the local transcript, never hosted billing metadata.
pub(crate) fn render(
    model: &dyn AIBlockModel<View = AIBlock>,
    app: &AppContext,
) -> Box<dyn Element> {
    let exchange = model.conversation(app).and_then(|conversation| {
        model
            .exchange_id(app)
            .and_then(|id| conversation.exchange_with_id(id))
    });
    let Some(exchange) = exchange else {
        return Empty::new().finish();
    };
    let model_id = model
        .model_id(app)
        .unwrap_or_else(|| exchange.model_id.clone());
    let label = request_label(
        model_id.as_str(),
        exchange.time_to_first_token_ms,
        exchange
            .duration()
            .map(|duration| duration.num_milliseconds()),
    );
    let appearance = Appearance::as_ref(app);
    Text::new_inline(
        label,
        appearance.ui_font_family(),
        appearance.monospace_font_size(),
    )
    .with_color(
        appearance
            .theme()
            .sub_text_color(appearance.theme().background())
            .into(),
    )
    .with_selectable(false)
    .finish()
}

#[cfg(test)]
#[path = "local_request_tests.rs"]
mod tests;
