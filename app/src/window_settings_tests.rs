use super::*;
use warpui::App;

#[test]
fn backdrop_migrates_legacy_acrylic_locally_and_preserves_explicit_choices() {
    App::test((), |mut app| async move {
        app.update(|ctx| {
            crate::settings::init_and_register_user_preferences(ctx);
            ctx.add_singleton_model(|_| settings::SettingsManager::default());
            WindowSettings::register(ctx);
        });
        WindowSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .legacy_override_blur_texture
                .set_value(true, ctx)
                .unwrap();
        });
        app.update(migrate_legacy_background_backdrop);
        app.read(|ctx| {
            assert_eq!(
                *WindowSettings::as_ref(ctx).background_backdrop,
                WindowBackdrop::Acrylic
            )
        });
        for backdrop in WindowBackdrop::ALL {
            WindowSettings::handle(&app).update(&mut app, |settings, ctx| {
                settings
                    .background_backdrop
                    .set_value(backdrop, ctx)
                    .unwrap();
            });
            app.update(migrate_legacy_background_backdrop);
            app.read(|ctx| assert_eq!(*WindowSettings::as_ref(ctx).background_backdrop, backdrop));
        }
    });
}

#[test]
fn backdrop_defaults_to_no_material_and_round_trips_each_choice() {
    assert_eq!(WindowBackdrop::default(), WindowBackdrop::None);
    for backdrop in WindowBackdrop::ALL {
        let encoded = serde_json::to_string(&backdrop).unwrap();
        assert_eq!(
            serde_json::from_str::<WindowBackdrop>(&encoded).unwrap(),
            backdrop
        );
    }
}
