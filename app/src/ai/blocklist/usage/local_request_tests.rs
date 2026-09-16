use super::request_label;

#[test]
fn local_request_details_show_model_and_observed_timings_without_prices() {
    assert_eq!(
        request_label("custom/local/qwen/coder", Some(125), Some(2400)),
        "local / qwen/coder · First token 125 ms · Response 2400 ms"
    );
}

#[test]
fn local_request_details_do_not_invent_missing_or_negative_timings() {
    assert_eq!(
        request_label("custom/local/model", None, None),
        "local / model"
    );
    assert_eq!(
        request_label("custom/local/model", Some(-1), Some(-1)),
        "local / model"
    );
    assert_eq!(
        request_label("custom/local/model", Some(0), Some(0)),
        "local / model · First token 0 ms · Response 0 ms"
    );
}
