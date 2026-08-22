use gitquarry_i18n::Locale;
use gitquarry_i18n::shared::release::*;

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
