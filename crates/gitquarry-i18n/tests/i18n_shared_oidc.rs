use gitquarry_i18n::Locale;
use gitquarry_i18n::shared::oidc::*;

#[test]
fn oidc_not_configured_differs_by_locale() {
    assert_ne!(oidc_not_configured(Locale::Es), oidc_not_configured(Locale::En));
}

#[test]
fn oidc_provider_rejected_login_carries_err_and_desc() {
    let msg = oidc_provider_rejected_login(Locale::Es, "access_denied", "user cancelled");
    assert!(msg.contains("access_denied") && msg.contains("user cancelled"));
}

#[test]
fn missing_code_param_differs_by_locale() {
    assert_ne!(missing_code_param(Locale::Es), missing_code_param(Locale::En));
}

#[test]
fn missing_state_param_differs_by_locale() {
    assert_ne!(missing_state_param(Locale::Es), missing_state_param(Locale::En));
}

#[test]
fn no_oidc_login_in_progress_differs_by_locale() {
    assert_ne!(
        no_oidc_login_in_progress(Locale::Es),
        no_oidc_login_in_progress(Locale::En)
    );
}

#[test]
fn invalid_oidc_flow_cookie_differs_by_locale() {
    assert_ne!(
        invalid_oidc_flow_cookie(Locale::Es),
        invalid_oidc_flow_cookie(Locale::En)
    );
}

#[test]
fn corrupt_oidc_flow_cookie_differs_by_locale() {
    assert_ne!(
        corrupt_oidc_flow_cookie(Locale::Es),
        corrupt_oidc_flow_cookie(Locale::En)
    );
}

#[test]
fn oidc_state_mismatch_differs_by_locale() {
    assert_ne!(oidc_state_mismatch(Locale::Es), oidc_state_mismatch(Locale::En));
}

#[test]
fn failed_to_build_oidc_client_differs_by_locale() {
    assert_ne!(
        failed_to_build_oidc_client(Locale::Es),
        failed_to_build_oidc_client(Locale::En)
    );
}

#[test]
fn code_token_exchange_failed_differs_by_locale() {
    assert_ne!(
        code_token_exchange_failed(Locale::Es),
        code_token_exchange_failed(Locale::En)
    );
}

#[test]
fn oidc_missing_id_token_differs_by_locale() {
    assert_ne!(oidc_missing_id_token(Locale::Es), oidc_missing_id_token(Locale::En));
}

#[test]
fn failed_to_verify_id_token_differs_by_locale() {
    assert_ne!(
        failed_to_verify_id_token(Locale::Es),
        failed_to_verify_id_token(Locale::En)
    );
}

#[test]
fn oidc_email_not_verified_differs_by_locale() {
    assert_ne!(oidc_email_not_verified(Locale::Es), oidc_email_not_verified(Locale::En));
}

#[test]
fn id_token_missing_email_claim_differs_by_locale() {
    assert_ne!(
        id_token_missing_email_claim(Locale::Es),
        id_token_missing_email_claim(Locale::En)
    );
}
