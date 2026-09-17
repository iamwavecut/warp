use super::*;

#[test]
fn local_conversation_summary_keeps_models_and_only_known_response_times() {
    let summary = Summary::from_requests([
        ("custom/home/model/a", Some(120)),
        ("custom/home/model/a", None),
        ("custom/other/b", Some(-5)),
        ("custom/other/b", Some(0)),
    ]);
    assert_eq!(summary.exchanges, 4);
    assert_eq!(summary.models.len(), 2);
    assert_eq!(summary.models["custom/home/model/a"], 2);
    assert_eq!(summary.models["custom/other/b"], 2);
    assert_eq!(summary.measured_responses, 2);
    assert_eq!(summary.response_ms, 120);
}

#[test]
fn local_conversation_summary_does_not_invent_missing_measurements() {
    let summary = Summary::from_requests([("custom/home/model", None)]);
    assert_eq!(summary.measured_responses, 0);
    let empty = Summary::from_requests([]);
    assert_eq!(empty.exchanges, 0);
    assert!(empty.models.is_empty());
}
