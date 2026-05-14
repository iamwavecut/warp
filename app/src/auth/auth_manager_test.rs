use super::AuthManager;
use crate::auth::{credentials::Credentials, AuthStateProvider};
use crate::ServerApiProvider;
use warpui::{App, SingletonEntity};

fn initialize_app(app: &mut App) {
    app.add_singleton_model(|_ctx| ServerApiProvider::new_for_test());
    app.add_singleton_model(|_| AuthStateProvider::new_for_test());
    app.add_singleton_model(AuthManager::new_for_test);
}

// This verifies that local credentials do not write hosted auth state to secure storage.
// No secure storage singleton is registered in this test app: if `write_to_secure_storage` were
// ever called, it would panic trying to look up the unregistered singleton.

#[test]
fn test_persist_skips_for_local_credentials() {
    App::test((), |mut app| async move {
        initialize_app(&mut app);

        app.update(|ctx| {
            AuthStateProvider::as_ref(ctx)
                .get()
                .set_credentials(Some(Credentials::Local));
        });

        AuthManager::handle(&app).update(&mut app, |auth_manager, ctx| {
            auth_manager.persist(ctx);
        });
    });
}
