use gitquarry_federation::gossip::*;
use std::collections::HashSet;

#[test]
fn report_upserts_and_accumulates_distinct_reporters() {
    let table = GossipTable::default();
    table.report("acme", "https://acme.example", "https://acme.example", Some("peer-a"));
    table.report("acme", "https://acme.example", "https://acme.example", Some("peer-b"));
    let snap = table.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].name, "acme");
}

#[test]
fn auto_link_requires_both_enough_linked_sources_and_enough_age() {
    let table = GossipTable::default();
    table.report("acme", "https://acme.example", "https://acme.example", Some("linked-a"));
    table.report("acme", "https://acme.example", "https://acme.example", Some("linked-b"));

    let linked: HashSet<String> = ["linked-a".to_string(), "linked-b".to_string()].into_iter().collect();
    // Only 2 qualifying sources, threshold is 3: not yet eligible even though old enough (age 0 >= 0).
    assert!(table.auto_link_candidates(3, 0, &linked).is_empty());
    // Lower the bar to 2 sources: now eligible (age requirement is 0).
    assert_eq!(table.auto_link_candidates(2, 0, &linked).len(), 1);
}

#[test]
fn reannouncing_a_corroborated_name_with_other_urls_starts_corroboration_over() {
    let table = GossipTable::default();
    table.report("acme", "https://acme.example", "https://acme.example", Some("linked-a"));
    table.report("acme", "https://acme.example", "https://acme.example", Some("linked-b"));
    let linked: HashSet<String> = ["linked-a".to_string(), "linked-b".to_string()].into_iter().collect();
    assert_eq!(table.auto_link_candidates(2, 0, &linked).len(), 1);

    table.report("acme", "https://evil.example", "https://evil.example", None);
    assert!(table.auto_link_candidates(1, 0, &linked).is_empty());
    // The old sources vouched for acme.example, so they stay forgotten even
    // once one of them reports the new URLs too — only fresh reports count.
    table.report("acme", "https://evil.example", "https://evil.example", Some("linked-a"));
    assert!(table.auto_link_candidates(2, 0, &linked).is_empty());
}

#[test]
fn auto_link_ignores_reporters_that_arent_currently_linked() {
    let table = GossipTable::default();
    table.report(
        "acme",
        "https://acme.example",
        "https://acme.example",
        Some("stranger-1"),
    );
    table.report(
        "acme",
        "https://acme.example",
        "https://acme.example",
        Some("stranger-2"),
    );
    table.report(
        "acme",
        "https://acme.example",
        "https://acme.example",
        Some("stranger-3"),
    );
    let linked: HashSet<String> = HashSet::new();
    assert!(table.auto_link_candidates(3, 0, &linked).is_empty());
}

#[test]
fn auto_link_never_reoffers_an_already_linked_name() {
    let table = GossipTable::default();
    table.report("acme", "https://acme.example", "https://acme.example", Some("linked-a"));
    let linked: HashSet<String> = ["acme".to_string(), "linked-a".to_string()].into_iter().collect();
    assert!(table.auto_link_candidates(1, 0, &linked).is_empty());
}

#[test]
fn report_ignores_an_empty_name_or_url() {
    let table = GossipTable::default();
    table.report("", "https://acme.example", "https://acme.example", None);
    table.report("acme", "", "", None);
    assert!(table.snapshot().is_empty());
}
