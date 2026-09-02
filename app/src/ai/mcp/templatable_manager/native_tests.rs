use std::collections::HashMap;

use super::{
    challenge_parameter_names, has_caller_supplied_credential, is_oauth_challenge,
    should_report_rejected_credentials,
};

fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect()
}

#[test]
fn oauth_challenge_requires_resource_metadata_parameter() {
    assert!(is_oauth_challenge(
        r#"Bearer resource_metadata="https://srv.example/.well-known/oauth-protected-resource""#
    ));
    assert!(is_oauth_challenge(
        r#"bearer RESOURCE_METADATA = "https://srv.example""#
    ));
    assert!(!is_oauth_challenge(
        r#"Bearer error="invalid_token", error_description="expired""#
    ));
}

#[test]
fn oauth_challenge_ignores_resource_metadata_inside_quoted_values() {
    assert!(!is_oauth_challenge(
        r#"Bearer error_description="no resource_metadata was supplied""#
    ));
    assert!(is_oauth_challenge(
        r#"Bearer error_description="mentions resource_metadata", resource_metadata="https://srv.example""#
    ));
}

#[test]
fn challenge_parameter_names_skip_quoted_values() {
    assert_eq!(
        challenge_parameter_names(r#"Bearer realm="srv", error="invalid_token""#),
        vec!["realm", "error"]
    );
    assert_eq!(
        challenge_parameter_names(r#"Bearer realm="unterminated"#),
        vec!["realm"]
    );
}

#[test]
fn bare_401_is_credential_rejection_only_when_a_credential_was_sent() {
    let configured = headers(&[("Authorization", "Bearer rejected-token")]);
    assert!(has_caller_supplied_credential(&configured));
    assert!(should_report_rejected_credentials(&configured, &[]));

    let oauth = vec![
        r#"Bearer resource_metadata="https://srv.example/.well-known/oauth-protected-resource""#
            .to_string(),
    ];
    assert!(!should_report_rejected_credentials(&configured, &oauth));
    assert!(!should_report_rejected_credentials(&headers(&[]), &[]));
    assert!(!has_caller_supplied_credential(&headers(&[(
        "Authorization",
        "   "
    )])));
}
