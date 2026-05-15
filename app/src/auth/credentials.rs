//! Representation of local user credentials.
//!
//! Local-first sessions use [`Credentials::Local`] and do not exchange tokens with hosted services.

/// Represents the different legacy ways a user could authenticate with Warp.
#[derive(Clone, Debug)]
pub enum Credentials {
    /// Authentication derived from an ambient browser session cookie.
    SessionCookie,
    /// Test credentials used in unit tests and integration tests.
    #[cfg(any(test, feature = "integration_tests"))]
    Test,
    /// Local credentials for builds where no server auth is needed.
    Local,
}

impl Credentials {
    /// Returns the short-lived token to use in HTTP requests to the server.
    pub fn bearer_token(&self) -> AuthToken {
        AuthToken::NoAuth
    }
}

/// Represents different types of authentication tokens.
#[derive(Debug, Clone)]
pub enum AuthToken {
    /// No authentication token available (e.g. session cookie auth or test credentials).
    NoAuth,
}

impl AuthToken {
    /// Returns the token string to use in an Authorization header, or `None` if auth is not
    /// header-based (e.g. session cookie) or there is no auth.
    pub fn as_bearer_token(&self) -> Option<&str> {
        match self {
            AuthToken::NoAuth => None,
        }
    }

    /// Returns the bearer token as an owned string, or `None` if auth is not header-based.
    pub fn bearer_token(&self) -> Option<String> {
        match self {
            AuthToken::NoAuth => None,
        }
    }
}
