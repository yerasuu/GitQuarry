use gitquarry_i18n::Locale;
use gitquarry_i18n::shared::settings::*;

#[test]
fn oidc_fields_go_together_differs_by_locale() {
    assert_ne!(oidc_fields_go_together(Locale::Es), oidc_fields_go_together(Locale::En));
}
