use gitquarry_auth_crypto::oidc::*;

#[tokio::test]
async fn none_when_all_four_fields_unset() {
    let settings = OidcSettings::default();
    assert!(discover(&settings).await.unwrap().is_none());
}

#[tokio::test]
async fn errors_when_configured_partway() {
    let settings = OidcSettings {
        issuer_url: Some("https://idp.example.com".to_string()),
        client_id: Some("client".to_string()),
        // secret and redirect_base_url left unset on purpose.
        ..OidcSettings::default()
    };
    let err = discover(&settings).await.unwrap_err();
    assert!(err.to_string().contains("all be set or none"), "{err}");
}
