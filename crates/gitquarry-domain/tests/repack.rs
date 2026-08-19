use gitquarry_domain::commands::repack::*;

#[test]
fn human_bytes_picks_the_right_unit() {
    assert_eq!(human_bytes(500), "500B");
    assert_eq!(human_bytes(2048), "2.0K");
    assert_eq!(human_bytes(5 * 1024 * 1024), "5.0M");
}
