use axum::extract::Form;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use gitquarry_webui::webui::diff_view::*;

#[test]
fn effective_diff_view_defaults_to_unified_without_a_cookie() {
    assert_eq!(effective_diff_view(&HeaderMap::new()), DiffView::Unified);

    let mut split = HeaderMap::new();
    split.insert(axum::http::header::COOKIE, "diff-view=split".parse().unwrap());
    assert_eq!(effective_diff_view(&split), DiffView::Split);

    let mut garbage = HeaderMap::new();
    garbage.insert(axum::http::header::COOKIE, "diff-view=nonsense".parse().unwrap());
    assert_eq!(effective_diff_view(&garbage), DiffView::Unified);
}

#[test]
fn diff_view_buttons_mark_the_current_one_active() {
    let html = render_diff_view_buttons("/acme/proj?view=commit&sha=abc", DiffView::Split);
    assert_eq!(html.matches("size-btn active").count(), 1);
    assert!(html.contains(r#"value="unified""#));
    assert!(html.contains(r#"value="split""#));
    let split_form = html.split("<form").nth(2).unwrap();
    assert!(split_form.contains("active"));
}

#[tokio::test]
async fn toggle_diff_view_sets_the_explicit_pick_and_redirects() {
    let form = Form(DiffViewForm {
        view: "split".to_string(),
        return_to: "/acme/proj".to_string(),
    });
    let resp = handler_toggle_diff_view(HeaderMap::new(), form).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let location = resp.headers().get(axum::http::header::LOCATION).unwrap();
    assert_eq!(location, "/acme/proj");
    let set_cookie = resp
        .headers()
        .get(axum::http::header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("diff-view=split"));
}

#[tokio::test]
async fn toggle_diff_view_rejects_an_off_site_return_to() {
    let form = Form(DiffViewForm {
        view: "unified".to_string(),
        return_to: "//evil.example".to_string(),
    });
    let resp = handler_toggle_diff_view(HeaderMap::new(), form).await;
    let location = resp.headers().get(axum::http::header::LOCATION).unwrap();
    assert_eq!(location, "/");
}
