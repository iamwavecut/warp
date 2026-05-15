use super::*;

#[test]
fn test_user_profile_metadata() {
    let user = User {
        is_onboarded: true,
        local_id: UserUid::new("test_local_id"),
        metadata: UserMetadata {
            email: "test_user@example.com".to_string(),
            display_name: Some("Test User".to_string()),
            photo_url: Some("https://photourl.example.com/1234".to_string()),
        },
        is_on_work_domain: false,
        principal_type: PrincipalType::User,
        global_skills: Vec::new(),
    };
    assert_eq!(user.metadata.display_name.as_deref(), Some("Test User"));
    assert_eq!(user.metadata.email, "test_user@example.com");
    assert_eq!(
        user.metadata.photo_url.as_deref(),
        Some("https://photourl.example.com/1234")
    );
}

#[test]
fn test_user_global_skills_defaults_to_empty() {
    assert_eq!(User::test().global_skills, Vec::<String>::new());
}
