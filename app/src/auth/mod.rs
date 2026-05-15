pub mod auth_manager;
pub mod auth_state;
pub mod credentials;
pub mod user;
pub mod user_uid;

pub use auth_manager::AuthManager;
pub use auth_state::AuthStateProvider;
pub use user_uid::UserUid;
use warpui::AppContext;

pub fn init(app: &mut AppContext) {
    let _ = app;
}
