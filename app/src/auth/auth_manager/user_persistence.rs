use warpui::AppContext;
use warpui_extras::secure_storage;

pub struct PersistedUser;

#[derive(Debug, thiserror::Error)]
pub enum UserPersistenceError {
    /// The persisted user was not successfully removed from secure storage.
    #[error("secure storage error")]
    SecureStorageError(#[from] secure_storage::Error),
}

impl PersistedUser {
    pub fn remove_from_secure_storage(ctx: &AppContext) -> Result<(), UserPersistenceError> {
        let _ = ctx;
        Ok(())
    }
}
