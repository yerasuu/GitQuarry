use gitquarry_webui::webui::flash::Flash;
use gitquarry_webui::webui::repo_new::*;

#[test]
fn new_repo_form_lists_own_username_and_every_group() {
    let group_names = vec!["acme".to_string(), "widgets".to_string()];
    let html = render_new_form("alice", &group_names, "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"<option value="alice">alice (me)</option>"#));
    assert!(html.contains(r#"<option value="acme">acme</option>"#));
    assert!(html.contains(r#"<option value="widgets">widgets</option>"#));
}

#[test]
fn new_repo_form_shows_the_flash_when_given_one() {
    let html = render_new_form(
        "alice",
        &[],
        "tok",
        Some(Flash::Err("a repo with that name already exists.")),
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.contains("a repo with that name already exists."));
    assert!(html.contains("flash-err"));
}

#[test]
fn new_repo_form_has_an_optional_template_field() {
    let html = render_new_form("alice", &[], "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.contains(r#"input name="template""#), "{html}");
}

/// Regression: the form is a *plain* POST, not `data-async` — a successful
/// create redirects to the new repo's own page (`Redirect::to` in
/// `handler_new_post`), a real navigation `data-async`'s `fetch()` can't
/// make happen client-side (it would follow the redirect itself and hand
/// the submit handler the destination page's full HTML to graft into
/// `#new-repo-body` as if it were a fragment). A rejected name/namespace
/// still re-renders the whole page in place of the old fragment swap.
#[test]
fn form_is_a_plain_post_not_an_async_fragment_swap() {
    let html = render_new_form("alice", &[], "tok", None, gitquarry_i18n::Locale::En).into_string();
    assert!(html.starts_with(r#"<div id="new-repo-body">"#), "{html}");
    assert!(!html.contains("data-async"), "{html}");
}
