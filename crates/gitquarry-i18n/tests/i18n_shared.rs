use gitquarry_i18n::Locale;
use gitquarry_i18n::shared::*;

#[test]
fn cas_conflict_differs_by_locale() {
    let es = cas_conflict(Locale::Es);
    let en = cas_conflict(Locale::En);
    assert_ne!(es, en);
    assert_eq!(
        es,
        "conflicto guardando (otra edicion concurrente) — recarga y reintenta."
    );
    assert_eq!(
        en,
        "conflict saving (another edit happened concurrently) — reload and retry."
    );
}

#[test]
fn unknown_action_differs_by_locale_and_carries_the_action() {
    let es = unknown_action(Locale::Es, "delete_everything");
    let en = unknown_action(Locale::En, "delete_everything");
    assert_ne!(es, en);
    assert_eq!(es, "accion desconocida: delete_everything");
    assert_eq!(en, "unknown action: delete_everything");
}

#[test]
fn invalid_csrf_differs_by_locale() {
    assert_ne!(invalid_csrf(Locale::Es), invalid_csrf(Locale::En));
}

#[test]
fn only_your_own_account_differs_by_locale() {
    assert_ne!(only_your_own_account(Locale::Es), only_your_own_account(Locale::En));
}
