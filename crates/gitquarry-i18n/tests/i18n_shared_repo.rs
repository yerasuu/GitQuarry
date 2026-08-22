use gitquarry_i18n::Locale;
use gitquarry_i18n::shared::repo::*;

#[test]
fn repo_not_found_differs_by_locale_and_carries_the_repo_name() {
    let es = repo_not_found(Locale::Es, "acme/proj");
    let en = repo_not_found(Locale::En, "acme/proj");
    assert_ne!(es, en);
    assert_eq!(es, "repo 'acme/proj' no existe");
    assert_eq!(en, "repo 'acme/proj' doesn't exist");
}

#[test]
fn repo_deleted_differs_by_locale() {
    assert_eq!(repo_deleted(Locale::Es), "(repo eliminado)");
    assert_ne!(repo_deleted(Locale::Es), repo_deleted(Locale::En));
}

#[test]
fn repo_already_exists_at_carries_the_name() {
    assert_eq!(
        repo_already_exists_at(Locale::Es, "acme/proj"),
        "ya existe un repo en 'acme/proj'"
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
fn repo_not_found_in_bucket_differs_from_the_plain_variant() {
    let plain = repo_not_found(Locale::Es, "acme/proj");
    let en_bucket = repo_not_found_in_bucket(Locale::Es, "acme/proj");
    assert_ne!(plain, en_bucket);
    assert_eq!(en_bucket, "repo 'acme/proj' no existe en el bucket");
}

#[test]
fn invalid_namespace_carries_the_namespace() {
    assert!(invalid_namespace(Locale::Es, "foo").contains("foo"));
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
