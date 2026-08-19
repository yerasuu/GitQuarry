//! Federation-issued fine-grained-token resolution for git-smart-HTTP
//! Basic-Auth. Split out of `webui::federation_client` (peer-browse
//! informational lookups, presentation-only, moving to its own crate) since
//! [`crate::git::http`]'s auth path needs this one specifically — `git::http`
//! stays in core, so this can't live in a crate core would have to depend
//! back on.

use crate::app::FederationLink;
use gitquarry_wire::Uid;
use serde::{Deserialize, Serialize};

const INTERNAL_TOKEN_HEADER: &str = "x-internal-token";

#[derive(Serialize)]
struct TokenResolveRequest<'a> {
    secret: &'a str,
    repo_uid: &'a Uid,
}

#[derive(Deserialize)]
pub struct TokenResolveOutcome {
    pub ok: bool,
    pub username: Option<String>,
    /// `false` = read-only, capped at `Permission::Reporter` regardless of
    /// the owner's real tier. `true` = read+write, capped only at the
    /// owner's own real permission — [`crate::git::http`]'s own doc comment
    /// on why fine-grained-token auth needs this two-step split now that
    /// `AccessToken` storage lives in `gitquarry-federation`.
    pub write: Option<bool>,
}

const TOKEN_NOT_RESOLVED: TokenResolveOutcome = TokenResolveOutcome {
    ok: false,
    username: None,
    write: None,
};

/// `POST /_internal-out/tokens/resolve` — [`crate::git::http`]'s
/// fine-grained-token-as-git-credential path. `ok: false` for an unknown/
/// expired/revoked secret, a secret that doesn't list `repo_uid`, or an
/// unreachable federation process — all three mean the same thing to the
/// caller: this credential doesn't authorize this repo, fail closed.
pub async fn resolve_token(federation: &FederationLink, secret: &str, repo_uid: &Uid) -> TokenResolveOutcome {
    let url = format!(
        "{}/_internal-out/tokens/resolve",
        federation.base_url.trim_end_matches('/')
    );
    let Ok(resp) = federation
        .http
        .post(&url)
        .header(INTERNAL_TOKEN_HEADER, &federation.internal_token)
        .json(&TokenResolveRequest { secret, repo_uid })
        .send()
        .await
    else {
        return TOKEN_NOT_RESOLVED;
    };
    resp.json::<TokenResolveOutcome>().await.unwrap_or(TOKEN_NOT_RESOLVED)
}
