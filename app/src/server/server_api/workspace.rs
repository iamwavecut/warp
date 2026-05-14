use super::ServerApi;

#[cfg(test)]
use mockall::automock;

#[cfg_attr(test, automock)]
pub trait WorkspaceClient: 'static + Send + Sync {}

impl WorkspaceClient for ServerApi {}
