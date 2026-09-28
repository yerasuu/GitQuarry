use gitquarry_webui::webui::page_chrome::*;

#[test]
fn layout_includes_the_mobile_nav_checkbox_toggle() {
    let html = layout("test", "", "", "", "", "", gitquarry_i18n::Locale::En);
    assert!(html.contains(r#"input type="checkbox" id="nav-toggle" class="nav-toggle-input""#));
    assert!(html.contains(r#"label for="nav-toggle" class="nav-toggle-btn""#));
}

#[test]
fn topic_badges_link_to_the_index_search() {
    let mut topics = std::collections::BTreeSet::new();
    topics.insert("rust".to_string());
    let html = topic_badges(&topics);
    assert!(html.contains(r#"href="/?q=rust""#));
    assert!(html.contains(">rust<"));
}

#[test]
fn topic_badges_empty_for_no_topics() {
    assert_eq!(topic_badges(&std::collections::BTreeSet::new()), "");
}

#[test]
fn tabs_bar_marks_the_active_tab_with_aria_current() {
    let tabs: Vec<(&str, String, String)> = vec![
        ("summary", "/x?view=summary".to_string(), "summary".to_string()),
        ("log", "/x?view=log".to_string(), "log".to_string()),
    ];
    let html = tabs_bar(&tabs, "log", &maud::html! {}, "page sections", "page sections menu").into_string();
    let log_link = html.split("</a>").find(|s| s.contains(">log")).unwrap();
    assert!(log_link.contains(r#"aria-current="page""#), "{log_link}");
    let summary_link = html.split("</a>").find(|s| s.contains(">summary")).unwrap();
    assert!(!summary_link.contains("aria-current"), "{summary_link}");
}

#[test]
fn admin_tabs_drop_peers_without_federation() {
    let with = admin_header("settings", true, gitquarry_i18n::Locale::En);
    assert!(with.contains(r#"href="/_peers""#), "{with}");
    let without = admin_header("settings", false, gitquarry_i18n::Locale::En);
    assert!(!without.contains("/_peers"), "{without}");
    // The rest of the bar is untouched -- only the federation entry goes.
    assert!(without.contains(r#"href="/_users""#), "{without}");
    assert!(without.contains(r#"href="/_audit""#), "{without}");
}

#[test]
fn account_sidebar_drops_tokens_without_federation() {
    let with = account_settings_page("profile", maud::html! {}, true, gitquarry_i18n::Locale::En);
    assert!(with.into_string().contains(r#"href="/_account/tokens""#));
    let without = account_settings_page("profile", maud::html! {}, false, gitquarry_i18n::Locale::En).into_string();
    assert!(!without.contains("/_account/tokens"), "{without}");
    assert!(without.contains(r#"href="/_account/pat""#), "{without}");
}
