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
fn user_not_found_differs_by_locale_and_carries_the_username() {
    let es = user_not_found(Locale::Es, "alice");
    let en = user_not_found(Locale::En, "alice");
    assert_ne!(es, en);
    assert_eq!(es, "usuario 'alice' no existe");
    assert_eq!(en, "user 'alice' doesn't exist");
}

#[test]
fn repo_not_found_differs_by_locale_and_carries_the_repo_name() {
    let es = repo_not_found(Locale::Es, "acme/proj");
    let en = repo_not_found(Locale::En, "acme/proj");
    assert_ne!(es, en);
    assert_eq!(es, "repo 'acme/proj' no existe");
    assert_eq!(en, "repo 'acme/proj' doesn't exist");
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
fn user_deleted_differs_by_locale() {
    assert_eq!(user_deleted(Locale::Es), "(usuario eliminado)");
    assert_ne!(user_deleted(Locale::Es), user_deleted(Locale::En));
}

#[test]
fn repo_deleted_differs_by_locale() {
    assert_eq!(repo_deleted(Locale::Es), "(repo eliminado)");
    assert_ne!(repo_deleted(Locale::Es), repo_deleted(Locale::En));
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
fn repo_already_exists_at_carries_the_name() {
    assert_eq!(
        repo_already_exists_at(Locale::Es, "acme/proj"),
        "ya existe un repo en 'acme/proj'"
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
fn destination_is_same_repo_differs_by_locale() {
    assert_eq!(destination_is_same_repo(Locale::Es), "el destino es el mismo repo");
    assert_ne!(
        destination_is_same_repo(Locale::Es),
        destination_is_same_repo(Locale::En)
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
fn copy_seems_to_have_failed_differs_by_locale() {
    assert_eq!(
        copy_seems_to_have_failed(Locale::Es),
        "la copia parece haber fallado, reintenta"
    );
    assert_ne!(
        copy_seems_to_have_failed(Locale::Es),
        copy_seems_to_have_failed(Locale::En)
    );
}

#[test]
fn team_already_exists_carries_the_team_name() {
    assert_eq!(
        team_already_exists(Locale::Es, "frontend"),
        "ya existe un equipo 'frontend' en este grupo"
    );
}

#[test]
fn protected_tag_cannot_be_deleted_carries_the_tag_name() {
    assert_eq!(
        protected_tag_cannot_be_deleted(Locale::Es, "v1.0"),
        "'v1.0' es una etiqueta protegida y no puede eliminarse"
    );
}

#[test]
fn snapshot_labels_differ_by_locale() {
    assert_eq!(snapshot_tar_gz(Locale::En), "snapshot .tar.gz");
    assert_eq!(snapshot_zip(Locale::En), "snapshot .zip");
    assert_ne!(snapshot_tar_gz(Locale::Es), snapshot_tar_gz(Locale::En));
}

#[test]
fn tag_already_exists_carries_the_tag_name() {
    assert_eq!(
        tag_already_exists(Locale::Es, "v1.0"),
        "ya existe una referencia 'v1.0' en este repo"
    );
}

#[test]
fn not_a_branch_carries_the_branch_name() {
    assert_eq!(not_a_branch(Locale::Es, "main"), "'main' no es una rama de este repo");
}

#[test]
fn not_a_tag_carries_the_tag_name() {
    assert_eq!(not_a_tag(Locale::Es, "v1.0"), "'v1.0' no es un tag de este repo");
}

#[test]
fn release_not_found_carries_the_tag_name() {
    assert_eq!(
        release_not_found(Locale::Es, "v1.0"),
        "no existe un release para el tag 'v1.0'"
    );
}

#[test]
fn asset_already_exists_carries_the_filename() {
    assert!(asset_already_exists(Locale::Es, "a.bin").contains("a.bin"));
}

#[test]
fn asset_not_found_carries_the_filename() {
    assert!(asset_not_found(Locale::Es, "a.bin").contains("a.bin"));
}

#[test]
fn repo_not_found_in_bucket_differs_from_the_plain_variant() {
    let plain = repo_not_found(Locale::Es, "acme/proj");
    let en_bucket = repo_not_found_in_bucket(Locale::Es, "acme/proj");
    assert_ne!(plain, en_bucket);
    assert_eq!(en_bucket, "repo 'acme/proj' no existe en el bucket");
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
fn invalid_namespace_carries_the_namespace() {
    assert!(invalid_namespace(Locale::Es, "foo").contains("foo"));
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
fn oidc_fields_go_together_differs_by_locale() {
    assert_ne!(oidc_fields_go_together(Locale::Es), oidc_fields_go_together(Locale::En));
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
fn invalid_csrf_differs_by_locale() {
    assert_ne!(invalid_csrf(Locale::Es), invalid_csrf(Locale::En));
}

#[test]
fn only_your_own_account_differs_by_locale() {
    assert_ne!(only_your_own_account(Locale::Es), only_your_own_account(Locale::En));
}

#[test]
fn file_too_large_carries_size_and_limit() {
    let msg = file_too_large(Locale::Es, 100, 50);
    assert!(msg.contains("100") && msg.contains("50"));
}

#[test]
fn invalid_snapshot_format_carries_the_format() {
    assert!(invalid_snapshot_format(Locale::Es, "rar").contains("rar"));
}

#[test]
fn no_asset_on_release_carries_filename_and_tag() {
    let msg = no_asset_on_release(Locale::Es, "a.bin", "v1.0");
    assert!(msg.contains("a.bin") && msg.contains("v1.0"));
}

#[test]
fn creating_temp_dir_for_asset_differs_by_locale() {
    assert_ne!(
        creating_temp_dir_for_asset(Locale::Es),
        creating_temp_dir_for_asset(Locale::En)
    );
}

#[test]
fn type_name_to_confirm_deletion_carries_the_name() {
    assert!(type_name_to_confirm_deletion(Locale::Es, "acme/proj").contains("acme/proj"));
}

#[test]
fn type_name_to_confirm_move_carries_the_name() {
    assert!(type_name_to_confirm_move(Locale::Es, "acme/proj").contains("acme/proj"));
}

#[test]
fn type_name_variants_differ_by_action() {
    assert_ne!(
        type_name_to_confirm_deletion(Locale::Es, "x"),
        type_name_to_confirm_move(Locale::Es, "x")
    );
}

#[test]
fn namespace_not_found_carries_the_namespace() {
    assert!(namespace_not_found(Locale::Es, "foo").contains("foo"));
}

#[test]
fn write_permission_required_differs_by_locale() {
    assert_ne!(
        write_permission_required(Locale::Es),
        write_permission_required(Locale::En)
    );
}

#[test]
fn insufficient_repo_permission_differs_by_locale() {
    assert_ne!(
        insufficient_repo_permission(Locale::Es),
        insufficient_repo_permission(Locale::En)
    );
}

#[test]
fn release_already_exists_for_tag_carries_the_tag() {
    assert!(release_already_exists_for_tag(Locale::Es, "v1.0").contains("v1.0"));
}

#[test]
fn invalid_multipart_carries_the_error() {
    assert!(invalid_multipart(Locale::Es, "boom").contains("boom"));
}

#[test]
fn creating_temp_file_carries_the_error() {
    assert!(creating_temp_file(Locale::Es, "boom").contains("boom"));
}

#[test]
fn reading_file_carries_the_error() {
    assert!(reading_file(Locale::Es, "boom").contains("boom"));
}

#[test]
fn asset_too_large_carries_the_limit() {
    assert!(asset_too_large(Locale::Es, 100).contains("100"));
}

#[test]
fn missing_file_to_upload_differs_by_locale() {
    assert_ne!(missing_file_to_upload(Locale::Es), missing_file_to_upload(Locale::En));
}

#[test]
fn building_set_cookie_differs_by_locale() {
    assert_ne!(building_set_cookie(Locale::Es), building_set_cookie(Locale::En));
}

#[test]
fn oidc_not_configured_differs_by_locale() {
    assert_ne!(oidc_not_configured(Locale::Es), oidc_not_configured(Locale::En));
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

#[test]
fn oidc_provider_rejected_login_carries_err_and_desc() {
    let msg = oidc_provider_rejected_login(Locale::Es, "access_denied", "user cancelled");
    assert!(msg.contains("access_denied") && msg.contains("user cancelled"));
}

#[test]
fn missing_code_param_differs_by_locale() {
    assert_ne!(missing_code_param(Locale::Es), missing_code_param(Locale::En));
}

#[test]
fn missing_state_param_differs_by_locale() {
    assert_ne!(missing_state_param(Locale::Es), missing_state_param(Locale::En));
}

#[test]
fn no_oidc_login_in_progress_differs_by_locale() {
    assert_ne!(
        no_oidc_login_in_progress(Locale::Es),
        no_oidc_login_in_progress(Locale::En)
    );
}

#[test]
fn invalid_oidc_flow_cookie_differs_by_locale() {
    assert_ne!(
        invalid_oidc_flow_cookie(Locale::Es),
        invalid_oidc_flow_cookie(Locale::En)
    );
}

#[test]
fn corrupt_oidc_flow_cookie_differs_by_locale() {
    assert_ne!(
        corrupt_oidc_flow_cookie(Locale::Es),
        corrupt_oidc_flow_cookie(Locale::En)
    );
}

#[test]
fn oidc_state_mismatch_differs_by_locale() {
    assert_ne!(oidc_state_mismatch(Locale::Es), oidc_state_mismatch(Locale::En));
}

#[test]
fn failed_to_build_oidc_client_differs_by_locale() {
    assert_ne!(
        failed_to_build_oidc_client(Locale::Es),
        failed_to_build_oidc_client(Locale::En)
    );
}

#[test]
fn code_token_exchange_failed_differs_by_locale() {
    assert_ne!(
        code_token_exchange_failed(Locale::Es),
        code_token_exchange_failed(Locale::En)
    );
}

#[test]
fn oidc_missing_id_token_differs_by_locale() {
    assert_ne!(oidc_missing_id_token(Locale::Es), oidc_missing_id_token(Locale::En));
}

#[test]
fn failed_to_verify_id_token_differs_by_locale() {
    assert_ne!(
        failed_to_verify_id_token(Locale::Es),
        failed_to_verify_id_token(Locale::En)
    );
}

#[test]
fn oidc_email_not_verified_differs_by_locale() {
    assert_ne!(oidc_email_not_verified(Locale::Es), oidc_email_not_verified(Locale::En));
}

#[test]
fn id_token_missing_email_claim_differs_by_locale() {
    assert_ne!(
        id_token_missing_email_claim(Locale::Es),
        id_token_missing_email_claim(Locale::En)
    );
}

#[test]
fn missing_sha_param_differs_by_locale() {
    assert_ne!(missing_sha_param(Locale::Es), missing_sha_param(Locale::En));
}

#[test]
fn missing_tag_param_differs_by_locale() {
    assert_ne!(missing_tag_param(Locale::Es), missing_tag_param(Locale::En));
}

#[test]
fn missing_number_param_differs_by_locale() {
    assert_ne!(missing_number_param(Locale::Es), missing_number_param(Locale::En));
}

#[test]
fn missing_filename_param_differs_by_locale() {
    assert_ne!(missing_filename_param(Locale::Es), missing_filename_param(Locale::En));
}

#[test]
fn missing_base_param_differs_by_locale() {
    assert_ne!(missing_base_param(Locale::Es), missing_base_param(Locale::En));
}

#[test]
fn missing_head_param_differs_by_locale() {
    assert_ne!(missing_head_param(Locale::Es), missing_head_param(Locale::En));
}

#[test]
fn blame_requires_path_differs_by_locale() {
    assert_ne!(blame_requires_path(Locale::Es), blame_requires_path(Locale::En));
}

#[test]
fn unknown_view_carries_the_view() {
    assert!(unknown_view(Locale::Es, "bogus").contains("bogus"));
}

#[test]
fn merge_request_not_found_carries_the_number() {
    assert!(merge_request_not_found(Locale::Es, 42).contains("42"));
}

#[test]
fn merge_request_wrong_status_differs_by_locale() {
    assert_ne!(
        merge_request_wrong_status(Locale::Es),
        merge_request_wrong_status(Locale::En)
    );
}
#[test]
fn source_repo_not_a_fork_differs_by_locale() {
    assert_ne!(source_repo_not_a_fork(Locale::Es), source_repo_not_a_fork(Locale::En));
}
