use gitquarry_core::storage::remote::Manifest;
use gitquarry_webui::webui::repo_settings::default_branch::*;
use std::collections::BTreeMap;

#[test]
fn default_branch_form_marks_the_current_head_selected() {
    let mut refs = BTreeMap::new();
    refs.insert("refs/heads/main".to_string(), "aaa".to_string());
    refs.insert("refs/heads/dev".to_string(), "bbb".to_string());
    refs.insert("refs/tags/v1".to_string(), "ccc".to_string()); // not a branch, excluded
    let manifest = Manifest {
        refs,
        head: Some("refs/heads/dev".to_string()),
    };
    let html =
        render_default_branch_form("proyectoA", &manifest, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<option value="dev" selected>dev</option>"#));
    assert!(html.contains(r#"<option value="main">main</option>"#));
    assert!(!html.contains("v1"));
}

#[test]
fn default_branch_form_empty_without_any_branch() {
    let manifest = Manifest {
        refs: BTreeMap::new(),
        head: None,
    };
    assert_eq!(
        render_default_branch_form("proyectoA", &manifest, "tok", None, gitquarry_i18n::Locale::En).into_string(),
        ""
    );
}
