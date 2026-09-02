use std::sync::Arc;

use chrono::{Duration, Local};
use futures_lite::future::block_on;

use super::*;

fn snapshot(entries: Vec<HistoryEntry>, query_text: &str) -> HistorySnapshot {
    HistorySnapshot {
        commands: entries.into_iter().map(Arc::new).collect(),
        query_text: query_text.to_owned(),
        current_session_id: SessionId::from(0),
    }
}

#[test]
fn history_search_ands_whitespace_separated_terms() {
    let results = block_on(fuzzy_match_history(snapshot(
        vec![
            HistoryEntry::command_only("cd ~/projects/history_orm".to_owned()),
            HistoryEntry::command_only("cd ~/projects/other".to_owned()),
        ],
        "cd hi orm",
    )))
    .expect("history matching should succeed");

    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].accessibility_label(),
        "History item: cd ~/projects/history_orm"
    );
}

#[test]
fn history_search_uses_recency_to_break_equal_quality_ties() {
    let now = Local::now();
    let mut old = HistoryEntry::command_only("make test".to_owned());
    old.start_ts = Some(now - Duration::days(30));
    let mut fresh = HistoryEntry::command_only("make test".to_owned());
    fresh.start_ts = Some(now);

    let results = block_on(fuzzy_match_history(snapshot(vec![old, fresh], "make test")))
        .expect("history matching should succeed");

    assert_eq!(results.len(), 2);
    assert!(results[1].score() > results[0].score());
}
