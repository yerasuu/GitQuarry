//! OIDC client construction: discovery against the configured issuer, once
//! at startup. The request-handling side (login/callback/logout routes,
//! session cookie, mapping the verified email to a `User`) lives in
//! `gitquarry_webui::auth::web::login` — this module only builds the client
//! type both that and `gitquarry_core::app::AppContext` need to name.
//!
//! A standalone primitive with no dependency on `gitquarry-core`'s own
//! config types — [`OidcSettings`] carries just the 4 `oidc_*` fields a
//! caller already has on its own `ServerSettings`, so this crate never needs
//! to depend back on core for them.

use anyhow::{Context, Result, bail};
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use openidconnect::{ClientId, ClientSecret, EndpointMaybeSet, EndpointNotSet, EndpointSet, IssuerUrl, RedirectUrl};

/// The four `oidc_*` fields a caller's own config carries — every one of
/// them `None` means "OIDC off"; any other partial combination is a config
/// error [`discover`] reports. Kept as owned `String`s (not borrowed) so a
/// caller can build this from its own config struct without fighting
/// lifetimes across an `.await`.
#[derive(Debug, Clone, Default)]
pub struct OidcSettings {
    pub issuer_url: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub redirect_base_url: Option<String>,
}

/// The four [`OidcSettings`] fields fully pin down every type parameter
/// `openidconnect`'s type-state builder tracks (which endpoints are known):
/// auth comes from discovery unconditionally (`EndpointSet`), token is
/// forced via `set_token_uri` right after `from_provider_metadata` so it's a
/// definite `EndpointSet` instead of discovery's `EndpointMaybeSet`,
/// userinfo stays whatever discovery found (`EndpointMaybeSet` — nothing
/// here calls a method that requires it to be set). Redirect isn't part of
/// the type state at all. Naming this concrete type once here is what lets
/// `AppContext` store it as a plain field instead of every caller
/// re-deriving it.
pub type OidcClient =
    CoreClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet, EndpointMaybeSet>;

/// `None` if OIDC isn't configured at all (all four fields unset — the
/// default, and the only state where admin pages stay exactly as open as
/// they've always been). An error if it's configured *partway* (a typo
/// dropping one field shouldn't silently disable login) or discovery
/// against the issuer fails outright.
pub async fn discover(settings: &OidcSettings) -> Result<Option<OidcClient>> {
    let fields = (
        settings.issuer_url.as_deref(),
        settings.client_id.as_deref(),
        settings.client_secret.as_deref(),
        settings.redirect_base_url.as_deref(),
    );
    let (issuer, client_id, client_secret, redirect_base) = match fields {
        (Some(i), Some(c), Some(s), Some(r)) => (i, c, s, r),
        (None, None, None, None) => return Ok(None),
        _ => bail!(
            "OIDC configured only halfway: oidc_issuer_url, oidc_client_id, oidc_client_secret and \
             oidc_redirect_base_url must all be set or none at all"
        ),
    };

    // Redirects here would let a malicious discovery/token endpoint response
    // point the client somewhere else entirely (SSRF), same caution the
    // crate's own examples call out.
    let http_client = openidconnect::reqwest::ClientBuilder::new()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .context("couldn't build the HTTP client for OIDC")?;

    let issuer_url = IssuerUrl::new(issuer.to_string()).context("invalid oidc_issuer_url")?;
    let metadata = CoreProviderMetadata::discover_async(issuer_url, &http_client)
        .await
        .with_context(|| format!("couldn't discover the OIDC provider at {issuer}"))?;
    let token_url = metadata
        .token_endpoint()
        .cloned()
        .context("the OIDC provider doesn't publish a token_endpoint in its discovery document")?;
    let redirect_url = RedirectUrl::new(format!("{}/_callback", redirect_base.trim_end_matches('/')))
        .context("invalid oidc_redirect_base_url")?;

    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(client_id.to_string()),
        Some(ClientSecret::new(client_secret.to_string())),
    )
    .set_token_uri(token_url)
    .set_redirect_uri(redirect_url);

    Ok(Some(client))
}
