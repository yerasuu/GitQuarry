use axum::http::HeaderMap;
use gitquarry_webui::webui::locale_pref::*;

#[test]
fn cookie_locale_reads_explicit_overrides_only() {
    let mut es = HeaderMap::new();
    es.insert(axum::http::header::COOKIE, "locale=es".parse().unwrap());
    assert_eq!(cookie_locale(&es), Some(gitquarry_i18n::Locale::Es));

    let mut en = HeaderMap::new();
    en.insert(axum::http::header::COOKIE, "locale=en".parse().unwrap());
    assert_eq!(cookie_locale(&en), Some(gitquarry_i18n::Locale::En));

    assert_eq!(cookie_locale(&HeaderMap::new()), None);
    let mut garbage = HeaderMap::new();
    garbage.insert(axum::http::header::COOKIE, "locale=fr".parse().unwrap());
    assert_eq!(cookie_locale(&garbage), None);
}
