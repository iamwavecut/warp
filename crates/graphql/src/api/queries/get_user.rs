use crate::{object_permissions::OwnerType, request_context::RequestContext, schema};

/*
query GetUser($requestContext: RequestContext!) {
  user(requestContext: $requestContext) {
    ... on UserOutput {
      apiKeyOwnerType
      principalType
      user {
        globalSkills
        isOnWorkDomain
        isOnboarded
        profile {
          displayName
          email
          needsSsoLink
          photoUrl
          uid
        }
        llms {
          agentMode {
            defaultId
            choices {
              id
              displayName
              baseModelName
              reasoningLevel
              description
              disableReason
              visionSupported
              onboardingInfo {
                title
                description
              }
            }
          }
          planning {
            defaultId
            choices {
              id
              displayName
              baseModelName
              reasoningLevel
              description
              disableReason
              visionSupported
              onboardingInfo {
                title
                description
              }
            }
          }
          coding {
            defaultId
            choices {
              id
              displayName
              baseModelName
              reasoningLevel
              description
              disableReason
              visionSupported
              onboardingInfo {
                title
                description
              }
            }
          }
          cliAgent {
            defaultId
            choices {
              id
              displayName
              baseModelName
              reasoningLevel
              description
              disableReason
              visionSupported
              onboardingInfo {
                title
                description
              }
            }
          }
        }
      }
    }
  }
}
*/

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "RootQuery", variables = "GetUserVariables")]
pub struct GetUser {
    #[arguments(requestContext: $request_context)]
    pub user: UserResult,
}
crate::client::define_operation! {
    get_user(GetUserVariables) -> GetUser;
}

#[derive(cynic::QueryVariables, Debug)]
pub struct GetUserVariables {
    pub request_context: RequestContext,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct UserOutput {
    pub api_key_owner_type: Option<OwnerType>,
    pub principal_type: Option<PrincipalType>,
    pub user: User,
}

#[derive(cynic::Enum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrincipalType {
    User,
    ServiceAccount,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct User {
    pub global_skills: Vec<String>,
    pub is_onboarded: bool,
    pub is_on_work_domain: bool,
    pub profile: FirebaseProfile,
}

#[derive(cynic::QueryFragment, Debug)]
pub struct FirebaseProfile {
    pub display_name: Option<String>,
    pub email: Option<String>,
    pub needs_sso_link: bool,
    pub photo_url: Option<String>,
    pub uid: String,
}

#[derive(cynic::InlineFragments, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum UserResult {
    UserOutput(UserOutput),
    #[cynic(fallback)]
    Unknown,
}
