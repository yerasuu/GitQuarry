use gitquarry_webui::webui::health::*;

#[test]
fn render_healthz_is_a_plain_ok() {
    assert_eq!(render_healthz(), "ok");
}

#[test]
fn render_metrics_includes_every_gauge_with_its_value() {
    let body = render_metrics(12.5, 3, 2);
    assert!(body.contains("gitquarry_up 1"));
    assert!(body.contains(&format!(
        "gitquarry_build_info{{version=\"{}\"}} 1",
        env!("CARGO_PKG_VERSION")
    )));
    assert!(body.contains("gitquarry_uptime_seconds 12.5"));
    assert!(body.contains("gitquarry_login_lockouts_active 3"));
    assert!(body.contains("gitquarry_git_rate_limited_active 2"));
}

#[test]
fn render_metrics_zero_lockouts_still_shows_the_gauge() {
    let body = render_metrics(0.0, 0, 0);
    assert!(body.contains("gitquarry_login_lockouts_active 0"));
    assert!(body.contains("gitquarry_git_rate_limited_active 0"));
}
