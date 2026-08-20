//! The release notes `/_changelog` compiles in have to be *this* version's.
//!
//! The build already fails if `tag-changelog/v<version>.md` is missing, so
//! what is left to check is the case it can't see: a file present under the
//! right name whose contents were copied from the previous release and
//! never retitled. The page would then confidently describe the wrong one.

/// The heading is the file's own claim about which release it documents;
/// it has to agree with the version the crate was built at.
#[test]
fn the_release_notes_are_titled_with_the_version_they_ship_in() {
    let version = env!("CARGO_PKG_VERSION");
    let notes = std::fs::read_to_string(format!(
        "{}/../../tag-changelog/v{version}.md",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("no release notes for this version");

    let heading = notes.lines().next().unwrap_or_default().trim();

    assert_eq!(heading, format!("# v{version}"));
}
