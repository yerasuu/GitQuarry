//! `/_account/tokens`: self-service fine-grained personal access tokens
//! (federation layer 4) plus linked peer credentials — proxied from core,
//! which already validated the caller's session and resolved their own
//! identity before ever reaching here (see `web::session`'s own doc
//! comment) — no username in the URL, this always operates on whichever
//! account `X-Gitquarry-User`/`-Uid` names. One page, two entity kinds, same
//! as the single-binary predecessor: a token is a credential *this*
//! instance mints for someone else to use against it; a peer credential is
//! the reverse, opaque, minted by the peer.

use crate::entities::{AccessToken, Peer};
use crate::state::State_;
use crate::web::session::{escape_html, forwarded_csrf, forwarded_user, forwarded_user_uid};
use crate::{access, domain};
use axum::extract::{Form, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;
use std::collections::BTreeSet;

fn unauthorized() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        "missing X-Gitquarry-User(-Uid) — this page must be reached through gitquarry-core's proxy",
    )
        .into_response()
}

/// `pub` unconditionally rather than the usual test-only-`pub` split other
/// whitebox-tested render functions in this codebase use: the gate that
/// actually matters is on the `tokens` module itself (`web::mod`'s
/// `pub mod tokens` vs. `mod tokens` split), so this struct is only ever
/// externally reachable when that module is public in the first place.
pub struct PageData {
    pub tokens: Vec<AccessToken>,
    pub peers: Vec<Peer>,
    pub peer_credentials: Vec<(String, access::PeerCredential)>,
}

async fn load_page_data(
    state: &crate::state::FederationState,
    username: &str,
    user_uid: &gitquarry_wire::Uid,
) -> PageData {
    PageData {
        tokens: domain::token::list_for_owner(state.backend.as_ref(), &state.cipher, username)
            .await
            .unwrap_or_default(),
        peers: access::list_all_peers(state.backend.as_ref(), &state.cipher)
            .await
            .unwrap_or_default(),
        peer_credentials: domain::peer_credential::list(state.backend.as_ref(), &state.cipher, user_uid)
            .await
            .unwrap_or_default(),
    }
}

pub(crate) async fn handler_tokens_get(State(state): State_, headers: HeaderMap) -> Response {
    let (Some(username), Some(user_uid)) = (forwarded_user(&headers), forwarded_user_uid(&headers)) else {
        return unauthorized();
    };
    let data = load_page_data(&state, &username, &user_uid).await;
    Html(render_page(&data, &forwarded_csrf(&headers), None, None)).into_response()
}

#[derive(Deserialize)]
pub(crate) struct TokenForm {
    action: String,
    #[serde(default)]
    uid: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    repos: String,
    #[serde(default)]
    write: String,
    #[serde(default)]
    expires_days: String,
    #[serde(default)]
    peer: String,
    #[serde(default)]
    secret: String,
}

pub(crate) async fn handler_tokens_post(
    State(state): State_,
    headers: HeaderMap,
    Form(form): Form<TokenForm>,
) -> Response {
    let (Some(username), Some(user_uid)) = (forwarded_user(&headers), forwarded_user_uid(&headers)) else {
        return unauthorized();
    };

    let mut pending_secret = None;
    let message = match form.action.as_str() {
        "create" => match create_token(&state, &username, &form).await {
            Ok(secret) => {
                pending_secret = Some(secret);
                "token created.".to_string()
            }
            Err(e) => e,
        },
        "revoke" => match domain::token::revoke(state.backend.as_ref(), &state.cipher, &username, &form.uid).await {
            Ok(true) => "revoked.".to_string(),
            Ok(false) => "no such token.".to_string(),
            Err(e) => format!("revoke failed: {e:#}"),
        },
        "link_peer" => {
            match domain::peer_credential::set(
                state.backend.as_ref(),
                &state.cipher,
                &user_uid,
                form.peer.trim(),
                form.secret.trim(),
                form.label.trim(),
            )
            .await
            {
                Ok(()) => "credential linked.".to_string(),
                Err(e) => e.to_string(),
            }
        }
        "unlink_peer" => {
            domain::peer_credential::delete(state.backend.as_ref(), &state.cipher, &user_uid, form.peer.trim()).await;
            "credential unlinked.".to_string()
        }
        other => format!("unknown action: {other}"),
    };

    let data = load_page_data(&state, &username, &user_uid).await;
    Html(render_page(
        &data,
        &forwarded_csrf(&headers),
        Some(message),
        pending_secret.as_deref(),
    ))
    .into_response()
}

/// Resolves every comma-separated repo name to a uid via core (a token
/// scopes itself by uid — see `entities::AccessToken::repos`'s own doc
/// comment) — existence-only, same as the single-binary predecessor's
/// `resolve_repo_name`: whether `username` can actually access it is a
/// live decision made at *use* time (`/_internal/authorize`), not gated
/// again here.
async fn create_token(
    state: &crate::state::FederationState,
    username: &str,
    form: &TokenForm,
) -> Result<String, String> {
    let mut repos = BTreeSet::new();
    for name in form.repos.split(',') {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        match state.core.uid_for_name(name).await {
            Ok(Some(uid)) => {
                repos.insert(uid);
            }
            _ => return Err(format!("repo doesn't exist: {name}")),
        }
    }
    let expires_in_days = (!form.expires_days.trim().is_empty())
        .then(|| form.expires_days.trim().parse::<u32>().ok())
        .flatten();
    let write = form.write == "on";
    match domain::token::create(
        state.backend.as_ref(),
        &state.cipher,
        username,
        form.label.trim(),
        repos,
        write,
        expires_in_days,
    )
    .await
    {
        Ok(Some((_token, secret))) => Ok(secret),
        Ok(None) => Err("token id collided, try again.".to_string()),
        Err(e) => Err(e.to_string()),
    }
}

/// Same "`pub` unconditionally, the module gate is what actually matters"
/// reasoning as [`PageData`]'s own doc comment.
pub fn render_page(data: &PageData, csrf: &str, message: Option<String>, pending_secret: Option<&str>) -> String {
    let csrf = escape_html(csrf);
    let banner = message
        .map(|m| format!("<p>{}</p>", escape_html(&m)))
        .unwrap_or_default();
    let secret_banner = pending_secret
        .map(|s| {
            format!(
                "<h4>your new token</h4><p>shown once, copy it now:</p><p><code>{}</code></p>",
                escape_html(s)
            )
        })
        .unwrap_or_default();

    let mut token_rows = String::new();
    if data.tokens.is_empty() {
        token_rows.push_str("<tr><td colspan=\"7\"><em>no tokens yet.</em></td></tr>");
    }
    for token in &data.tokens {
        let scope = if token.write { "read+write" } else { "read-only" };
        token_rows.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{scope}</td><td>{}</td><td>{}</td><td>{}</td><td>\
             <form method=\"post\" style=\"display:inline\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"action\" value=\"revoke\">\
             <input type=\"hidden\" name=\"uid\" value=\"{}\">\
             <button type=\"submit\">revoke</button></form></td></tr>",
            escape_html(&token.label),
            token.repos.len(),
            escape_html(&token.created_at),
            token.expires_at.as_deref().unwrap_or("never"),
            token.last_used_at.as_deref().unwrap_or("never"),
            escape_html(token.uid.as_str()),
        ));
    }

    let mut credential_rows = String::new();
    if data.peer_credentials.is_empty() {
        credential_rows.push_str("<tr><td colspan=\"4\"><em>no linked peer credentials yet.</em></td></tr>");
    }
    for (peer_name, credential) in &data.peer_credentials {
        let peer_name = escape_html(peer_name);
        credential_rows.push_str(&format!(
            "<tr><td>{peer_name}</td><td>{}</td><td>{}</td><td>\
             <form method=\"post\" style=\"display:inline\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"action\" value=\"unlink_peer\">\
             <input type=\"hidden\" name=\"peer\" value=\"{peer_name}\">\
             <button type=\"submit\">unlink</button></form></td></tr>",
            escape_html(&credential.label),
            escape_html(&credential.linked_at),
        ));
    }

    let link_form = if data.peers.is_empty() {
        "<p class=\"muted\">no peers linked on this instance yet.</p>".to_string()
    } else {
        let mut options = String::new();
        for peer in &data.peers {
            let name = escape_html(&peer.name);
            options.push_str(&format!("<option value=\"{name}\">{name}</option>"));
        }
        format!(
            "<h4>link a peer credential</h4>\
             <form method=\"post\">\
             <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"action\" value=\"link_peer\">\
             <p>peer <select name=\"peer\">{options}</select></p>\
             <p>secret <input name=\"secret\" size=\"50\"></p>\
             <p>label <input name=\"label\" size=\"40\"></p>\
             <button type=\"submit\">link</button></form>"
        )
    };

    format!(
        "{banner}{secret_banner}\
         <h4>tokens</h4>\
         <table class=\"list\"><tr><th>label</th><th>repos</th><th>scope</th><th>created</th><th>expires</th><th>last used</th><th></th></tr>{token_rows}</table>\
         <h4>create a token</h4>\
         <form method=\"post\">\
         <input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
         <input type=\"hidden\" name=\"action\" value=\"create\">\
         <p>label <input name=\"label\" size=\"40\"></p>\
         <p>repos (comma-separated) <input name=\"repos\" size=\"60\"></p>\
         <p><label><input type=\"checkbox\" name=\"write\"> read+write</label></p>\
         <p>expires in days (optional) <input name=\"expires_days\" size=\"20\"></p>\
         <button type=\"submit\">create</button></form>\
         <h4>linked peer credentials</h4>\
         <table class=\"list\"><tr><th>peer</th><th>label</th><th>linked</th><th></th></tr>{credential_rows}</table>\
         {link_form}"
    )
}
