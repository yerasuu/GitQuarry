use gitquarry_core::auth::entities::PersonalAccessToken;
use gitquarry_webui::auth::web::pat::render_pat_page;

/// Regression: `revoke` (per-row) and `create` both target `#pat-body` —
/// either one has to refresh the whole wrapper (the flash banner and, for
/// `create`, the one-time secret reveal both sit above the table, outside
/// either form) — see `render_pat_page`'s own doc comment. Losing either
/// form's `data-async` attributes silently brings back the browser's
/// "confirm resubmission" dialog on F5. One token in the list so the
/// per-row `revoke` form actually renders (`@for token in tokens` emits
/// nothing on an empty list).
#[test]
fn both_forms_share_one_async_target() {
    let tokens = [PersonalAccessToken {
        label: "ci".to_string(),
        ..Default::default()
    }];
    let html = render_pat_page(
        &tokens,
        &Default::default(),
        "tok",
        None,
        None,
        gitquarry_i18n::Locale::En,
    )
    .into_string();
    assert!(html.starts_with(r#"<div id="pat-body">"#), "{html}");
    assert_eq!(
        html.matches(r##"data-async data-async-target="#pat-body""##).count(),
        2,
        "{html}"
    );
}
