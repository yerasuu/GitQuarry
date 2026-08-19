use gitquarry_webui::webui::repo_settings::protected_branches::*;

use gitquarry_core::storage::entities::RepoConfig;
use gitquarry_core::storage::remote::Manifest;
use std::collections::BTreeMap;

#[test]
fn protected_branches_form_empty_without_any_branch() {
    let manifest = Manifest {
        refs: BTreeMap::new(),
        head: None,
    };
    let config = RepoConfig::default();
    assert_eq!(
        render_protected_branches_form("proyectoA", &config, &manifest, "tok", None, gitquarry_i18n::Locale::En)
            .into_string(),
        ""
    );
}

#[test]
fn protected_branches_form_shows_current_value_and_available_branches() {
    let mut refs = BTreeMap::new();
    refs.insert("refs/heads/main".to_string(), "abc".to_string());
    refs.insert("refs/heads/dev".to_string(), "def".to_string());
    let manifest = Manifest { refs, head: None };
    let mut config = RepoConfig::default();
    config.protected_branches.insert("main".to_string());
    let html = render_protected_branches_form("proyectoA", &config, &manifest, "tok", None, gitquarry_i18n::Locale::En)
        .into_string();
    assert!(html.contains(r#"value="main""#));
    assert!(html.contains("main, dev") || html.contains("dev, main"));
    assert!(html.contains(r#"value="set_protected_branches""#));
}
