use gitquarry_federation::time::*;

#[test]
fn epoch_formats_as_1970() {
    assert_eq!(format_unix_secs(0), "1970-01-01T00:00:00Z");
}
