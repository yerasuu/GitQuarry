use gitquarry_domain::commands::fsck::*;

#[test]
fn classify_accepts_known_shapes() {
    assert!(classify("refs.json").is_none());
    assert!(classify(".conf/config.json").is_none());
    assert!(classify("objects/pack/pack-abc123.pack").is_none());
    assert!(classify("objects/pack/pack-abc123.idx").is_none());
}

#[test]
fn classify_flags_folder_markers() {
    assert!(matches!(classify(""), Some(Finding::EmptyFolderMarker)));
    assert!(matches!(classify(".conf/"), Some(Finding::EmptyFolderMarker)));
    assert!(matches!(classify("objects/"), Some(Finding::EmptyFolderMarker)));
}

#[test]
fn classify_flags_anything_else_as_unexpected() {
    assert!(matches!(classify("README.md"), Some(Finding::Unexpected)));
    assert!(matches!(classify("objects/pack/junk.txt"), Some(Finding::Unexpected)));
}
