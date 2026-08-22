use gitquarry_i18n::Locale;
use gitquarry_i18n::shared::auth::*;

#[test]
fn user_not_found_differs_by_locale_and_carries_the_username() {
    let es = user_not_found(Locale::Es, "alice");
    let en = user_not_found(Locale::En, "alice");
    assert_ne!(es, en);
    assert_eq!(es, "usuario 'alice' no existe");
    assert_eq!(en, "user 'alice' doesn't exist");
}

#[test]
fn user_deleted_differs_by_locale() {
    assert_eq!(user_deleted(Locale::Es), "(usuario eliminado)");
    assert_ne!(user_deleted(Locale::Es), user_deleted(Locale::En));
}

#[test]
fn key_already_associated_differs_by_locale() {
    assert_eq!(
        key_already_associated(Locale::Es),
        "esa clave ya esta asociada a otro usuario"
    );
    assert_ne!(key_already_associated(Locale::Es), key_already_associated(Locale::En));
}

#[test]
fn invalid_permission_carries_the_bad_value() {
    assert_eq!(
        invalid_permission(Locale::Es, "delete"),
        "permiso invalido 'delete' (usar guest, reporter, developer, maintainer u owner)"
    );
    assert_ne!(
        invalid_permission(Locale::Es, "delete"),
        invalid_permission(Locale::En, "delete")
    );
}

#[test]
fn incorrect_code_differs_by_locale() {
    assert_eq!(incorrect_code(Locale::Es), "codigo incorrecto");
    assert_ne!(incorrect_code(Locale::Es), incorrect_code(Locale::En));
}

#[test]
fn totp_already_enabled_differs_by_locale() {
    assert!(totp_already_enabled(Locale::Es).contains("2FA"));
    assert_ne!(totp_already_enabled(Locale::Es), totp_already_enabled(Locale::En));
}

#[test]
fn no_pending_totp_enrollment_differs_by_locale() {
    assert_eq!(
        no_pending_totp_enrollment(Locale::Es),
        "no hay una inscripcion 2FA pendiente"
    );
}

#[test]
fn account_no_longer_exists_differs_by_locale() {
    assert_eq!(account_no_longer_exists(Locale::Es), "tu usuario ya no existe");
    assert_ne!(
        account_no_longer_exists(Locale::Es),
        account_no_longer_exists(Locale::En)
    );
}

#[test]
fn name_already_taken_carries_the_name() {
    assert_eq!(
        name_already_taken(Locale::Es, "acme"),
        "'acme' ya esta en uso (usuario o grupo)"
    );
}

#[test]
fn wrong_username_or_password_differs_by_locale() {
    assert_eq!(wrong_username_or_password(Locale::Es), "usuario o password incorrectos");
    assert_ne!(
        wrong_username_or_password(Locale::Es),
        wrong_username_or_password(Locale::En)
    );
}

#[test]
fn too_many_failed_attempts_differs_by_locale() {
    assert!(too_many_failed_attempts(Locale::Es).contains("intentos"));
    assert_ne!(
        too_many_failed_attempts(Locale::Es),
        too_many_failed_attempts(Locale::En)
    );
}

#[test]
fn team_not_found_carries_the_team_name() {
    assert_eq!(team_not_found(Locale::Es, "frontend"), "equipo 'frontend' no existe");
}

#[test]
fn invalid_entry_carries_the_entry() {
    assert!(invalid_entry(Locale::Es, "bad").contains("bad"));
}

#[test]
fn team_already_exists_carries_the_team_name() {
    assert_eq!(
        team_already_exists(Locale::Es, "frontend"),
        "ya existe un equipo 'frontend' en este grupo"
    );
}

#[test]
fn only_admin_creates_groups_differs_by_locale() {
    assert_ne!(
        only_admin_creates_groups(Locale::Es),
        only_admin_creates_groups(Locale::En)
    );
}

#[test]
fn only_admin_deletes_groups_differs_by_locale() {
    assert_ne!(
        only_admin_deletes_groups(Locale::Es),
        only_admin_deletes_groups(Locale::En)
    );
}

#[test]
fn you_dont_manage_group_carries_the_group_name() {
    assert_eq!(
        you_dont_manage_group(Locale::Es, "acme"),
        "no administras el grupo 'acme'"
    );
}

#[test]
fn group_not_found_carries_the_group_name() {
    assert_eq!(group_not_found(Locale::Es, "acme"), "grupo 'acme' no existe");
}

#[test]
fn favorites_not_public_carries_the_username() {
    assert!(favorites_not_public(Locale::Es, "alice").contains("alice"));
}

#[test]
fn wrong_password_or_code_differs_by_locale() {
    assert_ne!(wrong_password_or_code(Locale::Es), wrong_password_or_code(Locale::En));
}

#[test]
fn account_is_not_admin_differs_by_locale() {
    assert_ne!(account_is_not_admin(Locale::Es), account_is_not_admin(Locale::En));
}

#[test]
fn missing_repo_differs_by_locale() {
    assert_ne!(missing_repo(Locale::Es), missing_repo(Locale::En));
}

#[test]
fn invalid_size_carries_the_value() {
    assert!(invalid_size(Locale::Es, "xl").contains("xl"));
}

#[test]
fn invalid_theme_carries_the_value() {
    assert!(invalid_theme(Locale::Es, "blue").contains("blue"));
}

#[test]
fn building_set_cookie_differs_by_locale() {
    assert_ne!(building_set_cookie(Locale::Es), building_set_cookie(Locale::En));
}

#[test]
fn no_totp_login_in_progress_differs_by_locale() {
    assert_ne!(
        no_totp_login_in_progress(Locale::Es),
        no_totp_login_in_progress(Locale::En)
    );
}

#[test]
fn invalid_totp_cookie_differs_by_locale() {
    assert_ne!(invalid_totp_cookie(Locale::Es), invalid_totp_cookie(Locale::En));
}

#[test]
fn corrupt_totp_cookie_differs_by_locale() {
    assert_ne!(corrupt_totp_cookie(Locale::Es), corrupt_totp_cookie(Locale::En));
}
