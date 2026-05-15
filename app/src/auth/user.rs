use serde::{Deserialize, Serialize};

use super::UserUid;

#[cfg(any(test, feature = "integration_tests"))]
pub use warp_server_client::auth::{TEST_USER_EMAIL, TEST_USER_UID};

/// Type of principal making the authenticated request.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PrincipalType {
    #[default]
    User,
    ServiceAccount,
}

impl From<warp_graphql::queries::get_user::PrincipalType> for PrincipalType {
    fn from(value: warp_graphql::queries::get_user::PrincipalType) -> Self {
        use warp_graphql::queries::get_user::PrincipalType as GqlPrincipalType;
        match value {
            GqlPrincipalType::User => PrincipalType::User,
            GqlPrincipalType::ServiceAccount => PrincipalType::ServiceAccount,
        }
    }
}

/// The in-memory representation of a logged-in User.
/// This does not include authentication credentials, which are stored separately
/// in the `Credentials` enum.
#[derive(Debug, Clone)]
pub struct User {
    /// The local user identifier.
    pub local_id: UserUid,
    /// Metadata about the user.
    pub metadata: UserMetadata,
    /// Whether or not the user is onboarded.
    pub is_onboarded: bool,
    /// Whether or not this user is on what we consider a "work" domain, meaning the domain isn't
    /// from a general email provider (e.g. gmail.com, hotmail.com, proton.me, etc.).
    /// Local-only builds default this to `false`.
    pub is_on_work_domain: bool,
    /// Type of principal (user or service account). Fetched fresh from the server
    /// on each login/refresh.
    pub principal_type: PrincipalType,
    /// Skill specs that should be available to this principal in every agent run.
    pub global_skills: Vec<String>,
}

/// This struct holds extra information about the local user profile.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UserMetadata {
    /// The user's email. Empty for the default local profile.
    pub email: String,
    /// The user's display name. We should prefer showing this over their email, if available.
    pub display_name: Option<String>,
    /// A URL for their profile picture.
    pub photo_url: Option<String>,
}

impl User {
    pub fn local() -> Self {
        Self {
            local_id: UserUid::new("local_user"),
            metadata: UserMetadata {
                email: String::new(),
                display_name: Some("Local User".to_string()),
                photo_url: None,
            },
            is_onboarded: true,
            is_on_work_domain: false,
            principal_type: PrincipalType::User,
            global_skills: Vec::new(),
        }
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub fn test() -> Self {
        Self {
            local_id: UserUid::new(TEST_USER_UID),
            metadata: UserMetadata {
                email: TEST_USER_EMAIL.to_string(),
                display_name: None,
                photo_url: None,
            },
            is_onboarded: true,
            is_on_work_domain: false,
            principal_type: PrincipalType::User,
            global_skills: Vec::new(),
        }
    }
}

#[cfg(test)]
#[path = "user_test.rs"]
mod tests;
